use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use interface_common::sync::lock;
use interface_contracts::terminal::{
    CreateTerminalRequest, DismissTerminalRequest, RenameTerminalRequest, ResizeTerminalRequest,
    TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES, TerminalExit, TerminalGeneration,
    TerminalGenerationRequest, TerminalHandoffV1, TerminalId, TerminalLaunchProfileId,
    TerminalLaunchRequestV1, TerminalLaunchSessionV1, TerminalLifecycleEvent, TerminalSize,
    TerminalState, TerminalSummary,
};
use interface_terminal::{
    ApprovedTerminalRoots, CLAUDE_SESSION_BINDING_TITLE_PREFIX,
    CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER, ResolvedTerminalLaunch, SpawnedTerminal,
    SystemTerminalPtyBackend, TerminalBackendError, TerminalError, TerminalInput,
    TerminalLaunchCatalog, TerminalLaunchProgram, TerminalLeaseRequest, TerminalLifecycleSink,
    TerminalOutputRead, TerminalOutputReader, TerminalProcessControl, TerminalProcessIdentity,
    TerminalProcessWaiter, TerminalProviderSession, TerminalProviderSessionCaptureOwner,
    TerminalProviderSessionCaptureProcess, TerminalProviderSessionLookup,
    TerminalProviderSessionPersistence, TerminalProviderSessionTransition, TerminalPtyBackend,
    TerminalService, TerminalServiceConfig, TerminalServiceParts, TerminalSpawnSpec,
    TerminalStartupFileProbe, TerminalStartupFiles, TerminalWorkspaceLease,
    TerminalWorkspaceLeaseProvider,
};
use tempfile::TempDir;

fn temp_dir() -> TempDir {
    tempfile::Builder::new()
        .prefix("interface-embedded-terminal-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("temporary directory")
}

/// How long a test waits for another thread before it fails.
const WAIT: Duration = Duration::from_secs(2);

/// Poll `ready` every 5 ms until it holds; fail with `message` after `limit`.
fn eventually_within(limit: Duration, message: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// [`eventually_within`] the usual [`WAIT`].
fn eventually(message: &str, ready: impl FnMut() -> bool) {
    eventually_within(WAIT, message, ready);
}

/// Poll `found` every 5 ms until it returns a value; fail with `message`
/// after `limit`.
fn eventually_some<T>(limit: Duration, message: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(value) = found() {
            return value;
        }
        assert!(Instant::now() < deadline, "{message}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// One rendezvous between a test and a thread it holds up. The held side
/// counts each arrival and, when it must, parks until the test releases the
/// gate; the test waits for arrivals and then releases.
#[derive(Default)]
struct Gate {
    state: Mutex<(usize, bool)>,
    changed: Condvar,
}

impl Gate {
    /// Count one arrival and report whether the gate is still closed.
    fn enter(&self) -> bool {
        let mut state = lock(&self.state);
        state.0 = state.0.saturating_add(1);
        self.changed.notify_all();
        !state.1
    }

    /// Count one arrival, then park until the test releases the gate.
    fn enter_and_wait(&self) {
        let mut state = lock(&self.state);
        state.0 = state.0.saturating_add(1);
        self.changed.notify_all();
        drop(
            self.changed
                .wait_while(state, |state| !state.1)
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }

    /// Wait up to [`WAIT`] for `count` arrivals.
    fn wait_entered_times(&self, count: usize, message: &str) {
        let (state, _) = self
            .changed
            .wait_timeout_while(lock(&self.state), WAIT, |state| state.0 < count)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(state.0 >= count, "{message}");
    }

    /// Wait up to [`WAIT`] for the first arrival.
    fn wait_entered(&self, message: &str) {
        self.wait_entered_times(1, message);
    }

    fn release(&self) {
        lock(&self.state).1 = true;
        self.changed.notify_all();
    }

    fn released(&self) -> bool {
        lock(&self.state).1
    }
}

/// Read a real PTY's output until `marker` appears; fail with `message` after
/// `limit`.
fn read_until(
    output: &TerminalOutputReader,
    marker: &str,
    limit: Duration,
    message: &str,
) -> Vec<u8> {
    let deadline = Instant::now() + limit;
    let mut bytes = Vec::new();
    while !String::from_utf8_lossy(&bytes).contains(marker) {
        assert!(Instant::now() < deadline, "{message}");
        if let TerminalOutputRead::Chunk(chunk) = output.read_timeout(Duration::from_millis(100)) {
            bytes.extend(chunk.bytes);
        }
    }
    bytes
}

/// Read a real PTY's output until it closes or `limit` passes.
fn read_until_closed(output: &TerminalOutputReader, limit: Duration) -> Vec<u8> {
    let deadline = Instant::now() + limit;
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        match output.read_timeout(Duration::from_millis(100)) {
            TerminalOutputRead::Chunk(chunk) => bytes.extend(chunk.bytes),
            TerminalOutputRead::Closed => break,
            TerminalOutputRead::TimedOut => {}
        }
    }
    bytes
}

#[derive(Default)]
struct FakeCompletion {
    result: Mutex<Option<Result<TerminalExit, TerminalBackendError>>>,
    ready: Condvar,
}

impl FakeCompletion {
    fn complete(&self, result: Result<TerminalExit, TerminalBackendError>) {
        let mut slot = lock(&self.result);
        if slot.is_none() {
            *slot = Some(result);
            self.ready.notify_all();
        }
    }

    fn wait(&self) -> Result<TerminalExit, TerminalBackendError> {
        let mut slot = lock(&self.result);
        while slot.is_none() {
            slot = self
                .ready
                .wait(slot)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        slot.take().expect("completion exists")
    }
}

#[derive(Default)]
struct FakeRecord {
    writes: Mutex<Vec<Vec<u8>>>,
    resizes: Mutex<Vec<TerminalSize>>,
    first_resize_latch: Mutex<Option<Arc<Gate>>>,
    termination_gate: Mutex<Option<Arc<Gate>>>,
    closes: AtomicUsize,
    terminated: AtomicBool,
    suspended: AtomicBool,
    completion: Arc<FakeCompletion>,
    /// Reported by `TerminalProcessControl::process_id`. Left unset by every
    /// test that does not care, which is how the pass-off tests reach the
    /// recorded-cwd fallback.
    process_id: Mutex<Option<u32>>,
    process_identity: Mutex<Option<TerminalProcessIdentity>>,
    foreground_process_identity: Mutex<Option<TerminalProcessIdentity>>,
    foreground_profile: Mutex<Option<TerminalLaunchProfileId>>,
    foreground_process_cwd: Mutex<Option<PathBuf>>,
    foreground_process_live: AtomicBool,
    foreground_process_group_live: AtomicBool,
    foreground_survives_tree_teardown: AtomicBool,
    foreground_liveness_gate: Mutex<Option<Arc<Gate>>>,
    /// Parks the next foreground probe, then the next resume, once each.
    foreground_profile_gate: Mutex<Option<Arc<Gate>>>,
    resume_gate: Mutex<Option<Arc<Gate>>>,
}

impl FakeRecord {
    /// Point the live-cwd probe at a real process this test owns.
    #[cfg(target_os = "macos")]
    fn set_process_id(&self, pid: u32) {
        *lock(&self.process_id) = Some(pid);
    }

    fn exit(&self, code: i32) {
        self.completion.complete(Ok(TerminalExit {
            code: Some(code),
            signal: None,
        }));
    }

    fn block_first_resize_after_apply(&self, latch: Arc<Gate>) {
        *lock(&self.first_resize_latch) = Some(latch);
    }

    fn fail_termination_until_allowed(&self, gate: Arc<Gate>) {
        *lock(&self.termination_gate) = Some(gate);
    }

    fn exit_foreground_process(&self) {
        self.foreground_process_live.store(false, Ordering::SeqCst);
        self.foreground_process_group_live
            .store(false, Ordering::SeqCst);
    }

    fn exit_foreground_leader_only(&self) {
        self.foreground_process_live.store(false, Ordering::SeqCst);
    }

    fn exit_foreground_process_group(&self) {
        self.foreground_process_group_live
            .store(false, Ordering::SeqCst);
    }

    fn keep_foreground_process_live_on_tree_teardown(&self) {
        self.foreground_survives_tree_teardown
            .store(true, Ordering::SeqCst);
    }

    fn replace_foreground_process(&self, process: TerminalProcessIdentity, cwd: PathBuf) {
        *lock(&self.foreground_process_identity) = Some(process);
        *lock(&self.foreground_process_cwd) = Some(cwd);
        self.foreground_process_live.store(true, Ordering::SeqCst);
        self.foreground_process_group_live
            .store(true, Ordering::SeqCst);
    }

    fn gate_foreground_liveness(&self, gate: Arc<Gate>) {
        *lock(&self.foreground_liveness_gate) = Some(gate);
    }

    fn gate_foreground_profile(&self, gate: Arc<Gate>) {
        *lock(&self.foreground_profile_gate) = Some(gate);
    }

    fn gate_resume(&self, gate: Arc<Gate>) {
        *lock(&self.resume_gate) = Some(gate);
    }
}

impl TerminalInput for FakeRecord {
    fn write(&self, bytes: &[u8]) -> Result<(), TerminalBackendError> {
        lock(&self.writes).push(bytes.to_vec());
        Ok(())
    }

    fn close(&self) {}
}

impl TerminalProcessControl for FakeRecord {
    fn suspend_idle(&self, _agent_idle: bool) -> Result<bool, TerminalBackendError> {
        self.suspended.store(true, Ordering::SeqCst);
        Ok(true)
    }
    fn resume(&self) -> Result<(), TerminalBackendError> {
        let gate = lock(&self.resume_gate).take();
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        self.suspended.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn foreground_profile(&self) -> Result<Option<TerminalLaunchProfileId>, TerminalBackendError> {
        let gate = lock(&self.foreground_profile_gate).take();
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        Ok(*lock(&self.foreground_profile))
    }
    fn process_id(&self) -> Option<u32> {
        *lock(&self.process_id)
    }

    fn process_identity(&self) -> Option<TerminalProcessIdentity> {
        lock(&self.process_identity).clone()
    }

    fn foreground_process_identity(
        &self,
    ) -> Result<Option<TerminalProcessIdentity>, TerminalBackendError> {
        Ok(lock(&self.foreground_process_identity).clone())
    }

    fn process_identity_is_live(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<bool, TerminalBackendError> {
        if lock(&self.foreground_process_identity).as_ref() == Some(process)
            && let Some(gate) = lock(&self.foreground_liveness_gate).clone()
        {
            gate.enter_and_wait();
        }
        Ok(self.foreground_process_live.load(Ordering::SeqCst)
            && lock(&self.foreground_process_identity).as_ref() == Some(process))
    }

    fn process_group_identity_is_live(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<bool, TerminalBackendError> {
        if lock(&self.foreground_process_identity).as_ref() == Some(process)
            && let Some(gate) = lock(&self.foreground_liveness_gate).clone()
        {
            gate.enter_and_wait();
        }
        Ok(self.foreground_process_group_live.load(Ordering::SeqCst)
            && lock(&self.foreground_process_identity).as_ref() == Some(process))
    }

    fn process_cwd(
        &self,
        process: &TerminalProcessIdentity,
    ) -> Result<Option<PathBuf>, TerminalBackendError> {
        Ok(
            (lock(&self.foreground_process_identity).as_ref() == Some(process))
                .then(|| lock(&self.foreground_process_cwd).clone())
                .flatten(),
        )
    }

    fn resize(&self, size: TerminalSize) -> Result<(), TerminalBackendError> {
        let first = {
            let mut resizes = lock(&self.resizes);
            resizes.push(size);
            resizes.len() == 1
        };
        if first {
            if let Some(latch) = lock(&self.first_resize_latch).clone() {
                latch.enter_and_wait();
            }
        }
        Ok(())
    }

    fn terminate_tree(&self) -> Result<(), TerminalBackendError> {
        if let Some(gate) = lock(&self.termination_gate).clone()
            && gate.enter()
        {
            return Err(TerminalBackendError::new(
                "planned transient process teardown failure",
            ));
        }
        if self.terminated.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        if !self
            .foreground_survives_tree_teardown
            .load(Ordering::SeqCst)
        {
            self.foreground_process_live.store(false, Ordering::SeqCst);
            self.foreground_process_group_live
                .store(false, Ordering::SeqCst);
        }
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.completion.complete(Ok(TerminalExit {
            code: None,
            signal: Some(1),
        }));
        Ok(())
    }
}

struct FakeWaiter {
    completion: Arc<FakeCompletion>,
}

impl TerminalProcessWaiter for FakeWaiter {
    fn wait(&mut self) -> Result<TerminalExit, TerminalBackendError> {
        self.completion.wait()
    }
}

struct FakePlan {
    output: Vec<u8>,
    claude_provider_sessions: Vec<String>,
    claude_provider_steps: Vec<FakeProviderStep>,
    wrong_token_provider_session: Option<String>,
    previous_token_provider_session: Option<String>,
    output_suffix: Vec<u8>,
    fail_spawn: bool,
}

enum FakeProviderStep {
    Wake {
        source: &'static str,
        expected_session: String,
        live_session: Option<String>,
    },
    Live(String),
}

fn provider_transition(source: &str) -> TerminalProviderSessionTransition {
    match source {
        "startup" => TerminalProviderSessionTransition::Startup,
        "resume" => TerminalProviderSessionTransition::Resume,
        "clear" => TerminalProviderSessionTransition::Clear,
        "compact" => TerminalProviderSessionTransition::Compact,
        "fork" => TerminalProviderSessionTransition::Fork,
        other => panic!("unsupported fake provider transition {other}"),
    }
}

#[derive(Default)]
struct FakeProviderSessions {
    current: Mutex<BTreeMap<(u32, String), TerminalProviderSession>>,
    final_only: Mutex<BTreeMap<(u32, String), TerminalProviderSession>>,
    begun: Mutex<Vec<TerminalProviderSessionCaptureOwner>>,
    recorded: Mutex<Vec<(String, TerminalProcessIdentity)>>,
    captured: Mutex<BTreeMap<String, TerminalProviderSessionCaptureProcess>>,
    replace_on_seal: Mutex<Option<TerminalProviderSessionCaptureProcess>>,
    replace_on_settled: Mutex<Option<TerminalProviderSessionCaptureProcess>>,
    checkpointed: Mutex<Vec<(String, TerminalLaunchSessionV1)>>,
    rearmed: Mutex<Vec<(String, TerminalProcessIdentity, PathBuf)>>,
    ended: Mutex<Vec<String>>,
    sealed: Mutex<Vec<String>>,
    capture_reads: AtomicUsize,
    current_reads: Mutex<Vec<u32>>,
    final_reads: AtomicUsize,
    fail_final: AtomicBool,
    fail_begin: AtomicBool,
    fail_record: AtomicBool,
    fail_end: AtomicBool,
    fail_end_remaining: AtomicUsize,
    capture_process_gate: Mutex<Option<Arc<Gate>>>,
    current_session_gate: Mutex<Option<Arc<Gate>>>,
}

impl FakeProviderSessions {
    fn set(&self, process_id: u32, capture_token: &str, session: TerminalProviderSession) {
        lock(&self.current).insert((process_id, capture_token.to_owned()), session);
    }

    fn set_final(&self, process_id: u32, capture_token: &str, session: TerminalProviderSession) {
        lock(&self.final_only).insert((process_id, capture_token.to_owned()), session);
    }

    fn gate_capture_process(&self, gate: Arc<Gate>) {
        *lock(&self.capture_process_gate) = Some(gate);
    }

    fn set_captured(&self, capture_token: &str, process: TerminalProcessIdentity, cwd: PathBuf) {
        lock(&self.captured).insert(
            capture_token.to_owned(),
            TerminalProviderSessionCaptureProcess { process, cwd },
        );
    }

    fn replace_capture_during_next_settled_lookup(
        &self,
        captured: TerminalProviderSessionCaptureProcess,
    ) {
        *lock(&self.replace_on_settled) = Some(captured);
    }

    fn replace_capture_during_next_seal(&self, captured: TerminalProviderSessionCaptureProcess) {
        *lock(&self.replace_on_seal) = Some(captured);
    }

    /// The trusted wrapper's claim of a retained-shell capture, which the
    /// host makes outside the terminal service.
    fn record_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
        cwd: &Path,
    ) -> Result<(), TerminalBackendError> {
        if !cwd.is_absolute() {
            return Err(TerminalBackendError::new("planned unsafe nested cwd"));
        }
        self.record_process(profile_id, capture_token, process)?;
        self.set_captured(capture_token, process.clone(), cwd.to_path_buf());
        Ok(())
    }
}

impl TerminalProviderSessionLookup for FakeProviderSessions {
    fn begin_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        if profile_id == TerminalLaunchProfileId::Claude {
            if self.fail_begin.load(Ordering::SeqCst) {
                return Err(TerminalBackendError::new("planned capture begin failure"));
            }
            lock(&self.begun).push(owner.clone());
        }
        Ok(())
    }

    fn record_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        if profile_id == TerminalLaunchProfileId::Claude {
            lock(&self.recorded).push((capture_token.to_owned(), process.clone()));
            if self.fail_record.load(Ordering::SeqCst) {
                return Err(TerminalBackendError::new("planned capture record failure"));
            }
        }
        Ok(())
    }

    fn capture_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        if let Some(gate) = lock(&self.capture_process_gate).clone() {
            gate.enter_and_wait();
        }
        let captured = lock(&self.captured).get(capture_token).cloned();
        self.capture_reads.fetch_add(1, Ordering::SeqCst);
        Ok(captured)
    }

    fn seal_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        if let Some(replacement) = lock(&self.replace_on_seal).take() {
            lock(&self.captured).insert(capture_token.to_owned(), replacement);
        }
        let captured = self.capture_process(profile_id, capture_token)?;
        let mut sealed = lock(&self.sealed);
        if !sealed.iter().any(|token| token == capture_token) {
            sealed.push(capture_token.to_owned());
        }
        Ok(captured)
    }

    fn replace_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        _previous: &TerminalProcessIdentity,
        replacement: &TerminalProcessIdentity,
        cwd: &Path,
        _prior_launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        self.record_nested_process(profile_id, capture_token, replacement, cwd)?;
        Ok(lock(&self.current)
            .get(&(replacement.process_id, capture_token.to_owned()))
            .cloned())
    }

    fn checkpoint_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        launch_session: &TerminalLaunchSessionV1,
    ) -> Result<(), TerminalBackendError> {
        if profile_id == TerminalLaunchProfileId::Claude {
            lock(&self.checkpointed).push((capture_token.to_owned(), launch_session.clone()));
        }
        Ok(())
    }

    fn rearm_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
        cwd: &Path,
    ) -> Result<(), TerminalBackendError> {
        if profile_id == TerminalLaunchProfileId::Claude {
            lock(&self.rearmed).push((
                capture_token.to_owned(),
                process.clone(),
                cwd.to_path_buf(),
            ));
            lock(&self.captured).remove(capture_token);
        }
        Ok(())
    }

    fn current_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if !matches!(
            profile_id,
            TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Gemini
        ) {
            return Ok(None);
        }
        lock(&self.current_reads).push(process.process_id);
        let gate = lock(&self.current_session_gate).take();
        if let Some(gate) = gate {
            gate.enter_and_wait();
        }
        Ok(lock(&self.current)
            .get(&(process.process_id, capture_token.to_owned()))
            .cloned())
    }

    fn final_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if !matches!(
            profile_id,
            TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Gemini
        ) {
            return Ok(None);
        }
        self.final_reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_final.load(Ordering::SeqCst) {
            return Err(TerminalBackendError::new(
                "planned ambiguous final provider lookup",
            ));
        }
        Ok(lock(&self.final_only)
            .get(&(process.process_id, capture_token.to_owned()))
            .cloned()
            .or_else(|| {
                lock(&self.current)
                    .get(&(process.process_id, capture_token.to_owned()))
                    .cloned()
            }))
    }

    fn settled_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if let Some(replacement) = lock(&self.replace_on_settled).take() {
            lock(&self.captured).insert(capture_token.to_owned(), replacement);
            return Err(TerminalBackendError::new(
                "planned durable owner replacement",
            ));
        }
        if !matches!(
            profile_id,
            TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Gemini
        ) {
            return Ok(None);
        }
        Ok(lock(&self.final_only)
            .get(&(process.process_id, capture_token.to_owned()))
            .cloned()
            .or_else(|| {
                lock(&self.current)
                    .get(&(process.process_id, capture_token.to_owned()))
                    .cloned()
            }))
    }

    fn end_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        if profile_id == TerminalLaunchProfileId::Claude {
            if self.fail_end.load(Ordering::SeqCst)
                || self
                    .fail_end_remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        (remaining > 0).then(|| remaining - 1)
                    })
                    .is_ok()
            {
                return Err(TerminalBackendError::new(
                    "planned capture retirement failure",
                ));
            }
            lock(&self.ended).push(capture_token.to_owned());
            lock(&self.captured).remove(capture_token);
        }
        Ok(())
    }
}

struct RacingProviderSessions {
    live_lookup: Gate,
    stale: TerminalProviderSession,
    final_session: TerminalProviderSession,
}

impl TerminalProviderSessionLookup for RacingProviderSessions {
    fn current_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        self.live_lookup.enter_and_wait();
        Ok(Some(self.stale.clone()))
    }

    fn final_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        Ok(Some(self.final_session.clone()))
    }
}

struct FakeProviderReader {
    steps: VecDeque<(Vec<u8>, Option<TerminalProviderSession>)>,
    current: Option<(Vec<u8>, usize)>,
    provider_sessions: Arc<FakeProviderSessions>,
    process_id: u32,
    capture_token: String,
    nested_claim: Option<(TerminalProcessIdentity, PathBuf)>,
}

impl Read for FakeProviderReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if let Some((process, cwd)) = self.nested_claim.take() {
            self.provider_sessions
                .record_nested_process(
                    TerminalLaunchProfileId::Claude,
                    &self.capture_token,
                    &process,
                    &cwd,
                )
                .map_err(|error| std::io::Error::other(error.to_string()))?;
        }
        if self.current.is_none() {
            let Some((bytes, provider_session)) = self.steps.pop_front() else {
                return Ok(0);
            };
            if let Some(provider_session) = provider_session {
                self.provider_sessions
                    .set(self.process_id, &self.capture_token, provider_session);
            }
            self.current = Some((bytes, 0));
        }
        let (bytes, offset) = self.current.as_mut().expect("current fake output step");
        let read = buffer.len().min(bytes.len().saturating_sub(*offset));
        buffer[..read].copy_from_slice(&bytes[*offset..*offset + read]);
        *offset += read;
        if *offset == bytes.len() {
            self.current = None;
        }
        Ok(read)
    }
}

#[derive(Default)]
struct FakeBackend {
    plans: Mutex<VecDeque<FakePlan>>,
    records: Mutex<Vec<Arc<FakeRecord>>>,
    specs: Mutex<Vec<TerminalSpawnSpec>>,
    provider_sessions: Arc<FakeProviderSessions>,
    next_process_id: AtomicUsize,
    next_termination_gate: Mutex<Option<Arc<Gate>>>,
    next_foreground_cwd: Mutex<Option<PathBuf>>,
    /// Move the next spawn's cwd here and put a fresh directory in its place
    /// while that spawn runs, so the post-spawn cwd check fails.
    replace_cwd_on_next_spawn: Mutex<Option<PathBuf>>,
}

impl FakeBackend {
    fn with_output(output: impl Into<Vec<u8>>) -> Self {
        Self {
            plans: Mutex::new(VecDeque::from([FakePlan {
                output: output.into(),
                claude_provider_sessions: Vec::new(),
                claude_provider_steps: Vec::new(),
                wrong_token_provider_session: None,
                previous_token_provider_session: None,
                output_suffix: Vec::new(),
                fail_spawn: false,
            }])),
            ..Self::default()
        }
    }

    fn with_claude_binding(provider_session: &str) -> Self {
        Self {
            plans: Mutex::new(VecDeque::from([FakePlan {
                output: Vec::new(),
                claude_provider_sessions: vec![provider_session.to_owned()],
                claude_provider_steps: Vec::new(),
                wrong_token_provider_session: None,
                previous_token_provider_session: None,
                output_suffix: Vec::new(),
                fail_spawn: false,
            }])),
            ..Self::default()
        }
    }

    fn with_claude_wake_without_session(expected_session: &str) -> Self {
        Self {
            plans: Mutex::new(VecDeque::from([FakePlan {
                output: Vec::new(),
                claude_provider_sessions: Vec::new(),
                claude_provider_steps: vec![FakeProviderStep::Wake {
                    source: "startup",
                    expected_session: expected_session.to_owned(),
                    live_session: None,
                }],
                wrong_token_provider_session: None,
                previous_token_provider_session: None,
                output_suffix: Vec::new(),
                fail_spawn: false,
            }])),
            ..Self::default()
        }
    }

    fn record(&self, index: usize) -> Arc<FakeRecord> {
        Arc::clone(&lock(&self.records)[index])
    }

    fn fail_next_termination_until_allowed(&self, gate: Arc<Gate>) {
        *lock(&self.next_termination_gate) = Some(gate);
    }

    fn set_next_foreground_cwd(&self, cwd: PathBuf) {
        *lock(&self.next_foreground_cwd) = Some(cwd);
    }
}

impl TerminalPtyBackend for FakeBackend {
    #[allow(clippy::too_many_lines)] // One fake spawn script keeps each planned PTY transition ordered.
    fn spawn(&self, spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError> {
        lock(&self.specs).push(spec.clone());
        if let Some(moved) = lock(&self.replace_cwd_on_next_spawn).take() {
            std::fs::rename(&spec.cwd, moved).expect("move the spawn cwd away");
            std::fs::create_dir(&spec.cwd).expect("replace the spawn cwd");
        }
        let plan = lock(&self.plans).pop_front().unwrap_or(FakePlan {
            output: Vec::new(),
            claude_provider_sessions: Vec::new(),
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: None,
            output_suffix: Vec::new(),
            fail_spawn: false,
        });
        if plan.fail_spawn {
            return Err(TerminalBackendError::new("planned spawn failure"));
        }
        let record = Arc::new(FakeRecord::default());
        *lock(&record.termination_gate) = lock(&self.next_termination_gate).take();
        lock(&self.records).push(Arc::clone(&record));
        let input: Arc<dyn TerminalInput> = record.clone();
        let control: Arc<dyn TerminalProcessControl> = record.clone();
        let token = capture_token_from_launch_optional(&spec.launch).map(str::to_owned);
        let process_id = 10_000_u32.saturating_add(
            u32::try_from(self.next_process_id.fetch_add(1, Ordering::SeqCst))
                .unwrap_or(u32::MAX.saturating_sub(10_000)),
        );
        let manual_shell = matches!(
            spec.launch,
            ResolvedTerminalLaunch::LoginShellWithEnvironment { .. }
        );
        let claims_manual_provider = manual_shell
            && (!plan.claude_provider_sessions.is_empty()
                || !plan.claude_provider_steps.is_empty());
        let provider_process_id = if manual_shell {
            process_id.saturating_add(1_000)
        } else {
            process_id
        };
        if token.is_some() {
            *lock(&record.process_id) = Some(process_id);
            *lock(&record.process_identity) = Some(TerminalProcessIdentity {
                process_id,
                process_group_id: process_id,
                terminal_device: Some(format!("16/{process_id}")),
                process_start: format!("Mon Jan 1 00:00:{:02} 2024", process_id % 60),
            });
            if manual_shell {
                *lock(&record.foreground_process_identity) = Some(TerminalProcessIdentity {
                    process_id: provider_process_id,
                    process_group_id: provider_process_id,
                    terminal_device: Some(format!("16/{process_id}")),
                    process_start: format!("Mon Jan 1 00:01:{:02} 2024", provider_process_id % 60),
                });
                *lock(&record.foreground_process_cwd) = Some(
                    lock(&self.next_foreground_cwd)
                        .take()
                        .unwrap_or_else(|| spec.cwd.clone()),
                );
                record.foreground_process_live.store(true, Ordering::SeqCst);
                record
                    .foreground_process_group_live
                    .store(true, Ordering::SeqCst);
            }
        }
        let nested_claim = claims_manual_provider.then(|| {
            let process = lock(&record.foreground_process_identity)
                .clone()
                .expect("manual provider process");
            let cwd = lock(&record.foreground_process_cwd)
                .clone()
                .expect("manual provider cwd");
            (process, cwd)
        });
        let nested_claim =
            nested_claim.filter(|(_, cwd)| cwd == &spec.cwd || cwd.starts_with(&spec.cwd));
        let mut steps = VecDeque::new();
        if !plan.output.is_empty() {
            steps.push_back((plan.output, None));
        }
        if let Some(provider) = plan.wrong_token_provider_session {
            steps.push_back((
                format!(
                    "\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}99999999-9999-4999-8999-999999999999:clear:{provider}\u{7}"
                )
                .into_bytes(),
                None,
            ));
        }
        if let Some(provider) = plan.previous_token_provider_session {
            let previous = lock(&self.specs)
                .iter()
                .rev()
                .nth(1)
                .map(|previous| capture_token_from_launch(&previous.launch).to_owned())
                .expect("previous Claude generation token");
            steps.push_back((
                format!(
                    "\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}{previous}:clear:{provider}\u{7}"
                )
                .into_bytes(),
                None,
            ));
        }
        if let Some(token) = token.as_deref() {
            let resume_launch = match &spec.launch {
                ResolvedTerminalLaunch::Program { arguments, .. } => {
                    arguments.iter().any(|argument| {
                        argument
                            .to_str()
                            .is_some_and(|argument| argument.starts_with("--resume"))
                    })
                }
                ResolvedTerminalLaunch::LoginShell
                | ResolvedTerminalLaunch::LoginShellWithEnvironment { .. } => false,
            };
            let mut previous_provider: Option<String> = None;
            for provider in plan.claude_provider_sessions {
                let source = match previous_provider.as_deref() {
                    None if resume_launch => "resume",
                    None => "startup",
                    Some(previous) if previous == provider => "compact",
                    Some(_) => "clear",
                };
                steps.push_back((
                    format!(
                        "\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}{token}:{source}:{provider}\u{7}"
                    )
                    .into_bytes(),
                    Some(TerminalProviderSession {
                        id: provider.clone(),
                        transition: provider_transition(source),
                    }),
                ));
                previous_provider = Some(provider);
            }
            for step in plan.claude_provider_steps {
                match step {
                    FakeProviderStep::Wake {
                        source,
                        expected_session,
                        live_session,
                    } => steps.push_back((
                        format!(
                            "\u{1b}]2;{CLAUDE_SESSION_BINDING_TITLE_PREFIX}{token}:{source}:{expected_session}\u{7}"
                        )
                        .into_bytes(),
                        live_session.map(|session_id| TerminalProviderSession {
                            id: session_id,
                            transition: provider_transition(source),
                        }),
                    )),
                    // One visible byte makes the fake reader advance the
                    // verified provider state without sending a private wake.
                    FakeProviderStep::Live(session_id) => {
                        steps.push_back((
                            b".".to_vec(),
                            Some(TerminalProviderSession {
                                id: session_id,
                                transition: TerminalProviderSessionTransition::Resume,
                            }),
                        ));
                    }
                }
            }
        }
        if !plan.output_suffix.is_empty() {
            steps.push_back((plan.output_suffix, None));
        }
        Ok(SpawnedTerminal {
            reader: Box::new(FakeProviderReader {
                steps,
                current: None,
                provider_sessions: Arc::clone(&self.provider_sessions),
                process_id: provider_process_id,
                capture_token: token.unwrap_or_default(),
                nested_claim,
            }),
            input,
            control,
            waiter: Box::new(FakeWaiter {
                completion: Arc::clone(&record.completion),
            }),
        })
    }
}

fn capture_token_from_launch_optional(launch: &ResolvedTerminalLaunch) -> Option<&str> {
    match launch {
        ResolvedTerminalLaunch::Program {
            arguments,
            environment,
            ..
        } => arguments
            .iter()
            .find_map(|argument| argument.to_str()?.strip_prefix("capture="))
            .or_else(|| {
                environment.iter().find_map(|(name, value)| {
                    (name == interface_terminal::native_checkpoint::TOKEN_ENV)
                        .then(|| value.to_str())
                        .flatten()
                })
            }),
        ResolvedTerminalLaunch::LoginShellWithEnvironment { environment } => environment
            .iter()
            .find_map(|(_, value)| value.to_str()?.strip_prefix("capture=")),
        ResolvedTerminalLaunch::LoginShell => None,
    }
}

fn capture_token_from_launch(launch: &ResolvedTerminalLaunch) -> &str {
    capture_token_from_launch_optional(launch).expect("Claude capture token in fake argv")
}

/// Counts a fake PTY reader's reads and its drop.
#[derive(Default)]
struct ReaderLifecycle {
    reads: Gate,
    dropped: Gate,
}

struct DropTrackedReader {
    cursor: Cursor<Vec<u8>>,
    lifecycle: Arc<ReaderLifecycle>,
}

impl Read for DropTrackedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.cursor.read(buffer)?;
        if read > 0 {
            self.lifecycle.reads.enter();
        }
        Ok(read)
    }
}

impl Drop for DropTrackedReader {
    fn drop(&mut self) {
        self.lifecycle.dropped.enter();
    }
}

struct SaturatingOutputBackend {
    record: Arc<FakeRecord>,
    lifecycle: Arc<ReaderLifecycle>,
}

impl SaturatingOutputBackend {
    fn new() -> Self {
        Self {
            record: Arc::new(FakeRecord::default()),
            lifecycle: Arc::new(ReaderLifecycle::default()),
        }
    }
}

impl TerminalPtyBackend for SaturatingOutputBackend {
    fn spawn(&self, _spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError> {
        let input: Arc<dyn TerminalInput> = self.record.clone();
        let control: Arc<dyn TerminalProcessControl> = self.record.clone();
        Ok(SpawnedTerminal {
            reader: Box::new(DropTrackedReader {
                cursor: Cursor::new(b"abcdefgh".to_vec()),
                lifecycle: Arc::clone(&self.lifecycle),
            }),
            input,
            control,
            waiter: Box::new(FakeWaiter {
                completion: Arc::clone(&self.record.completion),
            }),
        })
    }
}

struct DelayedTailReader {
    cursor: Cursor<Vec<u8>>,
    delay: Arc<Gate>,
    started: bool,
}

impl Read for DelayedTailReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if !self.started {
            self.started = true;
            self.delay.enter_and_wait();
        }
        self.cursor.read(buffer)
    }
}

struct DelayedTailBackend {
    record: Arc<FakeRecord>,
    delay: Arc<Gate>,
    tail: Vec<u8>,
}

impl DelayedTailBackend {
    fn new(tail: &[u8]) -> Self {
        Self {
            record: Arc::new(FakeRecord::default()),
            delay: Arc::new(Gate::default()),
            tail: tail.to_vec(),
        }
    }
}

impl TerminalPtyBackend for DelayedTailBackend {
    fn spawn(&self, _spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError> {
        let input: Arc<dyn TerminalInput> = self.record.clone();
        let control: Arc<dyn TerminalProcessControl> = self.record.clone();
        Ok(SpawnedTerminal {
            reader: Box::new(DelayedTailReader {
                cursor: Cursor::new(self.tail.clone()),
                delay: Arc::clone(&self.delay),
                started: false,
            }),
            input,
            control,
            waiter: Box::new(FakeWaiter {
                completion: Arc::clone(&self.record.completion),
            }),
        })
    }
}

#[derive(Default)]
struct LeaseCounts {
    acquired: AtomicUsize,
    released: AtomicUsize,
    requests: Mutex<Vec<TerminalLeaseRequest>>,
    /// Refuse every acquisition while set.
    refuse: AtomicBool,
}

struct TrackedLease {
    counts: Arc<LeaseCounts>,
}

impl TerminalWorkspaceLease for TrackedLease {}

impl Drop for TrackedLease {
    fn drop(&mut self) {
        self.counts.released.fetch_add(1, Ordering::SeqCst);
    }
}

impl TerminalWorkspaceLeaseProvider for LeaseCounts {
    fn acquire(
        &self,
        request: &TerminalLeaseRequest,
    ) -> Result<Box<dyn TerminalWorkspaceLease>, TerminalBackendError> {
        self.acquired.fetch_add(1, Ordering::SeqCst);
        lock(&self.requests).push(request.clone());
        // The tests wrap this provider in an Arc and replace this temporary
        // counter below via SharedLeaseProvider so the Drop guard shares it.
        Err(TerminalBackendError::new(
            "LeaseCounts must be used through SharedLeaseProvider",
        ))
    }
}

struct SharedLeaseProvider(Arc<LeaseCounts>);

impl TerminalWorkspaceLeaseProvider for SharedLeaseProvider {
    fn acquire(
        &self,
        request: &TerminalLeaseRequest,
    ) -> Result<Box<dyn TerminalWorkspaceLease>, TerminalBackendError> {
        if self.0.refuse.load(Ordering::SeqCst) {
            return Err(TerminalBackendError::new("planned lease refusal"));
        }
        self.0.acquired.fetch_add(1, Ordering::SeqCst);
        lock(&self.0.requests).push(request.clone());
        Ok(Box::new(TrackedLease {
            counts: Arc::clone(&self.0),
        }))
    }
}

struct BlockingLease {
    drop_state: Arc<Gate>,
}

impl TerminalWorkspaceLease for BlockingLease {}

impl Drop for BlockingLease {
    fn drop(&mut self) {
        self.drop_state.enter_and_wait();
    }
}

struct BlockingLeaseProvider(Arc<Gate>);

impl TerminalWorkspaceLeaseProvider for BlockingLeaseProvider {
    fn acquire(
        &self,
        _request: &TerminalLeaseRequest,
    ) -> Result<Box<dyn TerminalWorkspaceLease>, TerminalBackendError> {
        Ok(Box::new(BlockingLease {
            drop_state: Arc::clone(&self.0),
        }))
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<TerminalLifecycleEvent>>);

impl TerminalLifecycleSink for Events {
    fn publish(&self, event: TerminalLifecycleEvent) {
        lock(&self.0).push(event);
    }
}

#[derive(Default)]
struct DurableReopenEvents {
    events: Mutex<Vec<TerminalLifecycleEvent>>,
    persisted: Mutex<Vec<TerminalSummary>>,
}

impl TerminalLifecycleSink for DurableReopenEvents {
    fn publish(&self, event: TerminalLifecycleEvent) {
        lock(&self.events).push(event);
    }

    fn persist_provider_reopen(
        &self,
        terminal: &TerminalSummary,
    ) -> TerminalProviderSessionPersistence {
        lock(&self.persisted).push(terminal.clone());
        TerminalProviderSessionPersistence::Durable
    }
}

#[derive(Default)]
struct RefusingPersistenceEvents(Mutex<Vec<TerminalLifecycleEvent>>);

impl TerminalLifecycleSink for RefusingPersistenceEvents {
    fn publish(&self, event: TerminalLifecycleEvent) {
        lock(&self.0).push(event);
    }

    fn persist_provider_reopen(
        &self,
        terminal: &TerminalSummary,
    ) -> TerminalProviderSessionPersistence {
        if terminal.launch_session.is_none() {
            return TerminalProviderSessionPersistence::NotSaved;
        }
        TerminalProviderSessionPersistence::RetryableFailure
    }
}

#[derive(Default)]
struct FailOncePersistenceEvents {
    events: Mutex<Vec<TerminalLifecycleEvent>>,
    attempts: AtomicUsize,
}

#[derive(Default)]
struct GatedManualDemotionEvents {
    events: Mutex<Vec<TerminalLifecycleEvent>>,
    allow_demotion: AtomicBool,
    demotion_attempts: AtomicUsize,
}

impl TerminalLifecycleSink for GatedManualDemotionEvents {
    fn publish(&self, event: TerminalLifecycleEvent) {
        lock(&self.events).push(event);
    }

    fn persist_provider_reopen(
        &self,
        terminal: &TerminalSummary,
    ) -> TerminalProviderSessionPersistence {
        let demotion = terminal.launch_session.is_none()
            && terminal
                .launch
                .is_none_or(|launch| launch.profile_id == TerminalLaunchProfileId::Shell);
        if !demotion {
            return TerminalProviderSessionPersistence::NotSaved;
        }
        self.demotion_attempts.fetch_add(1, Ordering::SeqCst);
        if self.allow_demotion.load(Ordering::SeqCst) {
            TerminalProviderSessionPersistence::Durable
        } else {
            TerminalProviderSessionPersistence::RetryableFailure
        }
    }
}

impl TerminalLifecycleSink for FailOncePersistenceEvents {
    fn publish(&self, event: TerminalLifecycleEvent) {
        lock(&self.events).push(event);
    }

    fn persist_provider_reopen(
        &self,
        terminal: &TerminalSummary,
    ) -> TerminalProviderSessionPersistence {
        if terminal.launch_session.is_none() {
            return TerminalProviderSessionPersistence::NotSaved;
        }
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            TerminalProviderSessionPersistence::RetryableFailure
        } else {
            TerminalProviderSessionPersistence::Durable
        }
    }
}

fn request(id: TerminalId, directory: &TempDir) -> CreateTerminalRequest {
    CreateTerminalRequest {
        sign_in: false,
        id,
        cwd: directory.path().to_string_lossy().into_owned(),
        size: TerminalSize {
            columns: 80,
            rows: 24,
        },
        title: Some("Main shell".to_owned()),
        session_key: Some("codex#terminal-test".to_owned()),
        launch: None,
        launch_session: None,
        handoff: None,
        account: None,
    }
}

fn service(
    backend: Arc<FakeBackend>,
    leases: Arc<LeaseCounts>,
    events: Arc<Events>,
    config: TerminalServiceConfig,
) -> TerminalService {
    TerminalService::new(TerminalServiceParts::new(
        config,
        backend,
        Arc::new(SharedLeaseProvider(leases)),
        events,
    ))
    .expect("service")
}

#[test]
fn live_workspace_tool_authority_survives_reopen_grant_and_ends_on_close() {
    let directory = temp_dir();
    let service = service(
        Arc::new(FakeBackend::default()),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    assert!(service.active_workspace_roots().is_empty());
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let created = service
        .create(request(TerminalId::random(), &directory), &approved)
        .unwrap();
    // A saved-workspace reopen uses a one-shot host grant. The live PTY keeps
    // its verified root after that grant leaves scope, without trusting a DTO.
    drop(approved);
    let roots = ApprovedTerminalRoots::new(service.active_workspace_roots()).unwrap();
    assert_eq!(
        roots.owning_root(directory.path()).unwrap(),
        directory.path().canonicalize().unwrap()
    );
    service
        .close(TerminalGenerationRequest {
            id: created.id,
            generation: created.generation,
        })
        .unwrap();
    assert!(service.active_workspace_roots().is_empty());
}

fn spawn_arguments(backend: &FakeBackend, index: usize) -> Vec<OsString> {
    match &lock(&backend.specs)[index].launch {
        ResolvedTerminalLaunch::Program { arguments, .. } => arguments.clone(),
        ResolvedTerminalLaunch::LoginShell
        | ResolvedTerminalLaunch::LoginShellWithEnvironment { .. } => {
            panic!("a curated profile must not fall back")
        }
    }
}

fn claude_provider_arguments(backend: &FakeBackend, index: usize) -> Vec<OsString> {
    spawn_arguments(backend, index)
        .into_iter()
        .filter(|argument| {
            argument
                .to_str()
                .is_none_or(|argument| !argument.starts_with("capture="))
        })
        .collect()
}

fn claude_capture_token(backend: &FakeBackend, index: usize) -> String {
    spawn_arguments(backend, index)
        .into_iter()
        .find_map(|argument| {
            argument
                .into_string()
                .ok()?
                .strip_prefix("capture=")
                .map(str::to_owned)
        })
        .expect("Claude launch capture token")
}

#[test]
fn capture_process_record_failure_tears_down_spawn_and_retires_owner() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    backend
        .provider_sessions
        .fail_record
        .store(true, Ordering::SeqCst);
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });

    assert_eq!(
        service.create(launched, &approved),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        ))
    );
    let token = claude_capture_token(&backend, 0);
    assert_eq!(lock(&backend.provider_sessions.begun).len(), 1);
    assert_eq!(lock(&backend.provider_sessions.recorded).len(), 1);
    assert_eq!(lock(&backend.provider_sessions.ended).as_slice(), &[token]);
    assert!(backend.record(0).terminated.load(Ordering::SeqCst));
    assert!(service.snapshot().terminals.is_empty());
}

#[test]
fn capture_process_record_failure_preserves_owner_during_deferred_teardown() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let gate = Arc::new(Gate::default());
    backend
        .provider_sessions
        .fail_record
        .store(true, Ordering::SeqCst);
    backend.fail_next_termination_until_allowed(Arc::clone(&gate));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });

    assert_eq!(
        service.create(launched, &approved),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        ))
    );
    gate.wait_entered("process teardown was not attempted");
    assert!(
        lock(&backend.provider_sessions.ended).is_empty(),
        "a capture owner was retired while its failed spawn remained alive"
    );
    assert_eq!(leases.released.load(Ordering::SeqCst), 0);

    gate.release();
    eventually(
        "deferred failed-spawn teardown did not release its lease",
        || leases.released.load(Ordering::SeqCst) == 1,
    );
    assert!(backend.record(0).terminated.load(Ordering::SeqCst));
    assert!(
        lock(&backend.provider_sessions.ended).is_empty(),
        "deferred teardown must leave capture recovery to the startup reconciler"
    );
}

/// One way a spawn attempt fails, from the capture owner through the
/// post-spawn cwd check.
#[derive(Clone, Copy, Debug)]
enum SpawnFailure {
    BeginCapture,
    Lease,
    Spawn,
    RecordProcess,
    CwdReplaced,
}

const SPAWN_FAILURES: [SpawnFailure; 5] = [
    SpawnFailure::BeginCapture,
    SpawnFailure::Lease,
    SpawnFailure::Spawn,
    SpawnFailure::RecordProcess,
    SpawnFailure::CwdReplaced,
];

impl SpawnFailure {
    fn arm(self, backend: &FakeBackend, leases: &LeaseCounts, moved: &Path) {
        match self {
            Self::BeginCapture => backend
                .provider_sessions
                .fail_begin
                .store(true, Ordering::SeqCst),
            Self::Lease => leases.refuse.store(true, Ordering::SeqCst),
            Self::Spawn => lock(&backend.plans).push_back(FakePlan {
                output: Vec::new(),
                claude_provider_sessions: Vec::new(),
                claude_provider_steps: Vec::new(),
                wrong_token_provider_session: None,
                previous_token_provider_session: None,
                output_suffix: Vec::new(),
                fail_spawn: true,
            }),
            Self::RecordProcess => backend
                .provider_sessions
                .fail_record
                .store(true, Ordering::SeqCst),
            Self::CwdReplaced => {
                *lock(&backend.replace_cwd_on_next_spawn) = Some(moved.to_path_buf());
            }
        }
    }

    fn error(self) -> TerminalError {
        match self {
            Self::BeginCapture | Self::RecordProcess => {
                TerminalError::ProviderSessionCaptureUnavailable(TerminalLaunchProfileId::Claude)
            }
            Self::Lease => TerminalError::Lease("planned lease refusal".to_owned()),
            Self::Spawn => TerminalError::Backend("planned spawn failure".to_owned()),
            Self::CwdReplaced => TerminalError::CwdChanged,
        }
    }

    /// Whether this failure happens after the process exists.
    const fn spawned(self) -> bool {
        matches!(self, Self::RecordProcess | Self::CwdReplaced)
    }
}

/// A capturing Claude pane's spawn that fails is undone the same way by create
/// and restart: the capture owner is retired, a spawned process is torn down
/// before its lease is released, a pass-off transcript is removed, and no
/// generation is published.
#[allow(clippy::too_many_lines)] // One fixture, one failed attempt, every undo checked.
fn assert_failed_spawn_is_undone(failure: SpawnFailure, restart: bool) {
    let root = temp_dir();
    let binaries = temp_dir();
    let handoffs = temp_dir();
    let aside = temp_dir();
    let moved = aside.path().join("moved");
    let transcripts = handoffs.path().join("transcripts");
    let approved = ApprovedTerminalRoots::new([root.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let events = Arc::new(Events::default());
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> =
        backend.provider_sessions.clone();
    let service = TerminalService::new(TerminalServiceParts {
        launches: claude_capture_catalog(&binaries),
        provider_sessions,
        handoff_directory: transcripts.clone(),
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::clone(&backend) as Arc<dyn TerminalPtyBackend>,
            Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
            Arc::clone(&events) as Arc<dyn TerminalLifecycleSink>,
        )
    })
    .expect("service");
    let claude = |id| {
        let mut claude = request(id, &root);
        claude.launch = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Claude,
        });
        claude
    };
    // The pane that fails: a pass-off from a live source for create, or an
    // exited Claude pane for restart.
    let source = service
        .create(claude(TerminalId::random()), &approved)
        .expect("source pane");
    let (attempt, id) = if restart {
        backend.record(0).exit(0);
        await_state(&service, source.id, TerminalState::Exited);
        (None, source.id)
    } else {
        let id = TerminalId::random();
        let mut pass_off = claude(id);
        pass_off.handoff = Some(TerminalHandoffV1 {
            version: 1,
            source_terminal_id: source.id,
            transcript: "did A".to_owned(),
            from_title: None,
            kind: None,
        });
        (Some(pass_off), id)
    };
    let spawns = lock(&backend.specs).len();
    let acquired = leases.acquired.load(Ordering::SeqCst);
    let ended = lock(&backend.provider_sessions.ended).len();
    let published = lock(&events.0).len();
    let before = service.snapshot();
    failure.arm(&backend, &leases, &moved);

    let result = match attempt {
        Some(pass_off) => service.create(pass_off, &approved).map(|_| ()),
        None => service
            .restart(
                TerminalGenerationRequest {
                    id,
                    generation: source.generation,
                },
                &approved,
            )
            .map(|_| ()),
    };

    assert_eq!(result, Err(failure.error()), "{failure:?}");
    let spawned = lock(&backend.specs).len() - spawns;
    assert_eq!(
        spawned,
        usize::from(!matches!(
            failure,
            SpawnFailure::BeginCapture | SpawnFailure::Lease
        )),
        "{failure:?}: spawn attempts"
    );
    if failure.spawned() {
        assert!(
            backend.record(1).terminated.load(Ordering::SeqCst),
            "{failure:?}: the spawned process was not torn down"
        );
    }
    let taken = leases.acquired.load(Ordering::SeqCst) - acquired;
    eventually("the failed generation kept its lease", || {
        leases.released.load(Ordering::SeqCst) == taken + usize::from(restart)
    });
    let retired = lock(&backend.provider_sessions.ended)[ended..].to_vec();
    assert_eq!(retired.len(), 1, "{failure:?}: capture owner retirement");
    assert_ne!(
        retired[0],
        claude_capture_token(&backend, 0),
        "{failure:?}: the earlier generation's capture was retired"
    );
    if spawned == 1 {
        assert_eq!(retired[0], claude_capture_token(&backend, spawns));
    }
    assert_eq!(lock(&events.0).len(), published, "{failure:?}: published");
    assert_eq!(service.snapshot(), before, "{failure:?}: registry changed");
    assert!(
        std::fs::read_dir(&transcripts)
            .map(|entries| entries.count() == 0)
            .unwrap_or(true),
        "{failure:?}: a transcript outlived its pane"
    );
}

#[test]
fn a_failed_create_undoes_each_spawn_step() {
    for failure in SPAWN_FAILURES {
        assert_failed_spawn_is_undone(failure, false);
    }
}

#[test]
fn a_failed_restart_undoes_each_spawn_step() {
    for failure in SPAWN_FAILURES {
        assert_failed_spawn_is_undone(failure, true);
    }
}

/// A spawn whose cwd changed under it is torn down like any other failed
/// spawn: when the teardown cannot be proven, its workspace lease and capture
/// owner stay until a retry proves it.
fn assert_cwd_change_keeps_the_lease_until_teardown(restart: bool) {
    let root = temp_dir();
    let binaries = temp_dir();
    let aside = temp_dir();
    let approved = ApprovedTerminalRoots::new([root.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        claude_capture_catalog(&binaries),
    );
    let mut claude = request(TerminalId::random(), &root);
    claude.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    let first = restart.then(|| {
        let first = service.create(claude.clone(), &approved).expect("first");
        backend.record(0).exit(0);
        await_state(&service, first.id, TerminalState::Exited);
        first
    });
    let released = leases.released.load(Ordering::SeqCst);
    let ended = lock(&backend.provider_sessions.ended).len();
    let gate = Arc::new(Gate::default());
    backend.fail_next_termination_until_allowed(Arc::clone(&gate));
    *lock(&backend.replace_cwd_on_next_spawn) = Some(aside.path().join("moved"));

    let result = match first {
        Some(first) => service
            .restart(
                TerminalGenerationRequest {
                    id: first.id,
                    generation: first.generation,
                },
                &approved,
            )
            .map(|_| ()),
        None => service.create(claude, &approved).map(|_| ()),
    };

    assert_eq!(result, Err(TerminalError::CwdChanged));
    gate.wait_entered("process teardown was not attempted");
    assert_eq!(
        leases.released.load(Ordering::SeqCst),
        released,
        "the lease was released before teardown was proven"
    );
    assert_eq!(
        lock(&backend.provider_sessions.ended).len(),
        ended,
        "a capture owner was retired while its failed spawn remained alive"
    );
    gate.release();
    eventually("the proven teardown did not release its lease", || {
        leases.released.load(Ordering::SeqCst) == released + 1
    });
    let spawned = backend.record(usize::from(restart));
    assert!(spawned.terminated.load(Ordering::SeqCst));
    assert_eq!(
        lock(&backend.provider_sessions.ended).len(),
        ended,
        "deferred teardown must leave capture recovery to the startup reconciler"
    );
}

#[test]
fn a_create_whose_cwd_changed_keeps_its_lease_until_teardown_is_proven() {
    assert_cwd_change_keeps_the_lease_until_teardown(false);
}

#[test]
fn a_restart_whose_cwd_changed_keeps_its_lease_until_teardown_is_proven() {
    assert_cwd_change_keeps_the_lease_until_teardown(true);
}

fn service_with_launches<E>(
    backend: Arc<FakeBackend>,
    leases: Arc<LeaseCounts>,
    events: Arc<E>,
    launches: TerminalLaunchCatalog,
) -> TerminalService
where
    E: TerminalLifecycleSink + 'static,
{
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> =
        backend.provider_sessions.clone();
    let events: Arc<dyn TerminalLifecycleSink> = events;
    TerminalService::new(TerminalServiceParts {
        launches,
        provider_sessions,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            backend,
            Arc::new(SharedLeaseProvider(leases)),
            events,
        )
    })
    .expect("service")
}

/// The current summary of terminal `id`, if the service still has it.
fn summary_of(service: &TerminalService, id: TerminalId) -> Option<TerminalSummary> {
    service
        .snapshot()
        .terminals
        .into_iter()
        .find(|summary| summary.id == id)
}

fn await_state(
    service: &TerminalService,
    id: TerminalId,
    expected: TerminalState,
) -> interface_contracts::terminal::TerminalSummary {
    eventually_some(WAIT, "terminal state timed out", || {
        summary_of(service, id).filter(|summary| summary.state == expected)
    })
}

fn claude_capture_catalog(directory: &TempDir) -> TerminalLaunchCatalog {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let executable = directory.path().join("fixed-claude");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("fake Claude");
    #[cfg(unix)]
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("Claude permissions");
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        &executable,
        vec![OsString::from(format!(
            "capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
        ))],
        Vec::new(),
    )
    .expect("launch program")
    .with_provider_session_capture()
    .expect("provider session capture");
    TerminalLaunchCatalog::new([program]).expect("launch catalog")
}

fn manual_claude_capture_catalog(directory: &TempDir) -> TerminalLaunchCatalog {
    claude_capture_catalog(directory)
        .with_shell_provider_session_capture(vec![(
            OsString::from("INTERFACE_TEST_MANUAL_CLAUDE_CAPTURE"),
            OsString::from(format!(
                "capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
            )),
        )])
        .expect("manual Claude shell capture")
}

fn await_launch_session_id(
    service: &TerminalService,
    id: TerminalId,
    expected: &str,
) -> interface_contracts::terminal::TerminalSummary {
    eventually_some(WAIT, "provider binding was not adopted", || {
        let summary = summary_of(service, id).expect("terminal summary");
        let bound = summary
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref())
            == Some(expected);
        bound.then_some(summary)
    })
}

#[test]
fn creation_is_exact_root_authorized_idempotent_and_conflict_safe() {
    let directory = temp_dir();
    let child = directory.path().join("nested");
    std::fs::create_dir(&child).expect("nested directory");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let events = Arc::new(Events::default());
    let service = service(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    assert_eq!(created.generation, TerminalGeneration::initial());
    assert_eq!(created.state, TerminalState::Running);
    assert_eq!(created.root, directory.path().to_string_lossy());
    assert_eq!(lock(&backend.specs).len(), 1);
    assert_eq!(leases.acquired.load(Ordering::SeqCst), 1);

    assert_eq!(
        service
            .create(request(id, &directory), &approved)
            .expect("idempotent create"),
        created
    );
    assert_eq!(lock(&backend.specs).len(), 1);

    let mut conflict = request(id, &directory);
    conflict.title = Some("Different".to_owned());
    assert_eq!(
        service.create(conflict, &approved),
        Err(TerminalError::CreateConflict(id))
    );

    let nested = CreateTerminalRequest {
        id: TerminalId::random(),
        cwd: child.to_string_lossy().into_owned(),
        ..request(TerminalId::random(), &directory)
    };
    assert_eq!(
        service.create(nested, &approved),
        Err(TerminalError::UnauthorizedCwd)
    );
    assert_eq!(lock(&backend.specs).len(), 1);
}

#[test]
fn exact_create_replay_wins_before_mutable_provider_history_is_read() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut original = request(id, &directory);
    original.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    original.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("12345678-1234-4234-8234-123456789abc".to_owned()),
        resume: true,
    });

    let created = service
        .create_with_provider_history_check(original.clone(), &approved, |mut recovered| {
            recovered
                .launch_session
                .as_mut()
                .expect("Claude binding")
                .id = None;
            Ok(recovered)
        })
        .expect("first recovered create");
    assert_eq!(
        claude_provider_arguments(&backend, 0),
        vec![OsString::from("--resume")]
    );

    let history_was_read = Arc::new(AtomicBool::new(false));
    let history_was_read_in_check = Arc::clone(&history_was_read);
    let replayed = service
        .create_with_provider_history_check(original, &approved, move |_| {
            history_was_read_in_check.store(true, Ordering::SeqCst);
            Err(TerminalError::ProviderSessionLookupFailed(
                TerminalLaunchProfileId::Claude,
            ))
        })
        .expect("exact replay despite unavailable provider store");
    assert_eq!(replayed, created);
    assert!(!history_was_read.load(Ordering::SeqCst));
    assert_eq!(lock(&backend.specs).len(), 1);

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close recovered Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn provider_history_lookup_does_not_block_unrelated_terminal_create() {
    let directory = temp_dir();
    let approved =
        Arc::new(ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots"));
    let backend = Arc::new(FakeBackend::default());
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let first_id = TerminalId::random();
    let second_id = TerminalId::random();
    let (history_started_tx, history_started_rx) = mpsc::channel();
    let (release_history_tx, release_history_rx) = mpsc::channel();
    let first_service = service.clone();
    let first_approved = Arc::clone(&approved);
    let first_directory = directory.path().to_path_buf();
    let first = thread::spawn(move || {
        let first_request = CreateTerminalRequest {
            sign_in: false,
            id: first_id,
            cwd: first_directory.to_string_lossy().into_owned(),
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
            title: Some("First".to_owned()),
            session_key: None,
            launch: None,
            launch_session: None,
            handoff: None,
            account: None,
        };
        first_service.create_with_provider_history_check(
            first_request,
            &first_approved,
            |request| {
                history_started_tx.send(()).expect("history started");
                release_history_rx.recv().expect("history released");
                Ok(request)
            },
        )
    });
    history_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("history lookup started");

    let second_service = service.clone();
    let second_approved = Arc::clone(&approved);
    let second_directory = directory.path().to_path_buf();
    let (second_done_tx, second_done_rx) = mpsc::channel();
    let second = thread::spawn(move || {
        let second_request = CreateTerminalRequest {
            sign_in: false,
            id: second_id,
            cwd: second_directory.to_string_lossy().into_owned(),
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
            title: Some("Second".to_owned()),
            session_key: None,
            launch: None,
            launch_session: None,
            handoff: None,
            account: None,
        };
        let result = second_service.create(second_request, &second_approved);
        second_done_tx.send(result).expect("second result");
    });
    let unrelated = second_done_rx.recv_timeout(Duration::from_millis(500));
    release_history_tx.send(()).expect("release history");
    let first_created = first.join().expect("first thread").expect("first create");
    second.join().expect("second thread");
    let second_created = unrelated
        .expect("unrelated create was blocked by provider history")
        .expect("second create");

    for summary in [first_created, second_created] {
        service
            .close(TerminalGenerationRequest {
                id: summary.id,
                generation: summary.generation,
            })
            .expect("close terminal");
        await_state(&service, summary.id, TerminalState::Exited);
    }
}

#[test]
fn curated_agent_launch_is_host_resolved_idempotent_and_restart_stable() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let executable = directory.path().join("fixed-claude");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("fake agent");
    #[cfg(unix)]
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("agent permissions");
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        &executable,
        vec!["--fixed-host-argument".into()],
        vec![("PATH".into(), "/fixed/agent/path".into())],
    )
    .expect("launch program");
    let expected_launch = ResolvedTerminalLaunch::Program {
        executable: std::fs::canonicalize(&executable).expect("canonical agent"),
        arguments: vec!["--fixed-host-argument".into()],
        environment: vec![("PATH".into(), "/fixed/agent/path".into())],
    };
    let catalog = TerminalLaunchCatalog::new([program]).expect("launch catalog");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        catalog,
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    let created = service
        .create(launched.clone(), &approved)
        .expect("agent terminal");
    assert_eq!(created.launch, launched.launch);
    assert_eq!(lock(&backend.specs)[0].launch, expected_launch);
    assert_eq!(
        service
            .create(launched.clone(), &approved)
            .expect("idempotent replay"),
        created
    );
    assert_eq!(lock(&backend.specs).len(), 1);

    let mut conflicting = launched;
    conflicting.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Shell,
    });
    assert_eq!(
        service.create(conflicting, &approved),
        Err(TerminalError::CreateConflict(id))
    );

    backend.record(0).exit(0);
    let first = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    await_state(&service, id, TerminalState::Exited);
    let restarted = service.restart(first, &approved).expect("restart agent");
    assert_eq!(restarted.launch, created.launch);
    assert_eq!(lock(&backend.specs)[1].launch, expected_launch);
    let second = TerminalGenerationRequest {
        id,
        generation: restarted.generation,
    };
    service.close(second).expect("close restarted agent");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn claude_adopts_the_provider_session_before_persisting_or_restarting() {
    const REQUESTED: &str = "11111111-1111-4111-8111-111111111111";
    const PROVIDER: &str = "22222222-2222-4222-8222-222222222222";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let catalog = claude_capture_catalog(&directory);
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        catalog,
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(REQUESTED.to_owned()),
        resume: false,
    });

    let created = service
        .create(launched.clone(), &approved)
        .expect("fresh Claude terminal");
    assert_eq!(
        created.launch_session,
        Some(TerminalLaunchSessionV1 {
            version: 1,
            id: None,
            resume: false,
        }),
        "the caller's UUID must not be published as provider truth"
    );
    assert_eq!(
        claude_provider_arguments(&backend, 0),
        vec![OsString::from(format!("--session-id={REQUESTED}"))],
        "a fresh Claude terminal must stay in the checkout Interface selected"
    );

    let adopted = await_launch_session_id(&service, id, PROVIDER);
    assert_eq!(
        adopted.title, "Main shell",
        "the private OSC is not a tab title"
    );
    assert_eq!(
        adopted
            .launch_session
            .as_ref()
            .map(|binding| binding.resume),
        Some(true)
    );
    assert_eq!(
        service
            .create(launched.clone(), &approved)
            .expect("idempotent create replay"),
        adopted,
        "provider adoption must not change the original create fingerprint"
    );
    assert_eq!(lock(&backend.specs).len(), 1);
    assert!(lock(&events.0).iter().any(|event| matches!(
        event,
        TerminalLifecycleEvent::Upsert { terminal, .. }
            if terminal.launch_session.as_ref().and_then(|binding| binding.id.as_deref())
                == Some(PROVIDER)
    )));

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart adopted Claude session");
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from(format!("--resume={PROVIDER}"))],
        "restart must use Claude's provider-confirmed UUID"
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close restarted Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins promotion, persistence, exit, and shell rearm.
fn manual_claude_promotes_the_exact_foreground_child_then_rearms_the_shell() {
    const PROVIDER: &str = "32323232-3232-4232-8232-323232323232";

    let directory = temp_dir();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    backend.set_next_foreground_cwd(nested.clone());
    let events = Arc::new(GatedManualDemotionEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        events.clone(),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();

    let launched = request(id, &directory);
    let created = service
        .create(launched.clone(), &approved)
        .expect("capturing login shell");
    assert_eq!(created.launch, None);
    assert!(matches!(
        lock(&backend.specs)[0].launch,
        ResolvedTerminalLaunch::LoginShellWithEnvironment { .. }
    ));

    let promoted = await_launch_session_id(&service, id, PROVIDER);
    assert_eq!(
        promoted.launch,
        Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Claude,
        })
    );
    assert_eq!(promoted.cwd, nested.to_string_lossy());
    assert_eq!(
        service
            .create(launched, &approved)
            .expect("original create replay"),
        promoted,
        "provider promotion must not mutate the exact create fingerprint"
    );
    let owner = lock(&backend.provider_sessions.begun)[0].clone();
    assert!(owner.nested_shell);
    let recorded = lock(&backend.provider_sessions.recorded)[0].1.clone();
    assert_ne!(
        recorded,
        backend
            .record(0)
            .process_identity()
            .expect("root shell identity"),
        "the shell itself must never be accepted as the Claude process"
    );

    backend.record(0).exit_foreground_process();
    eventually("manual demotion persistence was not attempted", || {
        events.demotion_attempts.load(Ordering::SeqCst) != 0
    });
    let still_promoted = service
        .snapshot()
        .terminals
        .into_iter()
        .find(|terminal| terminal.id == id)
        .expect("promoted terminal");
    assert_eq!(still_promoted.launch, promoted.launch);
    assert_eq!(still_promoted.launch_session, promoted.launch_session);
    assert!(lock(&backend.provider_sessions.ended).is_empty());

    events.allow_demotion.store(true, Ordering::SeqCst);
    eventually("manual Claude did not return to shell state", || {
        let summary = summary_of(&service, id).expect("terminal summary");
        summary.launch.is_none() && summary.launch_session.is_none()
    });
    assert_eq!(lock(&backend.provider_sessions.begun).len(), 1);
    assert!(lock(&backend.provider_sessions.ended).is_empty());
    assert_eq!(
        lock(&backend.provider_sessions.rearmed)
            .iter()
            .map(|(token, _, _)| token.as_str())
            .collect::<Vec<_>>(),
        vec![owner.capture_token.as_str()]
    );

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn manual_codex_is_promoted_to_a_codex_pane_by_its_thread_id() {
    // A shell pane is armed for Claude, but the wrapper ran Codex: the receipt
    // carries a v7 thread id, and that alone picks the promoted profile.
    const THREAD: &str = "01a06e8d-c9ac-78e3-be73-19f2255b8932";

    let directory = temp_dir();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(THREAD));
    backend.set_next_foreground_cwd(nested.clone());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    assert_eq!(created.launch, None);

    let promoted = await_launch_session_id(&service, id, THREAD);
    assert_eq!(
        promoted.launch,
        Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Codex,
        })
    );
    assert_eq!(
        promoted.launch_session,
        Some(TerminalLaunchSessionV1 {
            version: 1,
            id: Some(THREAD.to_owned()),
            resume: true,
        })
    );
    assert_eq!(promoted.cwd, nested.to_string_lossy());

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn lost_wake_poll_ignores_an_unclaimed_foreground_job_then_adopts_the_wrapper_claim() {
    const PROVIDER: &str = "36363636-3636-4636-8636-363636363636";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let foreground = backend
        .record(0)
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("unrelated foreground job");
    let root = std::fs::canonicalize(directory.path()).expect("canonical root");

    eventually(
        "bounded recovery poll did not inspect durable ownership",
        || {
            backend
                .provider_sessions
                .capture_reads
                .load(Ordering::SeqCst)
                != 0
        },
    );
    let still_shell = service
        .snapshot()
        .terminals
        .into_iter()
        .find(|terminal| terminal.id == id)
        .expect("shell summary");
    assert_eq!(still_shell.launch, None);
    assert_eq!(still_shell.launch_session, None);
    assert!(
        lock(&backend.provider_sessions.current_reads).is_empty(),
        "an unclaimed foreground job must remain unattached"
    );
    assert!(
        lock(&backend.provider_sessions.recorded).is_empty(),
        "the terminal service must not manufacture wrapper ownership"
    );
    assert!(
        !lock(&backend.provider_sessions.captured).contains_key(&capture_token),
        "the durable nested owner must remain processless"
    );

    backend
        .provider_sessions
        .record_nested_process(
            TerminalLaunchProfileId::Claude,
            &capture_token,
            &foreground,
            &root,
        )
        .expect("wrapper-owned process claim");
    backend.provider_sessions.set(
        foreground.process_id,
        &capture_token,
        TerminalProviderSession {
            id: PROVIDER.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    let promoted = await_launch_session_id(&service, id, PROVIDER);
    assert_eq!(
        promoted.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude),
        "a bounded poll must discover a durable wrapper claim without an OSC wake"
    );

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn close_recovers_a_wrapper_owned_receipt_when_the_private_wake_is_lost() {
    const PROVIDER: &str = "37373737-3737-4737-8737-373737373737";

    let directory = temp_dir();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).expect("nested cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    backend.set_next_foreground_cwd(nested.clone());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let process = backend
        .record(0)
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("wrapper-owned process");
    backend
        .provider_sessions
        .set_captured(&capture_token, process.clone(), nested.clone());
    backend.provider_sessions.set_final(
        process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: PROVIDER.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close before OSC delivery");
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.cwd, nested.to_string_lossy());
    assert_eq!(
        exited.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
    assert_eq!(
        exited.launch_session.and_then(|session| session.id),
        Some(PROVIDER.to_owned())
    );
    assert_eq!(
        lock(&backend.provider_sessions.ended).as_slice(),
        &[capture_token],
        "receipt retirement must follow final persistence and checkpoint"
    );
}

#[test]
fn short_lived_manual_claude_without_a_wake_is_settled_and_rearmed() {
    const FIRST: &str = "41414141-4141-4141-8141-414141414141";
    const SECOND: &str = "42424242-4242-4242-8242-424242424242";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let record = backend.record(0);
    let first_process = record
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("first wrapper process");
    let root = std::fs::canonicalize(directory.path()).expect("canonical root");
    backend
        .provider_sessions
        .set_captured(&capture_token, first_process.clone(), root.clone());
    backend.provider_sessions.set_final(
        first_process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: FIRST.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    record.exit_foreground_process();

    eventually_within(
        Duration::from_secs(3),
        "lost-wake receipt was not settled before shell rearm",
        || {
            let checkpointed = lock(&backend.provider_sessions.checkpointed)
                .iter()
                .any(|(_, session)| session.id.as_deref() == Some(FIRST));
            checkpointed && !lock(&backend.provider_sessions.rearmed).is_empty()
        },
    );

    let second_process = TerminalProcessIdentity {
        process_id: first_process.process_id.saturating_add(1),
        process_group_id: first_process.process_group_id.saturating_add(1),
        terminal_device: first_process.terminal_device.clone(),
        process_start: "Mon Jan 1 00:03:00 2024".to_owned(),
    };
    record.replace_foreground_process(second_process.clone(), root.clone());
    backend
        .provider_sessions
        .set_captured(&capture_token, second_process.clone(), root);
    backend.provider_sessions.set(
        second_process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: SECOND.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    await_launch_session_id(&service, id, SECOND);
    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn immediate_manual_claude_relaunch_adopts_the_second_proven_process_without_another_wake() {
    const FIRST: &str = "32323232-3232-4232-8232-323232323232";
    const SECOND: &str = "34343434-3434-4434-8434-343434343434";

    let directory = temp_dir();
    let second_cwd = directory.path().join("second");
    std::fs::create_dir(&second_cwd).expect("second provider cwd");
    let second_cwd = std::fs::canonicalize(second_cwd).expect("canonical second cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(FIRST));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, FIRST);

    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let first_process = lock(&backend.provider_sessions.recorded)[0].1.clone();
    let second_process = TerminalProcessIdentity {
        process_id: first_process.process_id.saturating_add(1),
        process_group_id: first_process.process_id.saturating_add(1),
        terminal_device: first_process.terminal_device.clone(),
        process_start: "Mon Jan 1 00:02:00 2024".to_owned(),
    };
    backend
        .record(0)
        .replace_foreground_process(second_process.clone(), second_cwd.clone());
    backend
        .provider_sessions
        .record_nested_process(
            TerminalLaunchProfileId::Claude,
            &capture_token,
            &second_process,
            &second_cwd,
        )
        .expect("second wrapper-owned process claim");
    backend.provider_sessions.set(
        second_process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: SECOND.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    let promoted = await_launch_session_id(&service, id, SECOND);
    assert_eq!(promoted.cwd, second_cwd.to_string_lossy());
    assert_eq!(lock(&backend.provider_sessions.recorded).len(), 2);
    assert_eq!(
        lock(&backend.provider_sessions.recorded)[1].1,
        second_process
    );
    assert!(lock(&backend.provider_sessions.rearmed).is_empty());

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn durable_owner_replacement_during_settlement_is_reconciled() {
    const FIRST: &str = "43434343-4343-4343-8343-434343434343";
    const SECOND: &str = "44444444-4444-4444-8444-444444444444";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(FIRST));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, FIRST);
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let record = backend.record(0);
    let first_process = lock(&backend.provider_sessions.recorded)[0].1.clone();
    let second_process = TerminalProcessIdentity {
        process_id: first_process.process_id.saturating_add(1),
        process_group_id: first_process.process_group_id.saturating_add(1),
        terminal_device: first_process.terminal_device.clone(),
        process_start: "Mon Jan 1 00:04:00 2024".to_owned(),
    };
    let root = std::fs::canonicalize(directory.path()).expect("canonical root");
    record.exit_foreground_process();
    record.replace_foreground_process(second_process.clone(), root.clone());
    backend.provider_sessions.set(
        second_process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: SECOND.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    backend
        .provider_sessions
        .replace_capture_during_next_settled_lookup(TerminalProviderSessionCaptureProcess {
            process: second_process,
            cwd: root,
        });

    await_launch_session_id(&service, id, SECOND);
    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn retained_restart_uses_the_manual_claude_child_cwd() {
    const PROVIDER: &str = "35353535-3535-4535-8535-353535353535";

    let directory = temp_dir();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    backend.set_next_foreground_cwd(nested.clone());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, PROVIDER);

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart promoted Claude");
    assert_eq!(lock(&backend.specs)[1].cwd, nested);
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from(format!("--resume={PROVIDER}"))]
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close restarted Claude");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn manual_claude_demotion_waits_for_its_entire_foreground_process_group() {
    const PROVIDER: &str = "39393939-3939-4939-8939-393939393939";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let promoted = await_launch_session_id(&service, id, PROVIDER);
    let record = backend.record(0);

    record.exit_foreground_leader_only();
    thread::sleep(Duration::from_millis(200));
    let retained = service
        .snapshot()
        .terminals
        .into_iter()
        .find(|terminal| terminal.id == id)
        .expect("retained terminal");
    assert_eq!(retained.launch, promoted.launch);
    assert_eq!(retained.launch_session, promoted.launch_session);
    assert!(lock(&backend.provider_sessions.rearmed).is_empty());

    record.exit_foreground_process_group();
    eventually(
        "shell was not rearmed after the full provider group exited",
        || {
            let summary = summary_of(&service, id).expect("terminal summary");
            summary.launch.is_none() && summary.launch_session.is_none()
        },
    );

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn close_retires_a_processless_manual_capture_after_root_teardown() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close processless shell");
    await_state(&service, id, TerminalState::Exited);

    assert!(lock(&backend.provider_sessions.sealed).contains(&capture_token));
    assert_eq!(
        lock(&backend.provider_sessions.ended).as_slice(),
        &[capture_token]
    );
}

#[test]
fn ambiguous_manual_final_lookup_retains_terminal_state_and_lease() {
    const PROVIDER: &str = "40404040-4040-4040-8040-404040404040";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, PROVIDER);
    backend
        .provider_sessions
        .fail_final
        .store(true, Ordering::SeqCst);
    let record = backend.record(0);
    record.exit_foreground_process();
    record.exit(0);

    eventually("ambiguous final lookup was not attempted", || {
        backend.provider_sessions.final_reads.load(Ordering::SeqCst) != 0
    });
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == id)
            .expect("retained terminal")
            .state,
        TerminalState::Running
    );
    assert_eq!(leases.released.load(Ordering::SeqCst), 0);
    assert!(lock(&backend.provider_sessions.ended).is_empty());

    backend
        .provider_sessions
        .fail_final
        .store(false, Ordering::SeqCst);
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);
    assert_eq!(created.generation, TerminalGeneration::initial());
}

#[test]
fn root_shell_exit_waits_for_the_exact_nested_claude_process_before_finalizing() {
    const PROVIDER: &str = "38383838-3838-4838-8838-383838383838";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let promoted = await_launch_session_id(&service, id, PROVIDER);
    let record = backend.record(0);
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    record.keep_foreground_process_live_on_tree_teardown();

    record.exit(0);
    eventually("root-shell teardown did not complete", || {
        record.terminated.load(Ordering::SeqCst)
    });
    thread::sleep(Duration::from_millis(150));

    let still_running = service
        .snapshot()
        .terminals
        .into_iter()
        .find(|terminal| terminal.id == id)
        .expect("retained terminal");
    assert_eq!(still_running.state, TerminalState::Running);
    assert_eq!(still_running.launch, promoted.launch);
    assert_eq!(still_running.launch_session, promoted.launch_session);
    assert_eq!(leases.released.load(Ordering::SeqCst), 0);
    assert_eq!(
        backend.provider_sessions.final_reads.load(Ordering::SeqCst),
        0,
        "the root shell is not proof that its nested Claude child was reaped"
    );
    assert!(lock(&backend.provider_sessions.ended).is_empty());

    record.exit_foreground_process();
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.launch, promoted.launch);
    assert_eq!(exited.launch_session, promoted.launch_session);
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);
    assert_eq!(
        backend.provider_sessions.final_reads.load(Ordering::SeqCst),
        1
    );
    assert_eq!(
        lock(&backend.provider_sessions.ended).as_slice(),
        &[capture_token]
    );
    assert_eq!(created.generation, exited.generation);
}

#[test]
fn close_waits_for_a_durable_manual_attach_and_preserves_its_binding() {
    const PROVIDER: &str = "36363636-3636-4636-8636-363636363636";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let gate = Arc::new(Gate::default());
    backend
        .provider_sessions
        .gate_capture_process(Arc::clone(&gate));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    gate.wait_entered("nested capture record did not start");

    let closing_service = service.clone();
    let (closed_tx, closed_rx) = std::sync::mpsc::channel();
    let closer = thread::spawn(move || {
        let result = closing_service.close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        });
        closed_tx.send(()).expect("announce close");
        result
    });
    assert!(
        closed_rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "close must wait until the durable owner is installed in memory"
    );
    gate.release();
    closed_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("close completed");
    closer.join().expect("close thread").expect("close shell");

    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        exited
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(PROVIDER)
    );
    assert_eq!(
        exited.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins the wrapper-claim/close ordering through final filing.
fn close_preserves_a_wrapper_replacement_claimed_before_poll_adoption() {
    const FIRST: &str = "46464646-4646-4646-8646-464646464646";
    const SECOND: &str = "47474747-4747-4747-8747-474747474747";

    let directory = temp_dir();
    let replacement_cwd = directory.path().join("replacement");
    std::fs::create_dir(&replacement_cwd).expect("replacement cwd");
    let replacement_cwd =
        std::fs::canonicalize(replacement_cwd).expect("canonical replacement cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(FIRST));
    let events = Arc::new(DurableReopenEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, FIRST);
    eventually("first Claude binding was not durably acknowledged", || {
        !lock(&events.persisted).is_empty()
    });
    lock(&events.persisted).clear();

    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let first_process = backend
        .record(0)
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("first manual Claude process");
    let replacement = TerminalProcessIdentity {
        process_id: first_process.process_id.saturating_add(1),
        process_group_id: first_process.process_group_id.saturating_add(1),
        terminal_device: first_process.terminal_device.clone(),
        process_start: "Mon Jan 1 00:06:00 2024".to_owned(),
    };
    let gate = Arc::new(Gate::default());
    backend
        .provider_sessions
        .gate_capture_process(Arc::clone(&gate));
    let closing_service = service.clone();
    let generation = created.generation;
    let closer =
        thread::spawn(move || closing_service.close(TerminalGenerationRequest { id, generation }));
    gate.wait_entered("nested capture record did not start");

    backend
        .record(0)
        .replace_foreground_process(replacement.clone(), replacement_cwd.clone());
    backend.provider_sessions.set_captured(
        &capture_token,
        replacement.clone(),
        replacement_cwd.clone(),
    );
    backend.provider_sessions.set_final(
        replacement.process_id,
        &capture_token,
        TerminalProviderSession {
            id: SECOND.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    assert!(
        !lock(&backend.provider_sessions.current_reads).contains(&replacement.process_id),
        "the live poll must not adopt B before shutdown seals it"
    );
    gate.release();
    closer.join().expect("close thread").expect("close shell");

    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.cwd, replacement_cwd.to_string_lossy());
    assert_eq!(
        exited.launch_session.and_then(|session| session.id),
        Some(SECOND.to_owned()),
        "B's Startup receipt must supersede the resumable A binding"
    );
    assert_eq!(
        lock(&backend.provider_sessions.checkpointed)
            .last()
            .and_then(|(_, session)| session.id.as_deref()),
        Some(SECOND)
    );
    assert!(lock(&backend.provider_sessions.ended).contains(&capture_token));
    let persisted = lock(&events.persisted);
    let final_reopen = persisted.last().expect("replacement reopen tuple");
    assert_eq!(final_reopen.cwd, replacement_cwd.to_string_lossy());
    assert_eq!(
        final_reopen
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref()),
        Some(SECOND)
    );
}

#[test]
fn close_cannot_turn_its_own_manual_claude_kill_into_a_shell_demotion() {
    const PROVIDER: &str = "37373737-3737-4737-8737-373737373737";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, PROVIDER);

    let liveness_gate = Arc::new(Gate::default());
    backend
        .record(0)
        .gate_foreground_liveness(Arc::clone(&liveness_gate));
    liveness_gate.wait_entered("provider liveness check did not start");

    let closing_service = service.clone();
    let (closed_tx, closed_rx) = mpsc::channel();
    let closer = thread::spawn(move || {
        let result = closing_service.close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        });
        closed_tx.send(()).expect("announce close");
        result
    });
    assert!(
        closed_rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "close must serialize with the manual-provider liveness decision"
    );
    liveness_gate.release();
    closed_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("close completed");
    closer.join().expect("close thread").expect("close shell");

    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        exited
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(PROVIDER)
    );
    assert_eq!(
        exited.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
    assert!(lock(&backend.provider_sessions.rearmed).is_empty());
}

#[test]
fn manual_claude_outside_the_retained_root_remains_an_uncaptured_shell() {
    const PROVIDER: &str = "45454545-4545-4545-8545-454545454545";

    let directory = temp_dir();
    let outside = temp_dir();
    let outside_cwd = std::fs::canonicalize(outside.path()).expect("canonical outside cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    backend.set_next_foreground_cwd(outside_cwd);
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("terminal output");
    assert_eq!(
        output.read_timeout(Duration::from_secs(1)),
        TerminalOutputRead::Closed
    );
    thread::sleep(Duration::from_millis(150));

    let summary = service
        .snapshot()
        .terminals
        .into_iter()
        .find(|terminal| terminal.id == id)
        .expect("shell summary");
    assert_eq!(summary.launch, None);
    assert_eq!(summary.launch_session, None);
    assert!(lock(&backend.provider_sessions.recorded).is_empty());

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn claude_recovery_picker_adopts_the_selected_provider_session() {
    const PROVIDER: &str = "33333333-3333-4333-8333-333333333333";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let catalog = claude_capture_catalog(&directory);
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        catalog,
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: None,
        resume: true,
    });

    let created = service
        .create(launched.clone(), &approved)
        .expect("Claude recovery picker");
    assert_eq!(
        created.launch_session, launched.launch_session,
        "a cancelled picker must remain retryable"
    );
    assert_eq!(
        claude_provider_arguments(&backend, 0),
        vec![OsString::from("--resume")]
    );

    let adopted = await_launch_session_id(&service, id, PROVIDER);
    assert!(
        adopted
            .launch_session
            .as_ref()
            .is_some_and(|binding| { binding.resume && binding.id.as_deref() == Some(PROVIDER) })
    );
    assert_eq!(
        service
            .create(launched, &approved)
            .expect("idempotent picker replay"),
        adopted
    );
    assert_eq!(lock(&backend.specs).len(), 1);

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart selected Claude session");
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from(format!("--resume={PROVIDER}"))]
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close recovered Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn provisional_startup_cannot_replace_an_exact_resume_binding() {
    const SAVED: &str = "12121212-1212-4212-8212-121212121212";
    const PROVISIONAL: &str = "23232323-2323-4232-8232-232323232323";
    const RESUMED: &str = "34343434-3434-4343-8343-343434343434";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(SAVED.to_owned()),
        resume: true,
    });
    let created = service.create(launched, &approved).expect("Claude resume");
    let token = claude_capture_token(&backend, 0);

    backend.provider_sessions.set(
        10_000,
        &token,
        TerminalProviderSession {
            id: PROVISIONAL.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    thread::sleep(Duration::from_millis(350));
    assert_eq!(
        service.snapshot().terminals[0]
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(SAVED)
    );
    assert!(!lock(&events.0).iter().any(|event| matches!(
        event,
        TerminalLifecycleEvent::Upsert { terminal, .. }
            if terminal.launch_session.as_ref().and_then(|binding| binding.id.as_deref())
                == Some(PROVISIONAL)
    )));

    backend.provider_sessions.set(
        10_000,
        &token,
        TerminalProviderSession {
            id: RESUMED.to_owned(),
            transition: TerminalProviderSessionTransition::Resume,
        },
    );
    await_launch_session_id(&service, id, RESUMED);

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn reaped_process_final_receipt_is_persisted_before_exit() {
    const REQUESTED: &str = "45454545-4545-4454-8454-454545454545";
    const DURABLE: &str = "56565656-5656-4565-8565-565656565656";

    let directory = temp_dir();
    std::fs::create_dir(directory.path().join(".git")).expect("repository marker");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(REQUESTED.to_owned()),
        resume: false,
    });
    let created = service.create(launched, &approved).expect("fresh Claude");
    let token = claude_capture_token(&backend, 0);
    backend.provider_sessions.set_final(
        10_000,
        &token,
        TerminalProviderSession {
            id: DURABLE.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    backend.record(0).exit(0);
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        exited
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(DURABLE)
    );
    assert_eq!(exited.generation, created.generation);
    assert_eq!(lock(&backend.provider_sessions.ended).as_slice(), &[token]);
}

#[test]
fn reaped_receipt_is_not_retired_without_a_durable_persistence_ack() {
    const REQUESTED: &str = "90909090-9090-4090-8090-909090909090";
    const DURABLE: &str = "91919191-9191-4191-8191-919191919191";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let events = Arc::new(RefusingPersistenceEvents::default());
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> =
        backend.provider_sessions.clone();
    let service = TerminalService::new(TerminalServiceParts {
        launches: claude_capture_catalog(&directory),
        provider_sessions,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            backend.clone(),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            events.clone(),
        )
    })
    .expect("terminal service");
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(REQUESTED.to_owned()),
        resume: false,
    });
    let created = service.create(launched, &approved).expect("fresh Claude");
    let token = claude_capture_token(&backend, 0);
    backend.provider_sessions.set_final(
        10_000,
        &token,
        TerminalProviderSession {
            id: DURABLE.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    backend.record(0).exit(0);
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        exited
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(DURABLE)
    );
    assert_eq!(exited.generation, created.generation);
    assert!(lock(&backend.provider_sessions.ended).is_empty());
    assert!(lock(&events.0).iter().any(|event| matches!(
        event,
        TerminalLifecycleEvent::Upsert { terminal, .. }
            if terminal.launch_session.as_ref().and_then(|binding| binding.id.as_deref())
                == Some(DURABLE)
    )));

    drop(service);
    assert!(
        lock(&backend.provider_sessions.ended).is_empty(),
        "final Shared destruction must preserve the receipt when the host still refuses durability"
    );
}

#[test]
fn restart_preflight_retries_persistence_before_spawning_a_new_generation() {
    const PRIOR: &str = "92929292-9292-4292-8292-929292929292";
    const RECOVERED: &str = "93939393-9393-4393-8393-939393939393";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let events = Arc::new(FailOncePersistenceEvents::default());
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> =
        backend.provider_sessions.clone();
    let service = TerminalService::new(TerminalServiceParts {
        launches: claude_capture_catalog(&directory),
        provider_sessions,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            backend.clone(),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            events.clone(),
        )
    })
    .expect("terminal service");
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(PRIOR.to_owned()),
        resume: true,
    });
    let created = service.create(launched, &approved).expect("Claude resume");
    let generation = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let token = claude_capture_token(&backend, 0);
    backend.provider_sessions.set_final(
        10_000,
        &token,
        TerminalProviderSession {
            id: RECOVERED.to_owned(),
            transition: TerminalProviderSessionTransition::Resume,
        },
    );

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(events.attempts.load(Ordering::SeqCst), 1);
    assert!(lock(&backend.provider_sessions.ended).is_empty());
    assert_eq!(
        service.restart(generation, &approved),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        )),
        "restart must refuse an unsettled prior capture"
    );
    assert_eq!(lock(&backend.specs).len(), 1);

    service
        .settle_provider_capture_for_restart(generation)
        .expect("retry durable capture settlement");
    assert_eq!(events.attempts.load(Ordering::SeqCst), 2);
    assert!(lock(&backend.provider_sessions.checkpointed).iter().any(
        |(observed_token, session)| {
            observed_token == &token && session.id.as_deref() == Some(RECOVERED)
        }
    ));
    assert_eq!(lock(&backend.provider_sessions.ended).as_slice(), &[token]);
    let restarted = service
        .restart(generation, &approved)
        .expect("restart after preflight");
    assert_eq!(lock(&backend.specs).len(), 2);
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close restarted terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn restart_preflight_retries_capture_retirement_before_spawning() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let generation = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let token = claude_capture_token(&backend, 0);
    backend
        .provider_sessions
        .fail_end_remaining
        .store(1, Ordering::SeqCst);

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    assert!(lock(&backend.provider_sessions.ended).is_empty());
    assert!(matches!(
        service.restart(generation, &approved),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        ))
    ));
    assert_eq!(lock(&backend.specs).len(), 1);

    service
        .settle_provider_capture_for_restart(generation)
        .expect("retry capture retirement");
    assert_eq!(lock(&backend.provider_sessions.ended).as_slice(), &[token]);
    let restarted = service
        .restart(generation, &approved)
        .expect("restart after capture retirement");
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close restarted terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn dismiss_and_abandon_preserve_a_capture_when_retirement_fails() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let generation = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    backend
        .provider_sessions
        .fail_end
        .store(true, Ordering::SeqCst);
    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);

    assert!(matches!(
        service.dismiss_generation(generation),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        ))
    ));
    assert_eq!(service.snapshot().terminals.len(), 1);
    assert!(matches!(
        service.abandon_generation(generation),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        ))
    ));
    assert_eq!(service.snapshot().terminals.len(), 1);

    backend
        .provider_sessions
        .fail_end
        .store(false, Ordering::SeqCst);
    assert!(
        service
            .abandon_generation(generation)
            .expect("retry abandon")
            .removed
    );
    assert!(service.snapshot().terminals.is_empty());
}

#[test]
fn stale_live_lookup_cannot_overwrite_the_final_reaped_binding() {
    const REQUESTED: &str = "67676767-6767-4676-8676-676767676767";
    const STALE: &str = "78787878-7878-4787-8787-787878787878";
    const FINAL: &str = "89898989-8989-4898-8989-898989898989";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let lookup = Arc::new(RacingProviderSessions {
        live_lookup: Gate::default(),
        stale: TerminalProviderSession {
            id: STALE.to_owned(),
            transition: TerminalProviderSessionTransition::Clear,
        },
        final_session: TerminalProviderSession {
            id: FINAL.to_owned(),
            transition: TerminalProviderSessionTransition::Clear,
        },
    });
    let provider_sessions: Arc<dyn TerminalProviderSessionLookup> = lookup.clone();
    let service = TerminalService::new(TerminalServiceParts {
        launches: claude_capture_catalog(&directory),
        provider_sessions,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            backend.clone(),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            Arc::new(Events::default()),
        )
    })
    .expect("terminal service");
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(REQUESTED.to_owned()),
        resume: false,
    });
    service.create(launched, &approved).expect("fresh Claude");
    lookup
        .live_lookup
        .wait_entered("live provider lookup did not start");

    backend.record(0).exit(0);
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        exited
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(FINAL)
    );

    lookup.live_lookup.release();
    thread::sleep(Duration::from_millis(150));
    let stable = service.snapshot().terminals.remove(0);
    assert_eq!(stable.state, TerminalState::Exited);
    assert_eq!(
        stable
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(FINAL)
    );
}

#[test]
fn claude_binding_wake_channel_is_generation_scoped_and_private() {
    const FIRST: &str = "44444444-4444-4444-8444-444444444444";
    const SECOND: &str = "55555555-5555-4555-8555-555555555555";
    const FORGED: &str = "66666666-6666-4666-8666-666666666666";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).push_back(FakePlan {
        output: b"before".to_vec(),
        claude_provider_sessions: vec![
            FIRST.to_owned(),
            FIRST.to_owned(),
            SECOND.to_owned(),
            SECOND.to_owned(),
        ],
        claude_provider_steps: Vec::new(),
        wrong_token_provider_session: Some(FORGED.to_owned()),
        previous_token_provider_session: None,
        output_suffix: b"after".to_vec(),
        fail_spawn: false,
    });
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("77777777-7777-4777-8777-777777777777".to_owned()),
        resume: false,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let adopted = await_launch_session_id(&service, id, SECOND);
    assert_eq!(adopted.title, created.title);

    let binding_ids = lock(&events.0)
        .iter()
        .filter_map(|event| match event {
            TerminalLifecycleEvent::Upsert { terminal, .. } => terminal
                .launch_session
                .as_ref()
                .and_then(|binding| binding.id.clone()),
            TerminalLifecycleEvent::Removed { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(binding_ids.last().map(String::as_str), Some(SECOND));
    assert!(
        binding_ids
            .iter()
            .all(|binding| matches!(binding.as_str(), FIRST | SECOND)),
        "the coalescing lookup worker may skip intermediate writes but must only publish verified provider state"
    );
    assert!(!binding_ids.iter().any(|id| id == FORGED));

    let reader = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("terminal output");
    let mut visible = Vec::new();
    loop {
        match reader.read_timeout(Duration::from_millis(200)) {
            TerminalOutputRead::Chunk(chunk) => visible.extend_from_slice(&chunk.bytes),
            TerminalOutputRead::Closed => break,
            TerminalOutputRead::TimedOut => panic!("terminal output did not close"),
        }
    }
    assert_eq!(visible, b"beforeafter");
    let token = claude_capture_token(&backend, 0);
    assert!(!String::from_utf8_lossy(&visible).contains(&token));
    assert!(!String::from_utf8_lossy(&visible).contains(CLAUDE_SESSION_BINDING_TITLE_PREFIX));

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn claude_lookup_worker_converges_on_the_latest_verified_transition() {
    const FIRST: &str = "10101010-1010-4010-8010-101010101010";
    const SECOND: &str = "20202020-2020-4020-8020-202020202020";
    const THIRD: &str = "30303030-3030-4030-8030-303030303030";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).push_back(FakePlan {
        output: Vec::new(),
        claude_provider_sessions: vec![FIRST.to_owned()],
        claude_provider_steps: vec![
            // The hook for B runs before Claude's asynchronous pointer write.
            FakeProviderStep::Wake {
                source: "clear",
                expected_session: SECOND.to_owned(),
                live_session: Some(FIRST.to_owned()),
            },
            // C is already the latest transition when the queued B write
            // lands; B must never become durable Interface state.
            FakeProviderStep::Wake {
                source: "resume",
                expected_session: THIRD.to_owned(),
                live_session: Some(SECOND.to_owned()),
            },
            FakeProviderStep::Live(THIRD.to_owned()),
        ],
        wrong_token_provider_session: None,
        previous_token_provider_session: None,
        output_suffix: Vec::new(),
        fail_spawn: false,
    });
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: None,
        resume: true,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    await_launch_session_id(&service, id, THIRD);

    let observed = lock(&events.0)
        .iter()
        .filter_map(|event| match event {
            TerminalLifecycleEvent::Upsert { terminal, .. } => terminal
                .launch_session
                .as_ref()
                .and_then(|session| session.id.clone()),
            TerminalLifecycleEvent::Removed { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(observed.last().map(String::as_str), Some(THIRD));
    assert!(
        observed
            .iter()
            .all(|session| matches!(session.as_str(), FIRST | SECOND | THIRD))
    );

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart latest Claude session");
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from(format!("--resume={THIRD}"))]
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close restarted Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn untrusted_transition_wakes_do_not_clear_a_verified_binding() {
    const FIRST: &str = "40404040-4040-4040-8040-404040404040";
    const SECOND: &str = "50505050-5050-4050-8050-505050505050";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).push_back(FakePlan {
        output: Vec::new(),
        claude_provider_sessions: vec![FIRST.to_owned()],
        claude_provider_steps: vec![
            FakeProviderStep::Wake {
                source: "clear",
                expected_session: SECOND.to_owned(),
                live_session: Some(FIRST.to_owned()),
            },
            // B is never observed. A newer wake returns to A, which the same
            // live provider metadata independently corroborates.
            FakeProviderStep::Wake {
                source: "resume",
                expected_session: FIRST.to_owned(),
                live_session: Some(FIRST.to_owned()),
            },
        ],
        wrong_token_provider_session: None,
        previous_token_provider_session: None,
        output_suffix: Vec::new(),
        fail_spawn: false,
    });
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: None,
        resume: true,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("terminal output");
    assert_eq!(
        output.read_timeout(Duration::from_secs(1)),
        TerminalOutputRead::Closed
    );
    await_launch_session_id(&service, id, FIRST);
    let binding_states = lock(&events.0)
        .iter()
        .filter_map(|event| match event {
            TerminalLifecycleEvent::Upsert { terminal, .. } => Some(
                terminal
                    .launch_session
                    .as_ref()
                    .and_then(|session| session.id.clone()),
            ),
            TerminalLifecycleEvent::Removed { .. } => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(binding_states.last(), Some(&Some(FIRST.to_owned())));
    assert_eq!(
        binding_states
            .iter()
            .filter(|state| state.is_none())
            .count(),
        1,
        "only the initial unconfirmed lifecycle state may be id-less"
    );

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn uncorroborated_transition_marker_cannot_replace_the_confirmed_binding() {
    const FIRST: &str = "60606060-6060-4060-8060-606060606060";
    const SECOND: &str = "70707070-7070-4070-8070-707070707070";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).push_back(FakePlan {
        output: Vec::new(),
        claude_provider_sessions: vec![FIRST.to_owned()],
        claude_provider_steps: vec![FakeProviderStep::Wake {
            source: "clear",
            expected_session: SECOND.to_owned(),
            live_session: Some(FIRST.to_owned()),
        }],
        wrong_token_provider_session: None,
        previous_token_provider_session: None,
        output_suffix: Vec::new(),
        fail_spawn: false,
    });
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: None,
        resume: true,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("terminal output");
    assert_eq!(
        output.read_timeout(Duration::from_secs(1)),
        TerminalOutputRead::Closed
    );
    await_launch_session_id(&service, id, FIRST);

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart confirmed Claude session");
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from(format!("--resume={FIRST}"))]
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close picker");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
#[allow(clippy::too_many_lines)] // One test pins two generations and both transition fences.
fn claude_restart_rotates_wake_correlation_and_uses_latest_verified_transition() {
    const FIRST: &str = "88888888-8888-4888-8888-888888888888";
    const SECOND: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const THIRD: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    const FORGED: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).extend([
        FakePlan {
            output: Vec::new(),
            claude_provider_sessions: vec![FIRST.to_owned(), SECOND.to_owned()],
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: None,
            output_suffix: Vec::new(),
            fail_spawn: false,
        },
        FakePlan {
            output: Vec::new(),
            claude_provider_sessions: vec![SECOND.to_owned(), THIRD.to_owned()],
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: Some(FORGED.to_owned()),
            output_suffix: Vec::new(),
            fail_spawn: false,
        },
    ]);
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("dddddddd-dddd-4ddd-8ddd-dddddddddddd".to_owned()),
        resume: false,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    await_launch_session_id(&service, id, SECOND);
    let first_token = claude_capture_token(&backend, 0);

    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart Claude terminal");
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from(format!("--resume={SECOND}"))]
    );
    let second_token = claude_capture_token(&backend, 1);
    assert_ne!(first_token, second_token);
    let transitioned = await_launch_session_id(&service, id, THIRD);
    assert_eq!(
        transitioned
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(THIRD)
    );
    assert_ne!(
        transitioned
            .launch_session
            .as_ref()
            .and_then(|binding| binding.id.as_deref()),
        Some(FORGED)
    );

    backend.record(1).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let third = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: restarted.generation,
            },
            &approved,
        )
        .expect("second Claude restart");
    assert_eq!(
        claude_provider_arguments(&backend, 2),
        vec![OsString::from(format!("--resume={THIRD}"))]
    );
    assert_ne!(second_token, claude_capture_token(&backend, 2));
    service
        .close(TerminalGenerationRequest {
            id,
            generation: third.generation,
        })
        .expect("close Claude terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
#[allow(clippy::too_many_lines)] // One test pins rollback fencing, publication, and retry argv.
fn failed_restart_can_restore_its_prior_provider_binding_before_retry() {
    const PRIOR: &str = "14141414-1414-4141-8141-141414141414";
    const FAILED: &str = "25252525-2525-4252-8252-252525252525";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).extend([
        FakePlan {
            output: Vec::new(),
            claude_provider_sessions: vec![PRIOR.to_owned()],
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: None,
            output_suffix: Vec::new(),
            fail_spawn: false,
        },
        FakePlan {
            output: Vec::new(),
            claude_provider_sessions: vec![FAILED.to_owned()],
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: None,
            output_suffix: Vec::new(),
            fail_spawn: false,
        },
    ]);
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("03030303-0303-4303-8303-030303030303".to_owned()),
        resume: false,
    });

    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let prior = await_launch_session_id(&service, id, PRIOR)
        .launch_session
        .expect("provider binding");
    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);

    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart Claude terminal");
    await_launch_session_id(&service, id, FAILED);
    backend.record(1).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let revision_before_restore = service.snapshot().revision;

    assert!(matches!(
        service.restore_settled_provider_session_binding(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            Some(prior.clone()),
        ),
        Err(TerminalError::StaleGeneration { .. })
    ));
    let restored = service
        .restore_settled_provider_session_binding(
            TerminalGenerationRequest {
                id,
                generation: restarted.generation,
            },
            Some(prior.clone()),
        )
        .expect("restore pre-restart binding");
    assert_eq!(restored.launch_session, Some(prior.clone()));
    assert_eq!(restored.state, TerminalState::Exited);
    assert_eq!(service.snapshot().revision, revision_before_restore + 1);
    assert!(matches!(
        lock(&events.0).last(),
        Some(TerminalLifecycleEvent::Upsert { terminal, .. })
            if terminal.generation == restarted.generation
                && terminal.launch_session.as_ref() == Some(&prior)
    ));

    let retried = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: restarted.generation,
            },
            &approved,
        )
        .expect("retry with restored binding");
    assert_eq!(
        claude_provider_arguments(&backend, 2),
        vec![OsString::from(format!("--resume={PRIOR}"))]
    );
    assert!(matches!(
        service.restore_settled_provider_session_binding(
            TerminalGenerationRequest {
                id,
                generation: retried.generation,
            },
            None,
        ),
        Err(TerminalError::StillRunning(current)) if current == id
    ));
    service
        .close(TerminalGenerationRequest {
            id,
            generation: retried.generation,
        })
        .expect("close retry");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn failed_restart_rollback_retries_retirement_and_drop_cannot_reapply_it() {
    const PRIOR: &str = "36363636-3636-4636-8636-363636363636";
    const FAILED: &str = "47474747-4747-4747-8747-474747474747";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let prior = TerminalLaunchSessionV1 {
        version: 1,
        id: Some(PRIOR.to_owned()),
        resume: true,
    };
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(prior.clone());
    let created = service.create(launched, &approved).expect("Claude resume");
    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("restart Claude terminal");
    let failed_generation = TerminalGenerationRequest {
        id,
        generation: restarted.generation,
    };
    let failed_token = claude_capture_token(&backend, 1);
    backend.provider_sessions.set_final(
        10_001,
        &failed_token,
        TerminalProviderSession {
            id: FAILED.to_owned(),
            transition: TerminalProviderSessionTransition::Clear,
        },
    );
    backend
        .provider_sessions
        .fail_end
        .store(true, Ordering::SeqCst);
    backend.record(1).exit(1);
    let failed = await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        failed
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref()),
        Some(FAILED)
    );

    assert!(matches!(
        service.restore_settled_provider_session_binding(failed_generation, Some(prior.clone())),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Claude
        ))
    ));
    assert_eq!(
        service.snapshot().terminals[0]
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref()),
        Some(FAILED),
        "failed retirement must leave rollback retryable and the capture attached"
    );

    backend
        .provider_sessions
        .fail_end
        .store(false, Ordering::SeqCst);
    let restored = service
        .restore_settled_provider_session_binding(failed_generation, Some(prior.clone()))
        .expect("retry rollback after capture retirement recovers");
    assert_eq!(restored.launch_session, Some(prior));
    let ended_before_drop = lock(&backend.provider_sessions.ended).clone();
    assert!(ended_before_drop.contains(&failed_token));

    drop(service);
    assert_eq!(
        *lock(&backend.provider_sessions.ended),
        ended_before_drop,
        "Shared::drop must not revisit an aborted generation after rollback cleared its capture"
    );
}

#[test]
fn unconfirmed_fresh_claude_restart_uses_the_recovery_picker() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee".to_owned()),
        resume: false,
    });
    let created = service
        .create(launched, &approved)
        .expect("Claude terminal");
    let first_token = claude_capture_token(&backend, 0);
    backend.record(0).exit(0);
    await_state(&service, id, TerminalState::Exited);

    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &approved,
        )
        .expect("picker restart");
    assert_eq!(
        claude_provider_arguments(&backend, 1),
        vec![OsString::from("--resume")]
    );
    assert_ne!(first_token, claude_capture_token(&backend, 1));
    assert_eq!(
        restarted.launch_session,
        Some(TerminalLaunchSessionV1 {
            version: 1,
            id: None,
            resume: true,
        })
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: restarted.generation,
        })
        .expect("close picker");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn codex_reopen_only_takes_the_last_thread_when_no_live_pane_shares_the_cwd() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let executable = directory.path().join("fixed-codex");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("fake codex");
    #[cfg(unix)]
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("codex permissions");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        &executable,
        Vec::new(),
        Vec::new(),
    )
    .expect("launch program")])
    .expect("launch catalog");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        catalog,
    );

    let first_id = TerminalId::random();
    let second_id = TerminalId::random();
    let mut first_request = request(first_id, &directory);
    first_request.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Codex,
    });
    first_request.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: None,
        resume: true,
    });
    let second_request = CreateTerminalRequest {
        id: second_id,
        ..first_request.clone()
    };

    let first = service
        .create(first_request, &approved)
        .expect("first codex pane");
    let second = service
        .create(second_request, &approved)
        .expect("second codex pane");
    assert_eq!(
        spawn_arguments(&backend, 0),
        vec![OsString::from("resume"), OsString::from("--last")]
    );
    assert_eq!(spawn_arguments(&backend, 1), vec![OsString::from("resume")]);

    // A restart beside a live Codex pane must not race it for that folder's
    // newest thread — the loser dies on the provider's writer lock.
    backend.record(0).exit(0);
    await_state(&service, first_id, TerminalState::Exited);
    let restarted = service
        .restart(
            TerminalGenerationRequest {
                id: first_id,
                generation: first.generation,
            },
            &approved,
        )
        .expect("restart beside a live pane");
    assert_eq!(spawn_arguments(&backend, 2), vec![OsString::from("resume")]);

    // Alone in the folder again, the reopen may take the newest thread.
    service
        .close(TerminalGenerationRequest {
            id: second_id,
            generation: second.generation,
        })
        .expect("close the second pane");
    await_state(&service, second_id, TerminalState::Exited);
    service
        .close(TerminalGenerationRequest {
            id: first_id,
            generation: restarted.generation,
        })
        .expect("close the restarted pane");
    await_state(&service, first_id, TerminalState::Exited);
    service
        .restart(
            TerminalGenerationRequest {
                id: first_id,
                generation: restarted.generation,
            },
            &approved,
        )
        .expect("restart alone in the folder");
    assert_eq!(
        spawn_arguments(&backend, 3),
        vec![OsString::from("resume"), OsString::from("--last")]
    );
}

#[test]
fn unavailable_or_unversioned_agent_is_refused_before_lease_or_spawn() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        TerminalLaunchCatalog::shell_only(),
    );
    let mut unavailable = request(TerminalId::random(), &directory);
    unavailable.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Gemini,
    });
    assert_eq!(
        service.create(unavailable, &approved),
        Err(TerminalError::UnavailableLaunchProfile(
            TerminalLaunchProfileId::Gemini
        ))
    );
    let mut unsupported = request(TerminalId::random(), &directory);
    unsupported.launch = Some(TerminalLaunchRequestV1 {
        version: 2,
        profile_id: TerminalLaunchProfileId::Shell,
    });
    assert_eq!(
        service.create(unsupported, &approved),
        Err(TerminalError::UnsupportedLaunchVersion(2))
    );
    assert!(lock(&backend.specs).is_empty());
    assert_eq!(leases.acquired.load(Ordering::SeqCst), 0);
}

#[test]
fn output_is_ordered_bounded_and_single_consumer() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_output(b"abcdefgh".to_vec()));
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig {
            max_terminals: 4,
            output_capacity_bytes: 4,
            output_read_chunk_bytes: 4,
        },
    );
    let id = TerminalId::random();
    let summary = service
        .create(request(id, &directory), &approved)
        .expect("create");
    let key = TerminalGenerationRequest {
        id,
        generation: summary.generation,
    };
    let output = service.take_output(key).expect("output reader");
    assert!(matches!(
        service.take_output(key),
        Err(TerminalError::OutputAlreadyClaimed(error_id)) if error_id == id
    ));

    let first = output.read_timeout(Duration::from_secs(1));
    let second = output.read_timeout(Duration::from_secs(1));
    assert_eq!(
        first,
        TerminalOutputRead::Chunk(interface_terminal::TerminalOutputChunk {
            id,
            generation: summary.generation,
            sequence: 0,
            bytes: b"abcd".to_vec(),
        })
    );
    assert_eq!(
        second,
        TerminalOutputRead::Chunk(interface_terminal::TerminalOutputChunk {
            id,
            generation: summary.generation,
            sequence: 1,
            bytes: b"efgh".to_vec(),
        })
    );
    assert_eq!(
        output.read_timeout(Duration::from_secs(1)),
        TerminalOutputRead::Closed
    );
    drop(output);
    assert!(service.take_output(key).is_ok());
}

#[test]
fn split_osc_candidate_cannot_create_an_oversized_backpressure_chunk() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_output(b"aaa\x1bXbbb".to_vec()));
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig {
            max_terminals: 4,
            output_capacity_bytes: 4,
            output_read_chunk_bytes: 4,
        },
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("terminal output");
    let mut visible = Vec::new();
    loop {
        match output.read_timeout(Duration::from_secs(1)) {
            TerminalOutputRead::Chunk(chunk) => visible.extend_from_slice(&chunk.bytes),
            TerminalOutputRead::Closed => break,
            TerminalOutputRead::TimedOut => panic!("split OSC output deadlocked"),
        }
    }
    assert_eq!(visible, b"aaa\x1bXbbb");

    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close terminal");
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn fast_process_exit_preserves_visible_output_tail_for_an_attached_reader() {
    const TAIL: &[u8] = b"final-visible-output";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(DelayedTailBackend::new(TAIL));
    let leases = Arc::new(LeaseCounts::default());
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig {
            max_terminals: 4,
            output_capacity_bytes: 4,
            output_read_chunk_bytes: 4,
        },
        backend.clone(),
        Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("attached output reader");
    backend.delay.wait_entered("tail reader did not start");
    backend.record.exit(0);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        service.snapshot().terminals[0].state,
        TerminalState::Running,
        "Exited must wait for the reader's final bytes"
    );

    backend.delay.release();
    let mut visible = Vec::new();
    loop {
        match output.read_timeout(Duration::from_secs(1)) {
            TerminalOutputRead::Chunk(chunk) => visible.extend_from_slice(&chunk.bytes),
            TerminalOutputRead::Closed => break,
            TerminalOutputRead::TimedOut => panic!("final output tail did not drain"),
        }
    }
    assert_eq!(visible, TAIL);
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);
}

#[test]
fn fast_process_exit_releases_an_unclaimed_backpressured_output_queue() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(SaturatingOutputBackend::new());
    let leases = Arc::new(LeaseCounts::default());
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig {
            max_terminals: 4,
            output_capacity_bytes: 4,
            output_read_chunk_bytes: 4,
        },
        backend.clone(),
        Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("create");
    backend
        .lifecycle
        .reads
        .wait_entered_times(2, "PTY reader did not fill its queue");
    backend.record.exit(0);

    await_state(&service, id, TerminalState::Exited);
    backend
        .lifecycle
        .dropped
        .wait_entered("explicit close left the PTY reader blocked");
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);
}

#[test]
fn a_late_output_claim_prevents_the_unclaimed_grace_close() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(SaturatingOutputBackend::new());
    let leases = Arc::new(LeaseCounts::default());
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig {
            max_terminals: 4,
            output_capacity_bytes: 4,
            output_read_chunk_bytes: 4,
        },
        backend.clone(),
        Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    backend
        .lifecycle
        .reads
        .wait_entered_times(2, "PTY reader did not fill its queue");
    backend.record.exit(0);

    // Claim after process exit and while its reader is blocked on the full
    // queue. Once claimed, the grace timer must not seal the output tail.
    thread::sleep(Duration::from_millis(100));
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("late output reader");
    thread::sleep(Duration::from_millis(550));
    assert_eq!(
        service.snapshot().terminals[0].state,
        TerminalState::Running,
        "the grace timer must leave an attached output tail open"
    );
    let mut visible = Vec::new();
    loop {
        match output.read_timeout(Duration::from_secs(1)) {
            TerminalOutputRead::Chunk(chunk) => visible.extend_from_slice(&chunk.bytes),
            TerminalOutputRead::Closed => break,
            TerminalOutputRead::TimedOut => panic!("late output reader lost the buffered tail"),
        }
    }
    assert_eq!(visible, b"abcdefgh");
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);
}

#[test]
fn explicit_close_unblocks_a_full_buffer_after_output_reader_disconnects() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(SaturatingOutputBackend::new());
    let leases = Arc::new(LeaseCounts::default());
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig {
            max_terminals: 4,
            output_capacity_bytes: 4,
            output_read_chunk_bytes: 4,
        },
        backend.clone(),
        Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let abandoned_reader = service.take_output(key).expect("output reader");
    backend
        .lifecycle
        .reads
        .wait_entered_times(2, "PTY reader did not fill its queue");
    drop(abandoned_reader);

    service.close(key).expect("explicit close");
    backend
        .lifecycle
        .dropped
        .wait_entered("explicit close left the PTY reader blocked");
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);

    let replacement = service.take_output(key).expect("replacement reader");
    assert_eq!(
        replacement.read_timeout(Duration::from_secs(1)),
        TerminalOutputRead::Chunk(interface_terminal::TerminalOutputChunk {
            id,
            generation: created.generation,
            sequence: 0,
            bytes: b"abcd".to_vec(),
        })
    );
    assert_eq!(
        replacement.read_timeout(Duration::from_secs(1)),
        TerminalOutputRead::Closed
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins generation, lease, and revision invariants.
fn lifecycle_generation_fencing_restart_and_leases_are_coherent() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let events = Arc::new(Events::default());
    let service = service(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    let first_key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    service.write(first_key, b"echo one\r").expect("write");
    assert_eq!(lock(&backend.record(0).writes)[0], b"echo one\r");

    let resized = service
        .resize(ResizeTerminalRequest {
            id,
            generation: created.generation,
            size: TerminalSize {
                columns: 132,
                rows: 43,
            },
        })
        .expect("resize");
    assert_eq!(resized.size.columns, 132);
    let renamed = service
        .rename(RenameTerminalRequest {
            id,
            generation: created.generation,
            title: " Build ".to_owned(),
        })
        .expect("rename");
    assert_eq!(renamed.title, "Build");

    backend.record(0).exit(7);
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.exit.and_then(|exit| exit.code), Some(7));
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);
    let restarted = service
        .restart(first_key, &approved)
        .expect("restart terminal");
    assert_eq!(restarted.generation, TerminalGeneration(2));
    assert_eq!(restarted.title, "Build");
    assert_eq!(restarted.size.columns, 132);
    assert_eq!(leases.acquired.load(Ordering::SeqCst), 2);
    assert!(matches!(
        service.write(first_key, b"stale"),
        Err(TerminalError::StaleGeneration {
            actual: TerminalGeneration(2),
            ..
        })
    ));

    let second_key = TerminalGenerationRequest {
        id,
        generation: restarted.generation,
    };
    assert!(matches!(
        service.dismiss_generation(first_key),
        Err(TerminalError::StaleGeneration {
            expected: TerminalGeneration(1),
            actual: TerminalGeneration(2),
            ..
        })
    ));
    assert_eq!(
        service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == id)
            .map(|terminal| (terminal.generation, terminal.state)),
        Some((TerminalGeneration(2), TerminalState::Running))
    );
    service.close(second_key).expect("close");
    assert_eq!(backend.record(1).closes.load(Ordering::SeqCst), 1);
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(leases.released.load(Ordering::SeqCst), 2);
    assert!(
        service
            .dismiss(DismissTerminalRequest { id })
            .expect("dismiss")
            .removed
    );
    assert!(service.snapshot().terminals.is_empty());
    assert_eq!(
        service.create(request(id, &directory), &approved),
        Err(TerminalError::RetiredId(id))
    );

    let revisions: Vec<u64> = lock(&events.0)
        .iter()
        .map(|event| match event {
            TerminalLifecycleEvent::Upsert { revision, .. }
            | TerminalLifecycleEvent::Removed { revision, .. } => *revision,
        })
        .collect();
    assert!(revisions.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn abandoning_an_unexposed_generation_does_not_retire_its_saved_id() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let events = Arc::new(Events::default());
    let service = service(
        Arc::clone(&backend),
        leases,
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create hidden generation");
    backend.record(0).exit(1);
    await_state(&service, id, TerminalState::Exited);
    let event_count = lock(&events.0).len();

    assert!(
        service
            .abandon_generation(TerminalGenerationRequest {
                id,
                generation: created.generation,
            })
            .expect("abandon hidden generation")
            .removed
    );
    assert_eq!(lock(&events.0).len(), event_count);
    assert!(service.snapshot().terminals.is_empty());

    let retried = service
        .create(request(id, &directory), &approved)
        .expect("same saved id remains reusable");
    assert_eq!(retried.generation, TerminalGeneration::initial());
}

#[test]
fn restore_resize_waits_for_an_in_flight_larger_resize() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let events = Arc::new(Events::default());
    let service = service(
        Arc::clone(&backend),
        leases,
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    let original = created.size;
    let expanded = TerminalSize {
        columns: 188,
        rows: 52,
    };
    let latch = Arc::new(Gate::default());
    backend
        .record(0)
        .block_first_resize_after_apply(Arc::clone(&latch));

    let first_service = service.clone();
    let first = thread::spawn(move || {
        first_service.resize(ResizeTerminalRequest {
            id,
            generation: created.generation,
            size: expanded,
        })
    });
    latch.wait_entered("first resize did not reach the PTY");

    let (started_sender, started_receiver) = std::sync::mpsc::channel();
    let (done_sender, done_receiver) = std::sync::mpsc::channel();
    let second_service = service.clone();
    let second = thread::spawn(move || {
        started_sender.send(()).expect("announce restore");
        let result = second_service.resize(ResizeTerminalRequest {
            id,
            generation: created.generation,
            size: original,
        });
        done_sender.send(()).expect("announce restored");
        result
    });
    started_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("restore started");
    let restore_finished_while_expand_was_blocked = done_receiver
        .recv_timeout(Duration::from_millis(150))
        .is_ok();

    latch.release();
    first.join().expect("expand thread").expect("expand resize");
    let restored = second
        .join()
        .expect("restore thread")
        .expect("restore resize");

    assert!(
        !restore_finished_while_expand_was_blocked,
        "the restore must serialize behind the physical expand"
    );
    assert_eq!(restored.size, original);
    assert_eq!(*lock(&backend.record(0).resizes), vec![expanded, original]);
    let snapshot = service.snapshot();
    assert_eq!(
        snapshot
            .terminals
            .iter()
            .find(|terminal| terminal.id == id)
            .expect("terminal summary")
            .size,
        original
    );
    let final_event_size = lock(&events.0)
        .iter()
        .rev()
        .find_map(|event| match event {
            TerminalLifecycleEvent::Upsert { terminal, .. } if terminal.id == id => {
                Some(terminal.size)
            }
            _ => None,
        })
        .expect("terminal lifecycle event");
    assert_eq!(final_event_size, original);
}

#[test]
fn exited_is_not_observable_until_workspace_lease_release_finishes() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let drop_state = Arc::new(Gate::default());
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend.clone(),
        Arc::new(BlockingLeaseProvider(Arc::clone(&drop_state))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("create");

    backend.record(0).exit(0);
    drop_state.wait_entered("lease drop did not start");
    assert_eq!(
        service.snapshot().terminals[0].state,
        TerminalState::Running
    );

    drop_state.release();
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn validation_and_final_service_drop_fail_closed() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let events = Arc::new(Events::default());
    let service = service(
        Arc::clone(&backend),
        Arc::clone(&leases),
        events,
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let mut invalid = request(id, &directory);
    invalid.size.columns = 0;
    assert_eq!(
        service.create(invalid, &approved),
        Err(TerminalError::InvalidSize)
    );
    let mut invalid = request(id, &directory);
    invalid.title = Some("bad\ntitle".to_owned());
    assert_eq!(
        service.create(invalid, &approved),
        Err(TerminalError::InvalidTitle)
    );

    let created = service
        .create(request(id, &directory), &approved)
        .expect("create");
    assert_eq!(
        service.write(
            TerminalGenerationRequest {
                id,
                generation: created.generation,
            },
            &vec![0_u8; 65_537],
        ),
        Err(TerminalError::InputTooLarge)
    );
    let clone = service.clone();
    drop(service);
    assert_eq!(backend.record(0).closes.load(Ordering::SeqCst), 0);
    drop(clone);
    assert_eq!(backend.record(0).closes.load(Ordering::SeqCst), 1);
    eventually("lease release timed out", || {
        leases.released.load(Ordering::SeqCst) == 1
    });
}

#[test]
fn provider_pollers_do_not_keep_the_final_terminal_service_alive() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    for _ in 0..2 {
        let mut launched = request(TerminalId::random(), &directory);
        launched.launch = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Claude,
        });
        launched.launch_session = Some(TerminalLaunchSessionV1 {
            version: 1,
            id: None,
            resume: true,
        });
        service
            .create(launched, &approved)
            .expect("Claude terminal");
    }

    drop(service);
    eventually("provider pollers retained the final service", || {
        leases.released.load(Ordering::SeqCst) == 2
    });
    assert_eq!(backend.record(0).closes.load(Ordering::SeqCst), 1);
    assert_eq!(backend.record(1).closes.load(Ordering::SeqCst), 1);
}

#[test]
fn final_service_drop_persists_a_complete_manual_claude_reopen_tuple() {
    const PROVIDER: &str = "46464646-4646-4646-8646-464646464646";

    let directory = temp_dir();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_wake_without_session(PROVIDER));
    let lookup_gate = Arc::new(Gate::default());
    *lock(&backend.provider_sessions.current_session_gate) = Some(Arc::clone(&lookup_gate));
    backend.set_next_foreground_cwd(nested.clone());
    let events = Arc::new(DurableReopenEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let process = backend
        .record(0)
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("manual Claude process");
    backend
        .provider_sessions
        .set_captured(&capture_token, process.clone(), nested.clone());
    lookup_gate.wait_entered("nested capture record did not start");
    let shell = service.snapshot().terminals[0].clone();
    assert!(shell.launch.is_none(), "default login shell is implicit");
    assert!(shell.launch_session.is_none());
    backend.provider_sessions.set_final(
        process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: PROVIDER.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    // A second, direct pane makes the regression assert that final-drop
    // promotion is scoped to the manual capture that owns the receipt.
    let unrelated_id = TerminalId::random();
    let mut unrelated = request(unrelated_id, &directory);
    unrelated.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    service
        .create(unrelated, &approved)
        .expect("unrelated direct Claude pane");

    drop(service);

    // A lookup already in progress temporarily owns Shared. Force that race
    // instead of assuming drop wins it, then wait for the durable completion
    // boundary before inspecting the exact reopen tuple and checkpoint below.
    assert!(lock(&events.persisted).is_empty());
    lookup_gate.release();
    eventually("final capture retirement timed out", || {
        lock(&backend.provider_sessions.ended).contains(&capture_token)
    });
    let persisted = lock(&events.persisted);
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].id, id);
    assert_eq!(persisted[0].cwd, nested.to_string_lossy());
    assert_eq!(
        persisted[0].launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
    assert_eq!(
        persisted[0]
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref()),
        Some(PROVIDER)
    );
    assert!(
        lock(&backend.provider_sessions.checkpointed)
            .iter()
            .any(|(token, session)| token == &capture_token
                && session.id.as_deref() == Some(PROVIDER))
    );
    assert!(lock(&backend.provider_sessions.ended).contains(&capture_token));
}

#[test]
#[allow(clippy::too_many_lines)] // One scenario pins replacement capture through final service teardown.
fn final_service_drop_uses_a_replacement_manual_claude_receipt() {
    const FIRST: &str = "47474747-4747-4747-8747-474747474747";
    const SECOND: &str = "48484848-4848-4848-8848-484848484848";

    let directory = temp_dir();
    let replacement_cwd = directory.path().join("replacement");
    std::fs::create_dir(&replacement_cwd).expect("replacement cwd");
    let replacement_cwd =
        std::fs::canonicalize(replacement_cwd).expect("canonical replacement cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(FIRST));
    let events = Arc::new(DurableReopenEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let first_process = backend
        .record(0)
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("first manual Claude process");
    backend.provider_sessions.set_captured(
        &capture_token,
        first_process.clone(),
        std::fs::canonicalize(directory.path()).expect("canonical root"),
    );
    await_launch_session_id(&service, id, FIRST);
    eventually(
        "initial Claude binding was not durably acknowledged",
        || !lock(&events.persisted).is_empty(),
    );
    lock(&events.persisted).clear();

    let replacement = TerminalProcessIdentity {
        process_id: first_process.process_id.saturating_add(1),
        process_group_id: first_process.process_group_id.saturating_add(1),
        terminal_device: first_process.terminal_device.clone(),
        process_start: "Mon Jan 1 00:05:00 2024".to_owned(),
    };
    // Model the wrapper's durable B claim linearizing inside the final seal,
    // after the service has already lost its last chance to poll-adopt B.
    backend.provider_sessions.replace_capture_during_next_seal(
        TerminalProviderSessionCaptureProcess {
            process: replacement.clone(),
            cwd: replacement_cwd.clone(),
        },
    );
    backend
        .record(0)
        .replace_foreground_process(replacement.clone(), replacement_cwd.clone());
    assert_eq!(
        service.snapshot().terminals[0]
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref()),
        Some(FIRST)
    );
    backend.provider_sessions.set_final(
        replacement.process_id,
        &capture_token,
        TerminalProviderSession {
            id: SECOND.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );

    drop(service);

    let persisted = lock(&events.persisted);
    let final_reopen = persisted.last().expect("final replacement reopen");
    assert_eq!(final_reopen.cwd, replacement_cwd.to_string_lossy());
    assert_eq!(
        final_reopen
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref()),
        Some(SECOND),
        "replacement Startup(B) must supersede the prior Claude A binding"
    );
    assert_eq!(
        lock(&backend.provider_sessions.checkpointed)
            .last()
            .and_then(|(_, session)| session.id.as_deref()),
        Some(SECOND)
    );
    assert!(
        !lock(&backend.provider_sessions.current_reads).contains(&replacement.process_id),
        "the final seal must carry B without a prior live-poll adoption"
    );
    assert!(lock(&backend.provider_sessions.ended).contains(&capture_token));
}

#[test]
fn final_service_drop_preserves_a_manual_receipt_when_its_cwd_was_removed() {
    const PROVIDER: &str = "49494949-4949-4949-8949-494949494949";

    let directory = temp_dir();
    let nested = directory.path().join("removed-before-drop");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_wake_without_session(PROVIDER));
    backend.set_next_foreground_cwd(nested.clone());
    let events = Arc::new(DurableReopenEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let process = backend
        .record(0)
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("manual Claude process");
    backend
        .provider_sessions
        .set_captured(&capture_token, process.clone(), nested.clone());
    eventually(
        "manual provider was not attached after its private wake",
        || lock(&backend.provider_sessions.current_reads).contains(&process.process_id),
    );
    backend.provider_sessions.set_final(
        process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: PROVIDER.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    std::fs::remove_dir(&nested).expect("remove captured cwd");

    drop(service);

    assert!(lock(&events.persisted).is_empty());
    assert!(lock(&backend.provider_sessions.ended).is_empty());
    assert!(lock(&backend.provider_sessions.sealed).contains(&capture_token));
    assert_eq!(
        backend.provider_sessions.final_reads.load(Ordering::SeqCst),
        0
    );
}

#[test]
fn a_live_poll_cannot_file_a_sealed_manual_binding_without_a_safe_cwd() {
    const PROVIDER: &str = "4a4a4a4a-4a4a-4a4a-8a4a-4a4a4a4a4a4a";

    let directory = temp_dir();
    let nested = directory.path().join("removed-before-close");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_wake_without_session(PROVIDER));
    backend.set_next_foreground_cwd(nested.clone());
    let events = Arc::new(DurableReopenEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let record = backend.record(0);
    let process = record
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("manual Claude process");
    backend
        .provider_sessions
        .set_captured(&capture_token, process.clone(), nested.clone());
    eventually(
        "manual provider was not attached after its private wake",
        || lock(&backend.provider_sessions.current_reads).contains(&process.process_id),
    );

    // Close seals the capture while its cwd is gone, and the provider keeps
    // running, so the pane stays live and closing with no safe cwd.
    std::fs::remove_dir(&nested).expect("remove captured cwd");
    record.keep_foreground_process_live_on_tree_teardown();
    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close");
    // The receipt lands after the seal; the live poll reads it more than once.
    backend.provider_sessions.set(
        process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: PROVIDER.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    let reads = |process_id| {
        lock(&backend.provider_sessions.current_reads)
            .iter()
            .filter(|read| **read == process_id)
            .count()
    };
    let before = reads(process.process_id);
    eventually("the live poll did not read the receipt", || {
        reads(process.process_id) >= before + 2
    });

    let filed = |summary: &TerminalSummary| {
        summary
            .launch_session
            .as_ref()
            .and_then(|session| session.id.as_deref())
            == Some(PROVIDER)
    };
    assert!(
        !lock(&events.persisted).iter().any(filed),
        "an incomplete manual reopen tuple was filed"
    );
    assert!(!lock(&events.events).iter().any(|event| matches!(
        event,
        TerminalLifecycleEvent::Upsert { terminal, .. } if filed(terminal)
    )));

    record.exit_foreground_process();
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.launch, None);
    assert_eq!(exited.launch_session, None);
    assert!(lock(&events.persisted).is_empty());
    assert!(
        lock(&backend.provider_sessions.ended).is_empty(),
        "the sealed receipt stays for startup recovery"
    );
}

#[test]
fn restart_settlement_files_a_late_manual_receipt_as_the_final_lookup_would() {
    const PROVIDER: &str = "4b4b4b4b-4b4b-4b4b-8b4b-4b4b4b4b4b4b";

    let directory = temp_dir();
    let nested = directory.path().join("nested");
    std::fs::create_dir(&nested).expect("nested provider cwd");
    let nested = std::fs::canonicalize(nested).expect("canonical nested cwd");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_wake_without_session(PROVIDER));
    backend.set_next_foreground_cwd(nested.clone());
    let events = Arc::new(DurableReopenEvents::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let record = backend.record(0);
    let process = record
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("manual Claude process");
    backend
        .provider_sessions
        .set_captured(&capture_token, process.clone(), nested.clone());
    eventually(
        "manual provider was not attached after its private wake",
        || lock(&backend.provider_sessions.current_reads).contains(&process.process_id),
    );

    // The shell exits first, then Claude, before its receipt lands; the final
    // lookup finds nothing and cannot retire the capture.
    backend
        .provider_sessions
        .fail_end_remaining
        .store(1, Ordering::SeqCst);
    record.keep_foreground_process_live_on_tree_teardown();
    record.exit(0);
    eventually("root-shell teardown did not complete", || {
        record.terminated.load(Ordering::SeqCst)
    });
    record.exit_foreground_process();
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.launch, None);
    assert_eq!(exited.launch_session, None);

    backend.provider_sessions.set_final(
        process.process_id,
        &capture_token,
        TerminalProviderSession {
            id: PROVIDER.to_owned(),
            transition: TerminalProviderSessionTransition::Startup,
        },
    );
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    service
        .settle_provider_capture_for_restart(key)
        .expect("settle the late receipt");

    let settled = summary_of(&service, id).expect("settled pane");
    assert_eq!(
        settled.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
    assert_eq!(settled.cwd, nested.to_string_lossy());
    assert_eq!(
        settled.launch_session.and_then(|session| session.id),
        Some(PROVIDER.to_owned())
    );
    let persisted = lock(&events.persisted);
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].cwd, nested.to_string_lossy());
    assert_eq!(
        persisted[0].launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
    drop(persisted);
    assert_eq!(
        lock(&backend.provider_sessions.ended).as_slice(),
        &[capture_token]
    );
    let restarted = service.restart(key, &approved).expect("restart");
    assert_eq!(
        restarted.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
}

#[test]
fn final_service_drop_retains_manual_lease_until_the_nested_group_is_absent() {
    const PROVIDER: &str = "45454545-4545-4545-8545-454545454545";

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_claude_binding(PROVIDER));
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    await_launch_session_id(&service, id, PROVIDER);
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let record = backend.record(0);
    record.keep_foreground_process_live_on_tree_teardown();

    drop(service);
    eventually("root teardown did not complete during final drop", || {
        record.terminated.load(Ordering::SeqCst)
    });
    assert_eq!(leases.released.load(Ordering::SeqCst), 0);
    assert!(lock(&backend.provider_sessions.sealed).contains(&capture_token));
    assert!(lock(&backend.provider_sessions.ended).is_empty());

    record.exit_foreground_process();
    eventually(
        "nested group absence did not release the retained lease",
        || leases.released.load(Ordering::SeqCst) == 1,
    );
    assert!(lock(&backend.provider_sessions.ended).is_empty());
}

#[test]
fn final_service_drop_retires_a_processless_manual_capture_after_teardown() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();

    drop(service);
    eventually(
        "processless shell lease was not released after sealing",
        || leases.released.load(Ordering::SeqCst) == 1,
    );
    assert!(lock(&backend.provider_sessions.sealed).contains(&capture_token));
    assert_eq!(
        lock(&backend.provider_sessions.ended).as_slice(),
        &[capture_token],
        "a claim-sealed ownerless generation is safe to retire after root teardown"
    );
}

#[test]
fn final_service_drop_retains_the_lease_until_transient_teardown_failure_recovers() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let leases = Arc::new(LeaseCounts::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let mut launched = request(id, &directory);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    service.create(launched, &approved).expect("create");
    let record = backend.record(0);
    let gate = Arc::new(Gate::default());
    record.fail_termination_until_allowed(Arc::clone(&gate));

    drop(service);
    gate.wait_entered("process teardown was not attempted");
    assert_eq!(
        leases.released.load(Ordering::SeqCst),
        0,
        "a failed final teardown released the worktree lease"
    );
    assert!(
        lock(&backend.provider_sessions.ended).is_empty(),
        "final drop retired capture evidence before process teardown"
    );

    gate.release();
    eventually(
        "lease release did not follow successful teardown retry",
        || leases.released.load(Ordering::SeqCst) == 1,
    );
    assert!(record.terminated.load(Ordering::SeqCst));
    assert!(
        lock(&backend.provider_sessions.ended).is_empty(),
        "deferred final teardown must leave capture recovery to startup"
    );
}

#[test]
fn real_pty_runs_in_the_authorized_cwd_and_streams_bytes() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(
        SystemTerminalPtyBackend::with_shell(PathBuf::from("/bin/sh").as_path())
            .expect("system shell"),
    );
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    service
        .write(key, b"printf '__IFACE_PTY_OK__\\n'; pwd; exit\r")
        .expect("terminal input");

    let bytes = read_until_closed(&output, Duration::from_secs(5));
    let rendered = String::from_utf8_lossy(&bytes);
    assert!(rendered.contains("__IFACE_PTY_OK__"), "output: {rendered}");
    assert!(
        rendered.contains(&directory.path().to_string_lossy().into_owned()),
        "output: {rendered}"
    );
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn real_pty_launches_the_fixed_agent_directly_with_inert_argv_and_environment() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let executable = directory.path().join("fake-codex");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '__IFACE_AGENT_OK__:%s:%s:%s\\n' \"$PWD\" \"$1\" \"$IFACE_AGENT_TEST\"\n",
    )
    .expect("fake agent");
    #[cfg(unix)]
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("agent permissions");
    let launch = TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Codex,
    };
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        &executable,
        vec!["fixed-argument".into()],
        vec![("IFACE_AGENT_TEST".into(), "fixed-environment".into())],
    )
    .expect("agent program")])
    .expect("launch catalog");
    let service = TerminalService::new(TerminalServiceParts {
        launches: catalog,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::new(
                SystemTerminalPtyBackend::with_shell(PathBuf::from("/bin/sh").as_path())
                    .expect("system shell"),
            ),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            Arc::new(Events::default()),
        )
    })
    .expect("service");
    let id = TerminalId::random();
    let mut launch_request = request(id, &directory);
    launch_request.launch = Some(launch);
    let created = service
        .create(launch_request, &approved)
        .expect("real agent terminal");
    assert_eq!(created.launch, Some(launch));
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    let bytes = read_until_closed(&output, Duration::from_secs(5));
    let rendered = String::from_utf8_lossy(&bytes);
    assert!(
        rendered.contains("__IFACE_AGENT_OK__"),
        "output: {rendered}"
    );
    assert!(rendered.contains("fixed-argument"), "output: {rendered}");
    assert!(rendered.contains("fixed-environment"), "output: {rendered}");
    assert!(
        rendered.contains(&directory.path().to_string_lossy().into_owned()),
        "output: {rendered}"
    );
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn real_pty_codex_new_panes_and_restarts_follow_installed_updates() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let old = directory.path().join("release-old");
    let new = directory.path().join("release-new");
    for (release, version) in [(&old, "old"), (&new, "new")] {
        std::fs::create_dir(release).expect("release directory");
        let executable = release.join("codex");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\nprintf '__CODEX_{version}__\\n'\nread -r line\n"),
        )
        .expect("fake Codex release");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
            .expect("executable release");
    }
    let current = directory.path().join("current");
    symlink(&old, &current).expect("initial current release");
    let launcher = directory.path().join("codex");
    symlink(current.join("codex"), &launcher).expect("stable launcher");
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        launcher,
        Vec::new(),
        Vec::new(),
    )
    .expect("Codex program")])
    .expect("catalog");
    let service = TerminalService::new(TerminalServiceParts {
        launches: catalog,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::new(SystemTerminalPtyBackend::default()),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            Arc::new(Events::default()),
        )
    })
    .expect("service");
    let mut create = request(TerminalId::random(), &directory);
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Codex,
    });
    let first = service.create(create.clone(), &approved).expect("old pane");
    assert_pty_version(&service, &first, "old");

    let replacement = directory.path().join("next");
    symlink(&new, &replacement).expect("updated release link");
    std::fs::rename(replacement, &current).expect("atomic installed update");
    create.id = TerminalId::random();
    let second = service.create(create.clone(), &approved).expect("new pane");
    assert_pty_version(&service, &second, "new");

    let first_key = TerminalGenerationRequest {
        id: first.id,
        generation: first.generation,
    };
    service.close(first_key).expect("close old pane");
    await_state(&service, first.id, TerminalState::Exited);
    std::fs::remove_file(old.join("codex")).expect("remove obsolete release executable");
    let restarted = service
        .restart(first_key, &approved)
        .expect("restart after update");
    assert_pty_version(&service, &restarted, "new");
    create.id = TerminalId::random();
    let third = service
        .create(create, &approved)
        .expect("pane after old release cleanup");
    assert_pty_version(&service, &third, "new");
    for pane in [second, restarted, third] {
        service
            .close(TerminalGenerationRequest {
                id: pane.id,
                generation: pane.generation,
            })
            .expect("close test pane");
    }
}

fn assert_pty_version(service: &TerminalService, pane: &TerminalSummary, version: &str) {
    let output = service
        .take_output(TerminalGenerationRequest {
            id: pane.id,
            generation: pane.generation,
        })
        .expect("pane output");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut bytes = Vec::new();
    while Instant::now() < deadline {
        match output.read_timeout(Duration::from_millis(100)) {
            TerminalOutputRead::Chunk(chunk) => bytes.extend(chunk.bytes),
            TerminalOutputRead::Closed => break,
            TerminalOutputRead::TimedOut => {}
        }
        if String::from_utf8_lossy(&bytes).contains(&format!("__CODEX_{version}__")) {
            return;
        }
    }
    panic!(
        "expected Codex {version}, got {}",
        String::from_utf8_lossy(&bytes)
    );
}

#[test]
fn real_pty_forced_close_treats_a_prompt_hup_exit_as_success() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let test_shell = directory.path().join("test-login-shell");
    std::fs::write(&test_shell, "#!/bin/sh\nexec /bin/sh -i\n").expect("test login shell");
    std::fs::set_permissions(&test_shell, std::fs::Permissions::from_mode(0o700))
        .expect("executable test login shell");
    let backend =
        Arc::new(SystemTerminalPtyBackend::with_shell(&test_shell).expect("system shell"));
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };

    let output = service.take_output(key).expect("output");
    service
        .write(key, b"printf '__IFACE_CLOSE_READY__\\n'\r")
        .expect("terminal input");
    read_until(
        &output,
        "__IFACE_CLOSE_READY__",
        Duration::from_secs(5),
        "terminal did not become ready",
    );

    service.close(key).expect("forced close is successful");
    await_state(&service, id, TerminalState::Exited);
    service.close(key).expect("repeated close is idempotent");
}

#[test]
fn real_pty_close_gives_a_hup_handler_time_to_finish() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let test_shell = directory.path().join("graceful-login-shell");
    let marker = directory.path().join("graceful-login-shell.done");
    std::fs::write(
        &test_shell,
        "#!/bin/sh\ntrap '/bin/sleep 0.05; printf done > \"$0.done\"; exit 0' HUP\nprintf '__IFACE_HUP_READY__\\n'\nwhile :; do /bin/sleep 1; done\n",
    )
    .expect("test login shell");
    std::fs::set_permissions(&test_shell, std::fs::Permissions::from_mode(0o700))
        .expect("executable test login shell");
    let backend =
        Arc::new(SystemTerminalPtyBackend::with_shell(&test_shell).expect("system shell"));
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    read_until(
        &output,
        "__IFACE_HUP_READY__",
        Duration::from_secs(5),
        "terminal did not become ready",
    );

    service.close(key).expect("forced close is successful");
    await_state(&service, id, TerminalState::Exited);
    assert_eq!(
        std::fs::read_to_string(marker).expect("HUP handler completion marker"),
        "done"
    );
}

#[test]
fn real_pty_close_reaps_a_descendant_after_its_supervisor_exits_on_hup() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let test_shell = directory.path().join("descendant-login-shell");
    let child_pid = directory.path().join("descendant-login-shell.child");
    std::fs::write(
        &test_shell,
        "#!/bin/sh\ntrap 'exit 0' HUP\n/bin/sh -c 'trap \"\" HUP; while :; do :; done' &\nprintf '%s' \"$!\" > \"$0.child\"\nprintf '__IFACE_DESCENDANT_READY__\\n'\nwhile :; do /bin/sleep 1; done\n",
    )
    .expect("test login shell");
    std::fs::set_permissions(&test_shell, std::fs::Permissions::from_mode(0o700))
        .expect("executable test login shell");
    let backend =
        Arc::new(SystemTerminalPtyBackend::with_shell(&test_shell).expect("system shell"));
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    read_until(
        &output,
        "__IFACE_DESCENDANT_READY__",
        Duration::from_secs(5),
        "terminal did not become ready",
    );
    let child = std::fs::read_to_string(&child_pid).expect("descendant pid");

    service.close(key).expect("forced close is successful");
    await_state(&service, id, TerminalState::Exited);
    eventually(
        "HUP-ignoring provider descendant survived terminal close",
        || {
            !Command::new("/bin/kill")
                .args(["-0", child.as_str()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        },
    );
}

#[test]
fn real_pty_natural_leader_exit_reaps_descendants_before_exit_and_lease_release() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let test_shell = directory.path().join("natural-exit-login-shell");
    let child_pid = directory.path().join("natural-exit-login-shell.child");
    let exit_marker = directory.path().join("natural-exit-login-shell.exit");
    std::fs::write(
        &test_shell,
        "#!/bin/sh\n/bin/sh -c 'trap \"\" HUP; while :; do /bin/sleep 1; done' &\nprintf '%s' \"$!\" > \"$0.child\"\nprintf '__IFACE_NATURAL_EXIT_READY__\\n'\nwhile [ ! -f \"$0.exit\" ]; do /bin/sleep 0.01; done\nexit 0\n",
    )
    .expect("test login shell");
    std::fs::set_permissions(&test_shell, std::fs::Permissions::from_mode(0o700))
        .expect("executable test login shell");
    let backend =
        Arc::new(SystemTerminalPtyBackend::with_shell(&test_shell).expect("system shell"));
    let leases = Arc::new(LeaseCounts::default());
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    read_until(
        &output,
        "__IFACE_NATURAL_EXIT_READY__",
        Duration::from_secs(5),
        "terminal did not become ready",
    );
    let child = std::fs::read_to_string(&child_pid).expect("descendant pid");
    let child_is_alive = || {
        Command::new("/bin/kill")
            .args(["-0", child.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(child_is_alive(), "test descendant did not start");

    std::fs::write(exit_marker, b"exit").expect("release natural leader exit");
    let exited = await_state(&service, id, TerminalState::Exited);
    assert_eq!(exited.exit.and_then(|exit| exit.code), Some(0));
    assert_eq!(leases.released.load(Ordering::SeqCst), 1);

    let survived = child_is_alive();
    if survived {
        let _ = Command::new("/bin/kill")
            .args(["-KILL", child.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    assert!(
        !survived,
        "naturally orphaned provider descendant survived Exited and lease release"
    );
}

#[test]
fn real_pty_close_forces_an_ignored_hup_after_the_bounded_grace() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let test_shell = directory.path().join("hup-ignoring-login-shell");
    std::fs::write(
        &test_shell,
        "#!/bin/sh\ntrap '' HUP\nprintf '__IFACE_HUP_IGNORED__\\n'\nwhile :; do :; done\n",
    )
    .expect("test login shell");
    std::fs::set_permissions(&test_shell, std::fs::Permissions::from_mode(0o700))
        .expect("executable test login shell");
    let backend =
        Arc::new(SystemTerminalPtyBackend::with_shell(&test_shell).expect("system shell"));
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    read_until(
        &output,
        "__IFACE_HUP_IGNORED__",
        Duration::from_secs(5),
        "terminal did not become ready",
    );

    let close_started = Instant::now();
    service.close(key).expect("forced close is successful");
    let close_elapsed = close_started.elapsed();
    assert!(
        close_elapsed >= Duration::from_millis(150),
        "ignored HUP skipped its grace: {close_elapsed:?}"
    );
    assert!(
        close_elapsed < Duration::from_secs(2),
        "ignored HUP exceeded its bounded grace: {close_elapsed:?}"
    );
    await_state(&service, id, TerminalState::Exited);
}

/// Poll the registry until the tab title settles, or give up. The reader
/// thread applies titles asynchronously, so a bare read races it.
fn await_title(service: &TerminalService, id: TerminalId, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let title = service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == id)
            .map(|terminal| terminal.title)
            .unwrap_or_default();
        if title == expected || Instant::now() >= deadline {
            return title;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn await_profile(
    service: &TerminalService,
    id: TerminalId,
    profile: Option<TerminalLaunchProfileId>,
) -> TerminalSummary {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let summary = service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == id)
            .expect("terminal");
        if summary.launch.map(|launch| launch.profile_id) == profile {
            return summary;
        }
        assert!(
            Instant::now() < deadline,
            "foreground profile did not become {profile:?}: {summary:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn shell_foreground_labels_never_invent_sessions_or_overwrite_custom_names() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let original = request(id, &directory);
    let created = service.create(original.clone(), &approved).expect("shell");
    let record = backend.record(0);
    for profile in [
        TerminalLaunchProfileId::Codex,
        TerminalLaunchProfileId::Claude,
    ] {
        *lock(&record.foreground_profile) = Some(profile);
        let promoted = await_profile(&service, id, Some(profile));
        assert!(promoted.launch_session.is_none());
        assert_eq!(promoted.generation, created.generation);
        assert_eq!(
            service
                .create(original.clone(), &approved)
                .expect("idempotent create"),
            promoted
        );
        *lock(&record.foreground_profile) = None;
        assert_eq!(await_profile(&service, id, None).title, created.title);
    }
    service
        .rename(RenameTerminalRequest {
            id,
            generation: created.generation,
            title: "My pane".to_owned(),
        })
        .expect("custom name");
    *lock(&record.foreground_profile) = Some(TerminalLaunchProfileId::Codex);
    assert_eq!(
        await_profile(&service, id, Some(TerminalLaunchProfileId::Codex)).title,
        "My pane"
    );
    *lock(&record.foreground_profile) = None;
    assert_eq!(await_profile(&service, id, None).title, "My pane");
    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close");
}

#[test]
fn attached_manual_claude_shows_its_mark_before_its_first_session_receipt() {
    // Claude writes no transcript until the first prompt, so an attached
    // manual process has no receipt yet. Its mark must not wait for one.
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service_with_launches(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        manual_claude_capture_catalog(&directory),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("capturing login shell");
    let capture_token = lock(&backend.provider_sessions.begun)[0]
        .capture_token
        .clone();
    let record = backend.record(0);
    let process = record
        .foreground_process_identity()
        .expect("foreground lookup")
        .expect("wrapper process");
    let root = std::fs::canonicalize(directory.path()).expect("canonical root");
    let reads = backend
        .provider_sessions
        .capture_reads
        .load(Ordering::SeqCst);
    backend
        .provider_sessions
        .set_captured(&capture_token, process, root);
    eventually("manual process was not attached", || {
        backend
            .provider_sessions
            .capture_reads
            .load(Ordering::SeqCst)
            > reads
    });
    thread::sleep(Duration::from_millis(50));

    *lock(&record.foreground_profile) = Some(TerminalLaunchProfileId::Claude);
    let shown = await_profile(&service, id, Some(TerminalLaunchProfileId::Claude));
    assert_eq!(shown.title, "Claude Code");
    assert!(shown.launch_session.is_none());
    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close shell");
}

#[cfg(target_os = "macos")]
#[test]
fn real_pty_recognizes_manual_provider_names_and_returns_to_shell() {
    use std::os::unix::fs::PermissionsExt;
    for (relative, profile, title) in [
        ("codex", TerminalLaunchProfileId::Codex, "Codex"),
        (
            "claude/versions/2.1.270",
            TerminalLaunchProfileId::Claude,
            "Claude Code",
        ),
    ] {
        let directory = temp_dir();
        let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
        // Native executables test real kernel names without provider requests or
        // dependency on the owner's CLI installation, including Claude version paths.
        let fixture_binary = directory.path().join(relative);
        std::fs::create_dir_all(fixture_binary.parent().unwrap()).expect("binary directory");
        std::fs::copy("/bin/sleep", &fixture_binary).expect("native fixture");
        assert!(
            std::process::Command::new("/usr/bin/codesign")
                .args(["--force", "--sign", "-"])
                .arg(&fixture_binary)
                .output()
                .expect("ad-hoc sign disposable fixture")
                .status
                .success()
        );
        let shell = directory.path().join("test-shell");
        std::fs::write(&shell, "#!/bin/sh\nexec /bin/zsh -f\n").expect("isolated shell");
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700))
            .expect("executable");
        let service = TerminalService::new(TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::new(SystemTerminalPtyBackend::with_shell(&shell).expect("backend")),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            Arc::new(Events::default()),
        ))
        .expect("service");
        let id = TerminalId::random();
        let created = service
            .create(request(id, &directory), &approved)
            .expect("shell");
        let key = TerminalGenerationRequest {
            id,
            generation: created.generation,
        };
        let output = service.take_output(key).expect("retain output");
        read_until(
            &output,
            "\x1b[?2004h",
            Duration::from_secs(3),
            "zsh editor did not become ready",
        );
        service
            .write(key, format!("./{relative} 30\r").as_bytes())
            .expect("manual launch");
        let promoted = await_profile(&service, id, Some(profile));
        assert_eq!(promoted.title, title);
        assert!(promoted.launch_session.is_none());
        service.write(key, &[3]).expect("stop fixture");
        assert_eq!(await_profile(&service, id, None).title, created.title);
        service.close(key).expect("close fixture shell");
    }
}

#[test]
fn opencode_activity_is_projected_before_output_without_renaming_the_pane() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Opencode,
        "/bin/sh",
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let bytes = b"\x1b]0;OC | Activity task\x07\x1b]0;interface-opencode-state:v1:ses_a|ses_a,-,busy,0,0\x07\x1b]0;interface-opencode-state:v1:ses_a|ses_a,-,busy,1,0\x07".to_vec();
    let service = service_with_launches(
        Arc::new(FakeBackend::with_output(bytes)),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalLaunchCatalog::new([program]).unwrap(),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Opencode,
    });
    let created = service.create(create, &approved).unwrap();
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let reader = service.take_output(key).unwrap();
    let mut output = Vec::new();
    while let TerminalOutputRead::Chunk(chunk) = reader.read_timeout(Duration::from_millis(200)) {
        output.extend_from_slice(&chunk.bytes);
    }
    let output = String::from_utf8(output).unwrap();
    assert!(!output.contains("interface-opencode-state"));
    assert!(output.contains("interface-terminal-activity:v1:working"));
    assert!(output.contains("interface-terminal-activity:v1:waiting"));
    assert_eq!(service.snapshot().terminals[0].title, "OC | Activity task");
    service.close(key).unwrap();
}

#[test]
fn codex_fast_mode_tracks_titles_without_changing_renamed_panes_or_resume_identity() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        "/bin/sh",
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let mut bytes = vec![b'.'; 4096];
    for title in [
        "codex | Fast off | gpt-6-astra | Ready | First task",
        "codex | Fast on | gpt-6-astra | Thinking | First task",
        "codex | Unsupported | Ready | Second task",
        "codex | Fast off | Future model | Ready | Final task",
    ] {
        bytes.extend_from_slice(format!("\x1b]0;{title}\x07").as_bytes());
    }
    let backend = Arc::new(FakeBackend::with_output(bytes));
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        backend,
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        TerminalLaunchCatalog::new([program]).unwrap(),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Codex,
    });
    let created = service.create(create, &approved).unwrap();
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    service
        .rename(RenameTerminalRequest {
            id,
            generation: created.generation,
            title: "My pane".to_owned(),
        })
        .unwrap();
    let reader = service.take_output(key).unwrap();
    while let TerminalOutputRead::Chunk(_) = reader.read_timeout(Duration::from_millis(200)) {}
    let summary = service.snapshot().terminals.remove(0);
    assert_eq!(summary.title, "My pane");
    assert!(summary.launch_session.is_none());
    let mode = summary.fast_mode.unwrap();
    assert_eq!(mode.model, "Future model");
    assert!(!mode.enabled);
    assert!(mode.ready);
    assert!(lock(&events.0).iter().any(|event| matches!(event,
        TerminalLifecycleEvent::Upsert { terminal, .. }
        if terminal.fast_mode.as_ref().is_some_and(|mode| mode.enabled && !mode.ready))));
    service.close(key).unwrap();
    await_state(&service, id, TerminalState::Exited);
    assert!(service.restart(key, &approved).unwrap().fast_mode.is_none());
}

/// Evicted until released: every other startup file is missing.
struct HeldStartupFile {
    path: PathBuf,
    held: Gate,
}

impl HeldStartupFile {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            held: Gate::default(),
        }
    }

    fn release(&self) {
        self.held.release();
    }
}

impl TerminalStartupFiles for HeldStartupFile {
    fn probe(&self, path: &Path) -> TerminalStartupFileProbe {
        if path != self.path {
            TerminalStartupFileProbe::Missing
        } else if self.held.released() {
            TerminalStartupFileProbe::Local
        } else {
            TerminalStartupFileProbe::Evicted
        }
    }

    fn materialize(&self, path: &Path) {
        if path == self.path {
            self.held.enter_and_wait();
        }
    }

    fn read_small(&self, _path: &Path) -> Option<String> {
        None
    }
}

fn service_with_startup_files(
    events: Arc<Events>,
    startup_files: Arc<dyn TerminalStartupFiles>,
) -> TerminalService {
    let events: Arc<dyn TerminalLifecycleSink> = events;
    TerminalService::new(TerminalServiceParts {
        startup_files,
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::new(FakeBackend::with_output(Vec::new())),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            events,
        )
    })
    .expect("service")
}

fn await_summary(
    service: &TerminalService,
    id: TerminalId,
    ready: impl Fn(&TerminalSummary) -> bool,
) -> TerminalSummary {
    eventually_some(
        Duration::from_secs(10),
        "summary never became ready",
        || summary_of(service, id).filter(|summary| ready(summary)),
    )
}

#[test]
fn a_pane_names_an_evicted_startup_file_until_it_downloads() {
    let directory = temp_dir();
    let cwd = std::fs::canonicalize(directory.path()).unwrap();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let held = Arc::new(HeldStartupFile::new(cwd.join("CLAUDE.md")));
    let events = Arc::new(Events::default());
    let startup_files: Arc<dyn TerminalStartupFiles> = held.clone();
    let service = service_with_startup_files(Arc::clone(&events), startup_files);
    let id = TerminalId::random();
    let created = service.create(request(id, &directory), &approved).unwrap();
    assert!(created.startup_download.is_none());

    let waiting = await_summary(&service, id, |summary| summary.startup_download.is_some());
    let download = waiting.startup_download.unwrap();
    assert_eq!(download.files, ["CLAUDE.md"]);
    assert_eq!(download.remaining, 1);

    held.release();
    await_summary(&service, id, |summary| summary.startup_download.is_none());
    let published: Vec<bool> = lock(&events.0)
        .iter()
        .filter_map(|event| match event {
            TerminalLifecycleEvent::Upsert { terminal, .. } if terminal.id == id => {
                Some(terminal.startup_download.is_some())
            }
            _ => None,
        })
        .collect();
    assert_eq!(published, [false, true, false]);
}

#[test]
fn an_exited_pane_stops_waiting_on_its_startup_files() {
    let directory = temp_dir();
    let cwd = std::fs::canonicalize(directory.path()).unwrap();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let held = Arc::new(HeldStartupFile::new(cwd.join(".mcp.json")));
    let startup_files: Arc<dyn TerminalStartupFiles> = held.clone();
    let service = service_with_startup_files(Arc::new(Events::default()), startup_files);
    let id = TerminalId::random();
    let created = service.create(request(id, &directory), &approved).unwrap();
    await_summary(&service, id, |summary| summary.startup_download.is_some());
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    service.close(key).unwrap();
    await_state(&service, id, TerminalState::Exited);
    let exited = await_summary(&service, id, |summary| {
        summary.state == TerminalState::Exited
    });
    assert!(exited.startup_download.is_none());
    held.release();
}

#[test]
fn a_bell_makes_the_pane_need_you_until_a_keystroke_answers_it() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let events = Arc::new(Events::default());
    let service = service(
        Arc::new(FakeBackend::with_output(b"build finished\x07".to_vec())),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let created = service.create(request(id, &directory), &approved).unwrap();
    assert!(created.attention.is_none());
    // Rust reads the bell off the PTY itself: no window has to be open.
    let waiting = await_summary(&service, id, |summary| summary.attention.is_some());
    let attention = waiting.attention.unwrap();
    assert_eq!(attention.version, 1);
    assert_eq!(attention.since, waiting.updated_at);
    assert!(lock(&events.0).iter().any(|event| matches!(event,
        TerminalLifecycleEvent::Upsert { terminal, .. } if terminal.id == id && terminal.attention.is_some())));
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    // A focus report is the emulator's, not an answer.
    service.write(key, b"\x1b[I").unwrap();
    assert!(summary_of(&service, id).unwrap().attention.is_some());
    service.write(key, b"y").unwrap();
    assert!(summary_of(&service, id).unwrap().attention.is_none());
    service.close(key).unwrap();
}

#[test]
fn codex_title_metadata_never_replaces_a_useful_task() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        "/bin/sh",
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let mut bytes = Vec::new();
    for title in [
        "codex | Fast off | gpt-6-astra | Ready | Fix pane names",
        "[ ! ] Action Required | codex | Fast on | gpt-6-astra | Fix pane names",
        "[ . ] Action Required | codex | Fast off | gpt-6-astra | 01a08e06-c060-78a2-a66d-3a43629736be",
        "codex | Fast on | gpt-6-astra | Future status | Internal metadata",
    ] {
        bytes.extend_from_slice(b"\x1b]0;");
        bytes.extend_from_slice(title.as_bytes());
        bytes.push(7);
    }
    let events = Arc::new(Events::default());
    let service = service_with_launches(
        Arc::new(FakeBackend::with_output(bytes)),
        Arc::new(LeaseCounts::default()),
        events.clone(),
        TerminalLaunchCatalog::new([program]).unwrap(),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Codex,
    });
    let created = service.create(create, &approved).unwrap();
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let reader = service.take_output(key).unwrap();
    while let TerminalOutputRead::Chunk(_) = reader.read_timeout(Duration::from_millis(200)) {}
    let summary = service.snapshot().terminals.remove(0);
    assert_eq!(summary.title, "Fix pane names");
    assert!(
        summary.fast_mode.is_none(),
        "unknown metadata hides an unverified Fast control"
    );
    assert!(
        summary.launch_session.is_none(),
        "an OSC UUID never becomes session authority"
    );
    assert!(lock(&events.0).iter().any(|event| matches!(event,
        TerminalLifecycleEvent::Upsert { terminal, .. } if terminal.fast_mode.as_ref().is_some_and(|m| m.enabled && !m.ready))));
    assert!(lock(&events.0).iter().all(|event| !matches!(event,
        TerminalLifecycleEvent::Upsert { terminal, .. } if terminal.title.contains("Action Required") || terminal.title.contains("Fast "))));
    service.close(key).unwrap();
}

#[test]
fn a_terminal_takes_the_title_its_program_announces() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_output(
        b"\x1b]0;I want you to fix parser recovery\x07ordinary output".to_vec(),
    ));
    let events = Arc::new(Events::default());
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    service
        .create(request(id, &directory), &approved)
        .expect("create");

    assert_eq!(
        await_title(&service, id, "Fix parser recovery"),
        "Fix parser recovery"
    );
    // The renderer learns through the same push a manual rename uses, so an
    // unopened tab still updates and no new channel is needed.
    let renamed = lock(&events.0).iter().any(|event| {
        matches!(event, TerminalLifecycleEvent::Upsert { terminal, .. }
            if terminal.title == "Fix parser recovery")
    });
    assert!(renamed, "a lifecycle upsert should carry the new title");
}

#[test]
fn codex_thread_uuid_titles_leave_new_panes_numbered() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_output(
        b"\x1b]0;01a06e3b-1918-7260-a9b5-50e54cf0f3c0\x07".to_vec(),
    ));
    let events = Arc::new(Events::default());
    let service = service(
        backend,
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.title = Some("Codex 2".to_owned());
    let created = service.create(create, &approved).expect("create");
    let output = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("output");
    while let TerminalOutputRead::Chunk(_) = output.read_timeout(Duration::from_secs(2)) {}

    assert_eq!(await_title(&service, id, "Codex 2"), "Codex 2");
    assert!(lock(&events.0).iter().all(|event| !matches!(event,
        TerminalLifecycleEvent::Upsert { terminal, .. }
            if terminal.title == "01a06e3b-1918-7260-a9b5-50e54cf0f3c0"
    )));
}

#[test]
fn codex_first_prompt_title_is_replaced_by_its_chat_name() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    // The exact OSC 0 sequence observed from Codex's `thread` title field:
    // provisional UUID, first prompt, then a provider chat-name update.
    let backend = Arc::new(FakeBackend::with_output(
        concat!(
            "\x1b]0;01a06e3b-1918-7260-a9b5-50e54cf0f3c0\x07",
            "\x1b]0;Name panes from the first prompt\x07",
            "\x1b]0;Useful chat title\x07"
        )
        .as_bytes()
        .to_vec(),
    ));
    let events = Arc::new(Events::default());
    let service = service(
        backend,
        Arc::new(LeaseCounts::default()),
        Arc::clone(&events),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.title = Some("Codex 2".to_owned());
    service.create(create, &approved).expect("create");

    assert_eq!(
        await_title(&service, id, "Useful chat title"),
        "Useful chat title"
    );
    let titles: Vec<_> = lock(&events.0)
        .iter()
        .filter_map(|event| {
            if let TerminalLifecycleEvent::Upsert { terminal, .. } = event {
                Some(terminal.title.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        titles,
        [
            "Codex 2",
            "Name panes from the first prompt",
            "Useful chat title"
        ]
    );
    // Titles never confer provider-session identity or resume authority.
    assert!(service.snapshot().terminals[0].launch_session.is_none());
}

#[test]
fn grok_status_titles_stay_numbered_until_the_session_file_names_the_pane() {
    let directory = temp_dir();
    let sessions = directory.path().join("grok-sessions");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::with_output(
        concat!(
            "\x1b]0;grok\x07",
            "\x1b]0;✳ Responding · grok\x07",
            "\x1b]0;Action Required | grok\x07",
        )
        .as_bytes()
        .to_vec(),
    ));
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Grok,
        "/bin/sh",
        Vec::new(),
        Vec::new(),
    )
    .expect("grok program");
    let service = service_with_launches(
        backend,
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalLaunchCatalog::new([program])
            .expect("catalog")
            .with_grok_sessions_root(sessions.clone()),
    );
    let id = TerminalId::random();
    let session_id = "8b764f80-19ee-47c7-a9da-08088a6db888";
    let mut create = request(id, &directory);
    create.title = Some("Grok 2".to_owned());
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Grok,
    });
    create.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(session_id.to_owned()),
        resume: false,
    });
    let created = service.create(create, &approved).expect("create");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    while let TerminalOutputRead::Chunk(_) = output.read_timeout(Duration::from_secs(2)) {}
    assert_eq!(await_title(&service, id, "Grok 2"), "Grok 2");

    let session_dir = sessions.join("project").join(session_id);
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::write(
        session_dir.join("summary.json"),
        r#"{"generated_title":"Name Grok panes"}"#,
    )
    .expect("summary");
    assert_eq!(
        await_title_until(&service, id, "Name Grok panes", Duration::from_secs(5)),
        "Name Grok panes"
    );
    // The file name is display only. It does not change the minted chat id.
    assert_eq!(
        service.snapshot().terminals[0]
            .launch_session
            .as_ref()
            .and_then(|session| session.id.clone())
            .as_deref(),
        Some(session_id)
    );

    service
        .rename(RenameTerminalRequest {
            id,
            generation: created.generation,
            title: "My pane".to_owned(),
        })
        .expect("rename");
    std::fs::write(
        session_dir.join("summary.json"),
        r#"{"generated_title":"A later generated title"}"#,
    )
    .expect("renamed summary");
    thread::sleep(Duration::from_secs(3));
    assert_eq!(await_title(&service, id, "My pane"), "My pane");
    service.close(key).expect("close");
}

#[test]
fn antigravity_panes_adopt_their_conversation_title_and_preserve_manual_renames() {
    let directory = temp_dir();
    let agy_root = directory.path().join("antigravity-cli");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let session_id = "8b764f80-19ee-47c7-a9da-08088a6db888";
    let binding = format!("agy-{session_id}");
    let logs = agy_root.join(format!("brain/{session_id}/.system_generated/logs"));
    let annotations = agy_root.join("annotations");
    std::fs::create_dir_all(&logs).expect("logs dir");
    std::fs::create_dir_all(&annotations).expect("annotations dir");
    std::fs::write(
        logs.join("transcript.jsonl"),
        "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"content\":\"<USER_REQUEST>Name Antigravity panes</USER_REQUEST>\"}\n",
    )
    .expect("transcript");

    let backend = FakeBackend::with_claude_binding(&binding);
    lock(&backend.plans)[0].output = b"\x1b]0;Antigravity\x07\x1b]0;agy\x07".to_vec();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Gemini,
        "/bin/sh",
        Vec::new(),
        Vec::new(),
    )
    .expect("antigravity program")
    .with_provider_session_capture()
    .expect("antigravity capture");
    let service = service_with_launches(
        Arc::new(backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalLaunchCatalog::new([program])
            .expect("catalog")
            .with_antigravity_root(agy_root),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.title = Some("Antigravity 2".to_owned());
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Gemini,
    });
    let created = service.create(create, &approved).expect("create");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    while let TerminalOutputRead::Chunk(_) = output.read_timeout(Duration::from_millis(200)) {}
    await_launch_session_id(&service, id, &binding);
    let wait = Duration::from_secs(5);
    assert_eq!(
        await_title_until(&service, id, "Name Antigravity panes", wait),
        "Name Antigravity panes"
    );

    let annotation = annotations.join(format!("{session_id}.pbtxt"));
    std::fs::write(&annotation, "title:\"Antigravity Terminal Titles\"\n").expect("annotation");
    assert_eq!(
        await_title_until(&service, id, "Antigravity Terminal Titles", wait),
        "Antigravity Terminal Titles"
    );
    let saved = interface_contracts::terminal::TerminalWorkspaceTerminal {
        id: TerminalId::random(),
        title: "Antigravity".to_owned(),
        cwd: directory.path().to_string_lossy().into_owned(),
        session_key: None,
        launch: created.launch,
        launch_session: Some(TerminalLaunchSessionV1 {
            version: 1,
            id: Some(binding),
            resume: true,
        }),
    };
    assert_eq!(
        service.saved_terminal_title(&saved).expect("saved title"),
        "Antigravity Terminal Titles"
    );

    service
        .rename(RenameTerminalRequest {
            id,
            generation: created.generation,
            title: "My pane".to_owned(),
        })
        .expect("rename");
    std::fs::write(&annotation, "title:\"A later generated title\"\n").expect("renamed annotation");
    thread::sleep(Duration::from_secs(3));
    assert_eq!(await_title(&service, id, "My pane"), "My pane");
    service.close(key).expect("close");
}

fn await_title_until(
    service: &TerminalService,
    id: TerminalId,
    expected: &str,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let title = service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == id)
            .map(|terminal| terminal.title)
            .unwrap_or_default();
        if title == expected || Instant::now() >= deadline {
            return title;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_announced_title_never_overwrites_one_the_user_typed() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    // Enough leading output that the rename lands before the title arrives.
    let mut output = vec![b'.'; 4096];
    output.extend_from_slice(b"\x1b]0;Announced\x07");
    let backend = Arc::new(FakeBackend::with_output(output));
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let summary = service
        .create(request(id, &directory), &approved)
        .expect("create");
    service
        .rename(RenameTerminalRequest {
            id,
            generation: summary.generation,
            title: "My tab".to_owned(),
        })
        .expect("rename");

    // Drain so the reader thread can run to completion past the buffer bound.
    let reader = service
        .take_output(TerminalGenerationRequest {
            id,
            generation: summary.generation,
        })
        .expect("output reader");
    while let TerminalOutputRead::Chunk(_) = reader.read_timeout(Duration::from_millis(200)) {}

    assert_eq!(await_title(&service, id, "Announced"), "My tab");
}

#[test]
fn a_program_that_only_announces_its_own_name_leaves_the_tab_numbered() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    // What Claude Code and Grok actually emit at startup, plus what Codex
    // emits (the project folder, which every tab in the folder would share).
    let folder = directory
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("folder name")
        .to_owned();
    let backend = Arc::new(FakeBackend::with_output(
        format!("\x1b]0;\u{2733} Claude Code\x07\x1b]0;grok\x07\x1b]0;{folder}\x07").into_bytes(),
    ));
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.title = Some("Claude 2".to_owned());
    service.create(create, &approved).expect("create");

    assert_eq!(await_title(&service, id, "never"), "Claude 2");
}

#[test]
fn a_restart_keeps_the_name_the_user_typed() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    lock(&backend.plans).extend([
        FakePlan {
            output: Vec::new(),
            claude_provider_sessions: Vec::new(),
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: None,
            output_suffix: Vec::new(),
            fail_spawn: false,
        },
        FakePlan {
            output: b"\x1b]0;Announced after restart\x07".to_vec(),
            claude_provider_sessions: Vec::new(),
            claude_provider_steps: Vec::new(),
            wrong_token_provider_session: None,
            previous_token_provider_session: None,
            output_suffix: Vec::new(),
            fail_spawn: false,
        },
    ]);
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let summary = service
        .create(request(id, &directory), &approved)
        .expect("create");
    service
        .rename(RenameTerminalRequest {
            id,
            generation: summary.generation,
            title: "My tab".to_owned(),
        })
        .expect("rename");
    backend.record(0).exit(0);
    let key = TerminalGenerationRequest {
        id,
        generation: summary.generation,
    };
    eventually("the exited pane did not restart", || {
        service.restart(key, &approved).is_ok()
    });

    assert_eq!(
        await_title(&service, id, "Announced after restart"),
        "My tab"
    );
}

#[test]
fn real_pty_adopts_a_title_a_live_shell_announces() {
    // The fake backend proves the policy; this proves the wiring against a
    // genuine PTY, where the escape arrives interleaved with shell noise and
    // split across whatever read boundaries the kernel chooses.
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(
        SystemTerminalPtyBackend::with_shell(PathBuf::from("/bin/sh").as_path())
            .expect("system shell"),
    );
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        backend,
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .expect("service");
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("real terminal");
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    let output = service.take_output(key).expect("output");
    service
        .write(key, b"printf '\\033]0;Live shell topic\\007'; exit\r")
        .expect("terminal input");

    // Keep draining; the reader thread blocks on a full buffer otherwise.
    read_until_closed(&output, Duration::from_secs(5));

    assert_eq!(
        await_title(&service, id, "Live shell topic"),
        "Live shell topic"
    );
}

fn fake_agent(directory: &TempDir, name: &str) -> PathBuf {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let executable = directory.path().join(name);
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("fake agent");
    #[cfg(unix)]
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))
        .expect("agent permissions");
    executable
}

#[cfg(unix)]
fn permission_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .expect("transcript metadata")
        .permissions()
        .mode()
        & 0o777
}

fn set_modified(path: &Path, when: SystemTime) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open transcript");
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .expect("transcript mtime");
}

/// A running Claude pane in one approved root, a *second* approved root that
/// every pass-off request names, and an isolated transcript directory. The two
/// roots are what make "the new pane starts where the source works" a real
/// assertion rather than a restatement of the request.
struct HandoffFixture {
    source_root: TempDir,
    requested_root: TempDir,
    transcript_directory: PathBuf,
    _handoffs: TempDir,
    _binaries: TempDir,
    approved: ApprovedTerminalRoots,
    backend: Arc<FakeBackend>,
    leases: Arc<LeaseCounts>,
    service: TerminalService,
    source: TerminalId,
}

impl HandoffFixture {
    fn new() -> Self {
        let source_root = temp_dir();
        let requested_root = temp_dir();
        let handoffs = temp_dir();
        let binaries = temp_dir();
        let approved = ApprovedTerminalRoots::new([
            source_root.path().to_path_buf(),
            requested_root.path().to_path_buf(),
        ])
        .expect("roots");
        let catalog = TerminalLaunchCatalog::new([
            TerminalLaunchProgram::new(
                TerminalLaunchProfileId::Claude,
                fake_agent(&binaries, "fake-claude"),
                Vec::new(),
                Vec::new(),
            )
            .expect("claude program"),
            TerminalLaunchProgram::new(
                TerminalLaunchProfileId::Codex,
                fake_agent(&binaries, "fake-codex"),
                vec!["--fixed-host-argument".into()],
                Vec::new(),
            )
            .expect("codex program"),
            TerminalLaunchProgram::new(
                TerminalLaunchProfileId::Kimi,
                fake_agent(&binaries, "fake-kimi"),
                Vec::new(),
                Vec::new(),
            )
            .expect("kimi program"),
        ])
        .expect("launch catalog");
        let backend = Arc::new(FakeBackend::default());
        let leases = Arc::new(LeaseCounts::default());
        // Deliberately a directory that does not exist yet, so the first write
        // has to create it and its permissions are observable.
        let transcripts = handoffs.path().join("transcripts");
        let service = TerminalService::new(TerminalServiceParts {
            launches: catalog,
            handoff_directory: transcripts.clone(),
            ..TerminalServiceParts::new(
                TerminalServiceConfig::default(),
                Arc::clone(&backend) as Arc<dyn TerminalPtyBackend>,
                Arc::new(SharedLeaseProvider(Arc::clone(&leases))),
                Arc::new(Events::default()),
            )
        })
        .expect("service");
        let source = TerminalId::random();
        let mut opened = request(source, &source_root);
        opened.launch = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Claude,
        });
        service.create(opened, &approved).expect("source terminal");
        Self {
            source_root,
            requested_root,
            transcript_directory: transcripts,
            _handoffs: handoffs,
            _binaries: binaries,
            approved,
            backend,
            leases,
            service,
            source,
        }
    }

    fn handoff(&self, transcript: &str) -> TerminalHandoffV1 {
        TerminalHandoffV1 {
            version: 1,
            source_terminal_id: self.source,
            transcript: transcript.to_owned(),
            from_title: Some("bridgemind".to_owned()),
            kind: None,
        }
    }

    fn pass_off(&self, id: TerminalId, handoff: TerminalHandoffV1) -> CreateTerminalRequest {
        let mut pass_off = request(id, &self.requested_root);
        pass_off.launch = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Codex,
        });
        pass_off.handoff = Some(handoff);
        pass_off
    }

    /// Empty while no pass-off has written anything: a refused create must not
    /// even create the directory.
    fn transcripts(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.transcript_directory) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        files.sort();
        files
    }
}

#[test]
fn create_with_handoff_appends_prompt_and_writes_transcript_file() {
    let fixture = HandoffFixture::new();
    let id = TerminalId::random();
    let created = fixture
        .service
        .create(
            fixture.pass_off(id, fixture.handoff("did A\ndid B")),
            &fixture.approved,
        )
        .expect("pass-off terminal");
    assert_eq!(created.id, id);
    // The summary names its source so a renderer that did not create the
    // pane can still seat it beside the pane it came from.
    assert_eq!(created.passed_off_from, Some(fixture.source));

    let file = fixture.transcript_directory.join(format!("{id}.md"));
    let transcript = std::fs::read_to_string(&file).expect("handoff transcript");
    assert!(transcript.contains("did A"), "transcript: {transcript}");
    assert!(
        transcript.starts_with("# Pass-off transcript\n"),
        "transcript: {transcript}"
    );
    assert!(transcript.contains("did B"), "transcript: {transcript}");
    assert!(
        transcript.contains("bridgemind"),
        "transcript: {transcript}"
    );
    // Verbatim terminal output is private to this user, like a pasted image.
    #[cfg(unix)]
    {
        assert_eq!(permission_mode(&file), 0o600);
        assert_eq!(permission_mode(&fixture.transcript_directory), 0o700);
    }

    {
        let specs = lock(&fixture.backend.specs);
        assert_eq!(specs.len(), 2);
        let spawned = &specs[1];
        // The pass-off pane starts where the source agent works. The fake
        // backend reports no pid, so this exercises the recorded-cwd fallback,
        // and it is deliberately not the directory the request named.
        assert_eq!(spawned.cwd, fixture.source_root.path());
        assert_ne!(spawned.cwd, fixture.requested_root.path());
        let ResolvedTerminalLaunch::Program { arguments, .. } = &spawned.launch else {
            panic!("a pass-off must launch a program: {:?}", spawned.launch);
        };
        let prompt = arguments.last().expect("prompt argument").to_string_lossy();
        assert!(prompt.contains("taking over"), "prompt: {prompt}");
        assert!(
            prompt.contains(file.to_string_lossy().as_ref()),
            "prompt: {prompt}"
        );
    }

    // The create fingerprint excludes the handoff by construction, so a replay
    // carrying a different transcript is still the same create — and it must
    // not rewrite the file the live pane was already pointed at.
    let replayed = fixture
        .service
        .create(
            fixture.pass_off(id, fixture.handoff("did C")),
            &fixture.approved,
        )
        .expect("idempotent replay");
    assert_eq!(replayed, created);
    assert_eq!(lock(&fixture.backend.specs).len(), 2);
    assert_eq!(
        std::fs::read_to_string(&file).expect("handoff transcript"),
        transcript
    );
}

#[test]
fn create_with_a_brief_handoff_writes_a_brief_that_asks_for_a_report() {
    let fixture = HandoffFixture::new();
    let id = TerminalId::random();
    let mut brief = fixture.handoff("Rename Foo to Bar in crates/x and run its tests.");
    brief.kind = Some(interface_contracts::terminal::TerminalHandoffKind::Brief);
    let created = fixture
        .service
        .create(fixture.pass_off(id, brief), &fixture.approved)
        .expect("brief pass-off terminal");
    assert_eq!(created.passed_off_from, Some(fixture.source));

    let file = fixture.transcript_directory.join(format!("{id}.md"));
    let body = std::fs::read_to_string(&file).expect("brief file");
    assert!(body.starts_with("# Pass-off brief\n"), "brief: {body}");
    assert!(
        body.contains(&format!("From: bridgemind (pane {})", fixture.source)),
        "brief: {body}"
    );
    assert!(
        body.contains(&format!(
            "send_to_pane tool with pane_id {}",
            fixture.source
        )),
        "brief: {body}"
    );
    assert!(
        body.contains("Rename Foo to Bar in crates/x and run its tests."),
        "brief: {body}"
    );
    assert!(!body.contains("TUI chrome"), "brief: {body}");

    let specs = lock(&fixture.backend.specs);
    let ResolvedTerminalLaunch::Program { arguments, .. } = &specs[1].launch else {
        panic!("a pass-off must launch a program: {:?}", specs[1].launch);
    };
    let prompt = arguments.last().expect("prompt argument").to_string_lossy();
    assert!(prompt.contains("passed you a task"), "prompt: {prompt}");
    assert!(prompt.contains("(bridgemind)"), "prompt: {prompt}");
    assert!(!prompt.contains("taking over"), "prompt: {prompt}");
    assert!(
        prompt.contains(file.to_string_lossy().as_ref()),
        "prompt: {prompt}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_process_under_a_running_pane_resolves_to_that_pane() {
    let fixture = HandoffFixture::new();
    // The source pane's "child" is this test process, so a child of the test
    // process is a descendant of the pane the way `nc` is of a CLI.
    fixture.backend.record(0).set_process_id(std::process::id());
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .spawn()
        .expect("spawn sleep");
    assert_eq!(
        fixture.service.terminal_for_process_ancestor(child.id()),
        Some((fixture.source, TerminalGeneration::initial()))
    );
    assert_eq!(
        fixture.service.terminal_for_process_ancestor(u32::MAX - 7),
        None
    );
    child.kill().ok();
    child.wait().ok();
    fixture
        .service
        .close(interface_contracts::terminal::TerminalGenerationRequest {
            id: fixture.source,
            generation: TerminalGeneration::initial(),
        })
        .expect("close source");
    await_state(&fixture.service, fixture.source, TerminalState::Exited);
    // An exited pane owns nothing: its pid may already belong to someone else.
    assert_eq!(
        fixture
            .service
            .terminal_for_process_ancestor(std::process::id()),
        None
    );
}

#[test]
fn create_with_handoff_from_a_missing_source_is_refused() {
    let fixture = HandoffFixture::new();
    let mut orphaned = fixture.handoff("did A");
    orphaned.source_terminal_id = TerminalId::random();
    assert_eq!(
        fixture.service.create(
            fixture.pass_off(TerminalId::random(), orphaned),
            &fixture.approved
        ),
        Err(TerminalError::HandoffSourceUnavailable)
    );
    assert_eq!(lock(&fixture.backend.specs).len(), 1);
    assert!(fixture.transcripts().is_empty());
}

#[test]
fn a_pass_off_whose_pane_never_starts_leaves_no_transcript_behind() {
    let fixture = HandoffFixture::new();
    lock(&fixture.backend.plans).push_back(FakePlan {
        output: Vec::new(),
        claude_provider_sessions: Vec::new(),
        claude_provider_steps: Vec::new(),
        wrong_token_provider_session: None,
        previous_token_provider_session: None,
        output_suffix: Vec::new(),
        fail_spawn: true,
    });
    assert!(
        fixture
            .service
            .create(
                fixture.pass_off(TerminalId::random(), fixture.handoff("did A")),
                &fixture.approved
            )
            .is_err()
    );
    assert!(fixture.transcripts().is_empty());
}

/// The next spawn prints `output` and then waits, like a TUI at rest.
fn plan_output(fixture: &HandoffFixture, output: &[u8]) {
    lock(&fixture.backend.plans).push_back(FakePlan {
        output: output.to_vec(),
        claude_provider_sessions: Vec::new(),
        claude_provider_steps: Vec::new(),
        wrong_token_provider_session: None,
        previous_token_provider_session: None,
        output_suffix: Vec::new(),
        fail_spawn: false,
    });
}

fn kimi_pass_off(fixture: &HandoffFixture, id: TerminalId) -> CreateTerminalRequest {
    let mut pass_off = fixture.pass_off(id, fixture.handoff("did A"));
    pass_off.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Kimi,
    });
    pass_off
}

// Kimi Code takes no initial prompt on argv, so the host types the one-line
// brief once its editor has enabled bracketed paste and its output rests.
#[test]
fn a_kimi_pass_off_is_typed_once_after_its_editor_takes_a_paste() {
    let fixture = HandoffFixture::new();
    plan_output(&fixture, b"\x1b]0;Kimi Code\x07\x1b[?2004h\x1b[?25l> ");
    let id = TerminalId::random();
    fixture
        .service
        .create(kimi_pass_off(&fixture, id), &fixture.approved)
        .expect("Kimi pass-off");
    {
        let specs = lock(&fixture.backend.specs);
        let ResolvedTerminalLaunch::Program { arguments, .. } = &specs[1].launch else {
            panic!("a pass-off must launch a program: {:?}", specs[1].launch);
        };
        assert!(arguments.is_empty(), "Kimi rejects positional prompts");
    }
    let record = Arc::clone(&lock(&fixture.backend.records)[1]);
    eventually("the brief is pasted and submitted", || {
        lock(&record.writes).len() >= 2
    });
    let writes = lock(&record.writes).clone();
    let paste = String::from_utf8(writes[0].clone()).expect("utf-8 paste");
    let file = fixture.transcript_directory.join(format!("{id}.md"));
    assert!(paste.starts_with("\x1b[200~"), "paste: {paste:?}");
    assert!(paste.ends_with("\x1b[201~"), "paste: {paste:?}");
    assert!(paste.contains("passed you a task") || paste.contains("taking over"));
    assert!(
        paste.contains(file.to_string_lossy().as_ref()),
        "paste: {paste:?}"
    );
    assert!(
        !paste.contains(['\r', '\n']),
        "the paste never submits itself"
    );
    assert_eq!(writes[1], b"\r");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(lock(&record.writes).len(), 2, "typed exactly once");
}

#[test]
fn a_kimi_pass_off_is_never_typed_before_an_editor_takes_a_paste() {
    let fixture = HandoffFixture::new();
    plan_output(&fixture, b"Signing in to Kimi Code...\r\n");
    fixture
        .service
        .create(
            kimi_pass_off(&fixture, TerminalId::random()),
            &fixture.approved,
        )
        .expect("Kimi pass-off");
    let record = Arc::clone(&lock(&fixture.backend.records)[1]);
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(lock(&record.writes).is_empty());
}

#[test]
fn create_with_handoff_on_a_login_shell_is_refused() {
    let fixture = HandoffFixture::new();
    let mut implicit = fixture.pass_off(TerminalId::random(), fixture.handoff("did A"));
    implicit.launch = None;
    assert_eq!(
        fixture.service.create(implicit, &fixture.approved),
        Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Shell
        ))
    );
    let mut explicit = fixture.pass_off(TerminalId::random(), fixture.handoff("did A"));
    explicit.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Shell,
    });
    assert_eq!(
        fixture.service.create(explicit, &fixture.approved),
        Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Shell
        ))
    );
    assert_eq!(lock(&fixture.backend.specs).len(), 1);
    assert!(fixture.transcripts().is_empty());
}

#[test]
fn create_with_an_oversized_handoff_transcript_is_refused() {
    let fixture = HandoffFixture::new();
    let mut oversized = fixture.handoff("");
    oversized.transcript = "x".repeat(TERMINAL_HANDOFF_TRANSCRIPT_MAX_BYTES + 1);
    assert_eq!(
        fixture.service.create(
            fixture.pass_off(TerminalId::random(), oversized),
            &fixture.approved
        ),
        Err(TerminalError::InvalidHandoff)
    );
    assert_eq!(lock(&fixture.backend.specs).len(), 1);
    assert!(fixture.transcripts().is_empty());
}

#[test]
fn create_with_an_unversioned_handoff_is_refused() {
    let fixture = HandoffFixture::new();
    let mut unversioned = fixture.handoff("did A");
    unversioned.version = 2;
    assert_eq!(
        fixture.service.create(
            fixture.pass_off(TerminalId::random(), unversioned),
            &fixture.approved
        ),
        Err(TerminalError::InvalidHandoff)
    );
    assert_eq!(lock(&fixture.backend.specs).len(), 1);
    assert!(fixture.transcripts().is_empty());
}

#[test]
fn creating_a_pass_off_prunes_stale_and_excess_transcripts() {
    let fixture = HandoffFixture::new();
    let directory = fixture.transcript_directory.clone();
    std::fs::create_dir(&directory).expect("transcript directory");
    let now = SystemTime::now();
    let stale = directory.join("stale.md");
    std::fs::write(&stale, "old").expect("stale transcript");
    set_modified(&stale, now - Duration::from_secs(48 * 60 * 60));
    let recent: Vec<PathBuf> = (0..70_u64)
        .map(|index| {
            let path = directory.join(format!("recent-{index:02}.md"));
            std::fs::write(&path, "recent").expect("recent transcript");
            // Distinct, ordered mtimes inside the TTL: index 0 is the oldest.
            set_modified(&path, now - Duration::from_secs(3_600 - index * 10));
            path
        })
        .collect();

    let id = TerminalId::random();
    fixture
        .service
        .create(
            fixture.pass_off(id, fixture.handoff("did A")),
            &fixture.approved,
        )
        .expect("pass-off terminal");

    assert!(!stale.exists(), "a transcript older than the TTL survived");
    for path in &recent[..6] {
        assert!(!path.exists(), "excess transcript survived: {path:?}");
    }
    for path in &recent[6..] {
        assert!(path.exists(), "retained transcript was pruned: {path:?}");
    }
    // The 64 newest survivors plus the transcript this create just wrote.
    assert!(directory.join(format!("{id}.md")).exists());
    assert_eq!(fixture.transcripts().len(), 65);
}

/// The only path that exercises `live_process_cwd` for real: the fake control
/// reports the pid of a genuine child parked in a subdirectory of an approved
/// root, so the service resolves that subdirectory through `lsof` rather than
/// through the recorded-cwd fallback.
///
/// Everything downstream must still speak in terms of the *root*: the cleanup
/// lease and `summary.root` name the approved root, `cwd` names the
/// subdirectory, and the pane stays reopenable even though the exact rule
/// refuses that subdirectory on its own.
#[cfg(target_os = "macos")]
#[test]
fn a_pass_off_into_a_live_subdirectory_keeps_its_root_and_restarts() {
    let fixture = HandoffFixture::new();
    let subdirectory = fixture.source_root.path().join("crates");
    std::fs::create_dir(&subdirectory).expect("subdirectory");
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("60")
        .current_dir(&subdirectory)
        .spawn()
        .expect("spawn sleep");
    fixture.backend.record(0).set_process_id(child.id());

    let id = TerminalId::random();
    let created = fixture.service.create(
        fixture.pass_off(id, fixture.handoff("did A")),
        &fixture.approved,
    );
    child.kill().ok();
    child.wait().ok();
    let created = created.expect("pass-off terminal");

    assert_eq!(Path::new(&created.cwd), subdirectory);
    assert_eq!(Path::new(&created.root), fixture.source_root.path());
    assert_eq!(lock(&fixture.backend.specs)[1].cwd, subdirectory);

    // The worktree guard is asked about the root, so the lease must name it;
    // leasing the subdirectory would let the tree be removed underneath a live
    // pane.
    let lease = lock(&fixture.leases.requests)
        .iter()
        .find(|request| request.id == id)
        .cloned()
        .expect("lease request");
    assert_eq!(lease.root, fixture.source_root.path());
    assert_eq!(lease.cwd, subdirectory);

    // The exact rule genuinely refuses this directory — an ordinary create
    // naming it is unauthorized — so a reopen has to re-authorize the stored
    // root and re-verify the cwd inside it rather than the cwd alone.
    let mut ordinary = request(TerminalId::random(), &fixture.requested_root);
    ordinary.cwd = subdirectory.to_string_lossy().into_owned();
    assert_eq!(
        fixture.service.create(ordinary, &fixture.approved),
        Err(TerminalError::UnauthorizedCwd)
    );
    let key = TerminalGenerationRequest {
        id,
        generation: created.generation,
    };
    fixture.service.close(key).expect("close pass-off");
    await_state(&fixture.service, id, TerminalState::Exited);
    let restarted = fixture
        .service
        .restart(key, &fixture.approved)
        .expect("restart pass-off");
    assert_ne!(restarted.generation, created.generation);
    assert_eq!(Path::new(&restarted.cwd), subdirectory);
    assert_eq!(Path::new(&restarted.root), fixture.source_root.path());
    let specs = lock(&fixture.backend.specs);
    assert_eq!(specs.len(), 3);
    assert_eq!(specs[2].cwd, subdirectory);
}

/// Spec section 6: both ordinary and pass-off terminals stay in the checkout
/// Interface selected. A pass-off differs only by following its source agent's
/// live cwd and receiving the transcript prompt.
#[test]
fn claude_launches_stay_in_the_selected_checkout() {
    let fixture = HandoffFixture::new();
    let passed = TerminalId::random();
    let mut pass_off = fixture.pass_off(passed, fixture.handoff("did A"));
    pass_off.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    pass_off.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("pass-off-chat".to_owned()),
        resume: false,
    });
    fixture
        .service
        .create(pass_off, &fixture.approved)
        .expect("pass-off terminal");

    let fresh = TerminalId::random();
    let mut launched = request(fresh, &fixture.requested_root);
    launched.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    launched.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("fresh-chat".to_owned()),
        resume: false,
    });
    fixture
        .service
        .create(launched, &fixture.approved)
        .expect("fresh terminal");

    let specs = lock(&fixture.backend.specs);
    let argv = |index: usize| -> Vec<String> {
        match &specs[index].launch {
            ResolvedTerminalLaunch::Program { arguments, .. } => arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            other @ (ResolvedTerminalLaunch::LoginShell
            | ResolvedTerminalLaunch::LoginShellWithEnvironment { .. }) => {
                panic!("expected a program launch: {other:?}")
            }
        }
    };

    let taken_over = argv(1);
    assert!(
        taken_over
            .iter()
            .any(|argument| argument.as_str() == "--session-id=pass-off-chat"),
        "{taken_over:?}"
    );
    assert!(
        taken_over
            .last()
            .expect("prompt argument")
            .contains("taking over"),
        "{taken_over:?}"
    );
    assert!(
        !taken_over
            .iter()
            .any(|argument| argument.as_str() == "--worktree"),
        "a pass-off must stay in its source checkout: {taken_over:?}"
    );

    // The ordinary launch also stays in the checkout the user selected.
    let started = argv(2);
    assert!(
        started
            .iter()
            .any(|argument| argument.as_str() == "--session-id=fresh-chat"),
        "{started:?}"
    );
    assert!(
        !started
            .iter()
            .any(|argument| argument.as_str() == "--worktree"),
        "a fresh launch must not create a worktree implicitly: {started:?}"
    );
}

#[test]
fn a_transcript_that_contains_a_fence_cannot_break_out_of_the_block() {
    let fixture = HandoffFixture::new();
    let id = TerminalId::random();
    let transcript = "before\n```text\ninner\n```\nafter";
    fixture
        .service
        .create(
            fixture.pass_off(id, fixture.handoff(transcript)),
            &fixture.approved,
        )
        .expect("pass-off terminal");

    let body = std::fs::read_to_string(fixture.transcript_directory.join(format!("{id}.md")))
        .expect("handoff transcript");
    assert!(body.contains(transcript), "body: {body}");
    // One backtick longer than the longest run inside, on both ends, so the
    // block closes only where the host closed it.
    assert!(body.contains("\n````text\n"), "body: {body}");
    assert!(body.trim_end().ends_with("\n````"), "body: {body}");
}

#[cfg(target_os = "macos")]
#[test]
fn owned_server_links_keep_two_worktrees_and_an_unrelated_listener_separate() {
    let directory = temp_dir();
    let base = directory.path().join("base");
    let other = directory.path().join("other");
    std::fs::create_dir(&base).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&base)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    std::fs::write(base.join("server.py"), "import socket,pathlib,time\ns=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(); pathlib.Path('port.txt').write_text(str(s.getsockname()[1])); time.sleep(120)\n").unwrap();
    git(&["add", "server.py"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "fixture",
    ]);
    git(&[
        "worktree",
        "add",
        "--detach",
        other.to_str().unwrap(),
        "HEAD",
    ]);
    let unrelated = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let unrelated_port = unrelated.local_addr().unwrap().port();
    let roots = ApprovedTerminalRoots::new([base.clone(), other.clone()]).unwrap();
    let service = TerminalService::new(TerminalServiceParts::new(
        TerminalServiceConfig::default(),
        Arc::new(SystemTerminalPtyBackend::with_shell(Path::new("/bin/sh")).unwrap()),
        Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
        Arc::new(Events::default()),
    ))
    .unwrap();
    let mut keys = Vec::new();
    for root in [&base, &other] {
        let mut launch = request(TerminalId::random(), &directory);
        launch.cwd = root.to_string_lossy().into_owned();
        let terminal = service.create(launch, &roots).unwrap();
        let key = TerminalGenerationRequest {
            id: terminal.id,
            generation: terminal.generation,
        };
        service
            .write(key, b"/usr/bin/python3 -B server.py\r")
            .unwrap();
        keys.push(key);
    }
    // The server creates port.txt before it writes the port into it.
    let port = |root: &PathBuf| {
        std::fs::read_to_string(root.join("port.txt"))
            .ok()?
            .parse::<u16>()
            .ok()
    };
    let ports = eventually_some(
        Duration::from_secs(5),
        "owned servers did not start",
        || Some([port(&base)?, port(&other)?]),
    );
    for (index, key) in keys.iter().enumerate() {
        let links = service.server_links(*key).unwrap();
        assert_eq!(links.len(), 1, "foreign listener entered owned tree");
        assert_eq!(links[0].url, format!("http://127.0.0.1:{}", ports[index]));
        assert!(!links[0].url.ends_with(&unrelated_port.to_string()));
        assert!(
            service
                .server_links(TerminalGenerationRequest {
                    generation: key.generation.next(),
                    ..*key
                })
                .is_err()
        );
    }
    service.close(keys[0]).unwrap();
    assert!(service.server_links(keys[0]).is_err());
    assert_eq!(service.server_links(keys[1]).unwrap().len(), 1);
    assert!(
        std::net::TcpStream::connect(unrelated.local_addr().unwrap()).is_ok(),
        "closing a pane killed the unrelated listener"
    );
    service.close(keys[1]).unwrap();
}

#[test]
#[cfg(target_os = "macos")]
fn report_worker_identity_requires_a_live_confirmed_provider_and_survives_reopen() {
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let launches = claude_capture_catalog(&directory);
    std::fs::write(
        directory.path().join("fixed-claude"),
        "#!/bin/sh\nexec /bin/cat\n",
    )
    .unwrap();
    let lookup = Arc::new(FakeProviderSessions::default());
    let service = TerminalService::new(TerminalServiceParts {
        launches,
        provider_sessions: lookup.clone(),
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::new(SystemTerminalPtyBackend::default()),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            Arc::new(Events::default()),
        )
    })
    .unwrap();
    let session = "11111111-1111-4111-8111-111111111111";
    let id = TerminalId::random();
    let mut create = request(id, &directory);
    create.launch = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Claude,
    });
    create.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(session.into()),
        resume: true,
    });
    let first = service.create(create, &approved).unwrap();
    let (token, process) = lock(&lookup.recorded).last().unwrap().clone();
    let request = TerminalGenerationRequest {
        id,
        generation: first.generation,
    };
    assert!(
        service
            .report_worker_conversation(request, process.process_id)
            .is_err(),
        "requested resume is not provider proof"
    );
    lookup.set(
        process.process_id,
        &token,
        TerminalProviderSession {
            id: session.into(),
            transition: TerminalProviderSessionTransition::Resume,
        },
    );
    let identity = service
        .report_worker_conversation(request, process.process_id)
        .unwrap();
    assert_eq!(identity, (TerminalLaunchProfileId::Claude, session.into()));
    assert!(
        service
            .report_worker_conversation(request, std::process::id())
            .is_err(),
        "unrelated process cannot collect"
    );
    service.close(request).unwrap();
    await_state(&service, id, TerminalState::Exited);
    assert!(
        service
            .report_worker_conversation(request, process.process_id)
            .is_err()
    );
    let reopened = service.restart(request, &approved).unwrap();
    let (token, process) = lock(&lookup.recorded).last().unwrap().clone();
    lookup.set(
        process.process_id,
        &token,
        TerminalProviderSession {
            id: session.into(),
            transition: TerminalProviderSessionTransition::Resume,
        },
    );
    let current = TerminalGenerationRequest {
        id,
        generation: reopened.generation,
    };
    assert_eq!(
        service
            .report_worker_conversation(current, process.process_id)
            .unwrap(),
        identity
    );
    assert!(
        service
            .report_worker_conversation(request, process.process_id)
            .is_err(),
        "old generation cannot reclaim"
    );
    service.close(current).unwrap();
    await_state(&service, id, TerminalState::Exited);
}

#[test]
fn workspace_sleep_ticker_serves_a_terminal_created_after_the_last_was_dismissed() {
    use interface_contracts::terminal::TerminalWorkspaceActivityRequest;
    let directory = temp_dir();
    let prompt = b"\x1b[?2004h$ ".to_vec();
    let backend = Arc::new(FakeBackend::with_output(prompt.clone()));
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let first = service
        .create(request(TerminalId::random(), &directory), &approved)
        .unwrap();
    service
        .close(TerminalGenerationRequest {
            id: first.id,
            generation: first.generation,
        })
        .unwrap();
    await_state(&service, first.id, TerminalState::Exited);
    assert!(
        service
            .dismiss(DismissTerminalRequest { id: first.id })
            .unwrap()
            .removed
    );
    // With no terminal left, the shared ticker stops at its next tick.
    thread::sleep(Duration::from_millis(1_500));
    lock(&backend.plans).push_back(FakePlan {
        output: prompt,
        claude_provider_sessions: Vec::new(),
        claude_provider_steps: Vec::new(),
        wrong_token_provider_session: None,
        previous_token_provider_session: None,
        output_suffix: Vec::new(),
        fail_spawn: false,
    });
    let second = service
        .create(request(TerminalId::random(), &directory), &approved)
        .unwrap();
    service
        .set_workspace_activity(&TerminalWorkspaceActivityRequest {
            version: 1,
            id: second.id,
            generation: second.generation,
            active: false,
            screen: None,
            output_cursor: None,
        })
        .unwrap();
    let record = backend.record(1);
    eventually_within(
        Duration::from_secs(8),
        "the restarted ticker never ran",
        || record.suspended.load(Ordering::SeqCst),
    );
    service
        .close(TerminalGenerationRequest {
            id: second.id,
            generation: second.generation,
        })
        .unwrap();
}

#[test]
fn workspace_sleep_service_retains_generation_and_lease_and_wakes_before_input() {
    use interface_contracts::terminal::TerminalWorkspaceActivityRequest;
    let directory = temp_dir();
    let backend = Arc::new(FakeBackend::with_output(b"\x1b[?2004h$ ".to_vec()));
    let leases = Arc::new(LeaseCounts::default());
    let service = service(
        Arc::clone(&backend),
        Arc::clone(&leases),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let created = service
        .create(request(TerminalId::random(), &directory), &approved)
        .unwrap();
    let key = TerminalGenerationRequest {
        id: created.id,
        generation: created.generation,
    };
    let record = backend.record(0);
    let inactive = TerminalWorkspaceActivityRequest {
        version: 1,
        id: key.id,
        generation: key.generation,
        active: false,
        screen: None,
        output_cursor: None,
    };
    service.set_workspace_activity(&inactive).unwrap();
    eventually_within(
        Duration::from_secs(8),
        "idle workspace did not sleep",
        || record.suspended.load(Ordering::SeqCst),
    );
    assert_eq!(leases.released.load(Ordering::SeqCst), 0);
    assert_eq!(
        service.snapshot().terminals[0].generation,
        created.generation
    );
    assert!(
        service
            .set_workspace_activity(&TerminalWorkspaceActivityRequest {
                generation: key.generation.next(),
                ..inactive.clone()
            })
            .is_err()
    );
    assert!(record.suspended.load(Ordering::SeqCst));
    service
        .set_workspace_activity(&TerminalWorkspaceActivityRequest {
            active: true,
            ..inactive
        })
        .unwrap();
    assert!(!record.suspended.load(Ordering::SeqCst));
    record.suspended.store(true, Ordering::SeqCst);
    service.write(key, b"printf done\r").unwrap();
    assert!(
        !record.suspended.load(Ordering::SeqCst),
        "remote/native input must wake first"
    );
    assert_eq!(lock(&record.writes).last().unwrap(), b"printf done\r");
    assert_eq!(
        lock(&backend.specs).len(),
        1,
        "waking must not spawn a replacement"
    );
    service.close(key).unwrap();
}

#[test]
fn a_shell_label_refresh_never_waits_on_a_resume_under_the_state_lock() {
    use interface_contracts::terminal::TerminalWorkspaceActivityRequest;
    let directory = temp_dir();
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
    let backend = Arc::new(FakeBackend::default());
    let service = service(
        Arc::clone(&backend),
        Arc::new(LeaseCounts::default()),
        Arc::new(Events::default()),
        TerminalServiceConfig::default(),
    );
    let id = TerminalId::random();
    let created = service
        .create(request(id, &directory), &approved)
        .expect("shell");
    let record = backend.record(0);
    // Park the ticker's next label refresh in its foreground probe, which
    // runs after its suspension check. Codex is in the foreground by the
    // time the probe answers.
    let probe = Arc::new(Gate::default());
    record.gate_foreground_profile(Arc::clone(&probe));
    probe.wait_entered("provider liveness check did not start");
    *lock(&record.foreground_profile) = Some(TerminalLaunchProfileId::Codex);
    // A resume now holds the pane's power lock across its (parked) wake.
    let resume = Arc::new(Gate::default());
    record.gate_resume(Arc::clone(&resume));
    let waking = service.clone();
    let woken = thread::spawn(move || {
        waking.set_workspace_activity(&TerminalWorkspaceActivityRequest {
            version: 1,
            id,
            generation: created.generation,
            active: true,
            screen: None,
            output_cursor: None,
        })
    });
    resume.wait_entered("provider liveness check did not start");
    probe.release();
    // The refresh publishes the label without waiting on the power lock,
    // and snapshots, which take only the state lock, keep answering.
    let (done, finished) = mpsc::channel();
    let reader = service.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let shown = reader.snapshot().terminals[0]
                .launch
                .map(|launch| launch.profile_id);
            if shown == Some(TerminalLaunchProfileId::Codex) || Instant::now() >= deadline {
                let _ = done.send(shown);
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    });
    let shown = finished.recv_timeout(Duration::from_secs(3));
    resume.release();
    woken.join().expect("wake thread").expect("wake");
    assert_eq!(
        shown,
        Ok(Some(TerminalLaunchProfileId::Codex)),
        "the label refresh held the state lock while the resume held the power lock"
    );
    service
        .close(TerminalGenerationRequest {
            id,
            generation: created.generation,
        })
        .expect("close");
}

/// Parks the durable pin until the test releases it.
struct ParkedTitles {
    pinning: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl interface_terminal::TerminalTitlePersistence for ParkedTitles {
    fn user_title(&self, _: TerminalId) -> Result<Option<String>, TerminalBackendError> {
        Ok(None)
    }
    fn pin_title(&self, _: TerminalId, _: &str) -> Result<(), TerminalBackendError> {
        if let Some(pinning) = lock(&self.pinning).take() {
            let _ = pinning.send(());
        }
        let _ = lock(&self.release).recv();
        Ok(())
    }
}

#[test]
fn a_durable_rename_does_not_hold_the_state_lock_across_its_write() {
    let directory = temp_dir();
    let (pinning, pinned) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let service = TerminalService::new(TerminalServiceParts {
        title_persistence: Arc::new(ParkedTitles {
            pinning: Mutex::new(Some(pinning)),
            release: Mutex::new(released),
        }),
        ..TerminalServiceParts::new(
            TerminalServiceConfig::default(),
            Arc::new(FakeBackend::default()),
            Arc::new(SharedLeaseProvider(Arc::new(LeaseCounts::default()))),
            Arc::new(Events::default()),
        )
    })
    .expect("service");
    let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).unwrap();
    let created = service
        .create(request(TerminalId::random(), &directory), &approved)
        .unwrap();
    let pinning_service = service.clone();
    let renamed = thread::spawn(move || {
        pinning_service.rename(RenameTerminalRequest {
            id: created.id,
            generation: created.generation,
            title: "Pinned name".to_owned(),
        })
    });
    pinned
        .recv_timeout(Duration::from_secs(2))
        .expect("the rename reaches its durable write");
    // Keystrokes and snapshots, which take only the state lock, still run
    // while the write is parked. (Event publishers such as resize stay
    // ordered behind the rename by the event gate, by design.)
    let (done, finished) = mpsc::channel();
    let typist = service.clone();
    thread::spawn(move || {
        let _ = typist.snapshot();
        let typed = typist.write(
            TerminalGenerationRequest {
                id: created.id,
                generation: created.generation,
            },
            b"x",
        );
        let _ = done.send(typed.is_ok());
    });
    assert!(
        finished
            .recv_timeout(Duration::from_secs(2))
            .expect("snapshot and input do not wait for the durable pin"),
        "input reaches the pane"
    );
    release.send(()).unwrap();
    assert_eq!(renamed.join().unwrap().unwrap().title, "Pinned name");
    service
        .close(TerminalGenerationRequest {
            id: created.id,
            generation: created.generation,
        })
        .unwrap();
}
