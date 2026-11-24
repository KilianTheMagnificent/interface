//! Provider replay: recorded Claude, Codex and Grok sessions replayed
//! through the real adapters, runtime, desktop dispatcher and threads.
//!
//! Each scenario runs the real `SessionRuntime` with the native adapter
//! pointed at `provider-replay`, which serves a transcript recorded from the
//! real CLI (`crates/provider-replay/fixtures/<engine>/<scenario>.json`).
//! The scenario talks to the host as the renderer does — `createSession`,
//! `threadControl` bind, `send`, `respond`, `interrupt`, `closeSession`,
//! `resumeSession` through a `RuntimeHostDispatcher` with its event pump —
//! and the `ThreadHost` sees every event through its run tap, as in the app.
//! The same scenario code drives the recording (`provider_record_live.rs`),
//! so the frames the host sends when replaying are the frames it sent when
//! recording.
//!
//! No sleeps: every wait is for a pushed event or thread view, bounded by a
//! generous upper limit. A replay that meets a frame its transcript does not
//! expect exits and writes why to stderr, which the wrapper keeps in a file
//! each failure message here includes.
#![cfg(unix)]

use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use engine_acp::{AcpAdapterConfig, BuiltinAcpProvider};
use engine_codex::CodexAdapterConfig;
use interface_contracts::thread_journal::JournalEntry;
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    ipc::{CHANNELS, CollaborationStatusDto, CollaborationSubmitRequest},
    threads::{RunOutcome, ThreadView, ThreadsView, TurnState},
};
use interface_host::{
    DispatchError, HostDispatcher, PushFrame, RuntimeHostDispatcher, SessionEventObserver,
    ThreadHost, ThreadHostConfig, ThreadJournalConfig, ThreadPorts, ThreadRegistrySignal,
    threads::{
        FolderAuthority, HiddenSessions, PullRequestLookup, PullRequests, RuntimeThreadSessions,
        ShipTools, SystemThreadGit, ThreadConsults, ThreadSessions as _, ThreadTitles,
    },
};
use interface_registry::SessionRegistry;
use interface_review::thread_git::ship::Gh;
use interface_runtime::{
    EngineHistoryStore, HistoryLimits, NativeAdapterConfiguration, RuntimeServices, SessionClaims,
    SessionRuntime, TerminalSessionBindings,
};
use interface_settings::thread_journal::{self, JournalWatch};
use interface_storage::PrivateDirectory;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::mpsc;

/// Upper bound on one wait while replaying. Generous: the gate runs four
/// test binaries at once on a loaded machine.
pub(crate) const REPLAY_LIMIT: Duration = Duration::from_secs(60);
/// How often a wait with nothing arriving looks at the replayer's report.
const REPORT_CHECK: Duration = Duration::from_secs(1);
/// Codex's request timeout and ACP's prompt timeout while replaying: far
/// past any wait here, so no adapter deadline decides a replay.
const ADAPTER_DEADLINE: Duration = Duration::from_secs(60 * 60);

/// The shortest prompts that produce each scenario.
pub(crate) const SIMPLE_PROMPT: &str = "Reply with the single word: ready";
pub(crate) const TOOL_PROMPT: &str = "Run the shell command `touch ready.txt` in the current directory, then reply with the single word: done";
pub(crate) const INTERRUPT_PROMPT: &str =
    "Count from 1 to 300, one number per line, with no other text.";
pub(crate) const RESUME_PROMPT: &str =
    "What single word did you reply with before? Reply with that word only.";

/// The scenarios, by fixture name.
pub(crate) const SCENARIOS: [&str; 4] = ["simple", "tool-approval", "interrupt", "resume"];

/// A provider engine as the host names it, and its fixture folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Engine {
    Claude,
    Codex,
    Grok,
}

impl Engine {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "acp:grok",
        }
    }

    pub(crate) const fn folder(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }

    /// The launch options every recording and replay of this engine uses:
    /// the cheapest model, no owner settings where the provider allows it.
    pub(crate) fn launch(self, cwd: &Path, prompt: &str) -> Value {
        let cwd = cwd.to_string_lossy();
        match self {
            // Settings isolation keeps the owner's hooks, plugins and MCP
            // servers out of the recording.
            Self::Claude => json!({
                "cwd": cwd, "prompt": prompt, "model": "haiku", "kind": "code",
                "settingSources": [], "permissionMode": "default"
            }),
            // `untrusted`: Codex asks before any command it does not know
            // to be safe, so `touch` raises an approval.
            Self::Codex => json!({
                "cwd": cwd, "prompt": prompt, "model": "gpt-6-luna", "effort": "low",
                "kind": "code", "permissionMode": "untrusted"
            }),
            Self::Grok => json!({"cwd": cwd, "prompt": prompt, "kind": "code"}),
        }
    }
}

/// What the provider executable is.
pub(crate) enum Provider {
    /// `provider-replay` serving this transcript.
    Replay { transcript: PathBuf },
    /// `provider-record` running the real CLI `target`, journaling for `out`.
    Record { target: PathBuf, out: PathBuf },
}

/// The workspace's built binaries: the test executable's own profile
/// directory (`target/<profile>/deps/..`).
pub(crate) fn built_binary(name: &str) -> PathBuf {
    let executable = std::env::current_exe().expect("test executable");
    let profile = executable
        .parent()
        .and_then(Path::parent)
        .expect("target profile directory");
    let binary = profile.join(name);
    assert!(
        binary.is_file(),
        "{} is missing: build it with `cargo build -p provider-replay` (the gate's \
         `cargo test --workspace` builds it)",
        binary.display()
    );
    binary
}

/// Writes an executable script named `name` in `directory` that sets the
/// replay or record environment and runs the binary in the CLI's place,
/// keeping its stderr in `report`. The variables reach the binary through
/// its environment only: the adapters pass argv through unchanged.
fn wrapper(directory: &Path, name: &str, provider: &Provider, report: &Path) -> PathBuf {
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"));
    let body = match provider {
        Provider::Replay { transcript } => format!(
            "{}={}\nexport {}\nexec {} \"$@\" 2>>{}\n",
            provider_replay::TRANSCRIPT_ENV,
            quote(transcript),
            provider_replay::TRANSCRIPT_ENV,
            quote(&built_binary("provider-replay")),
            quote(report),
        ),
        Provider::Record { target, out } => format!(
            "{}={}\n{}={}\nexport {} {}\nexec {} \"$@\"\n",
            provider_replay::RECORD_TARGET_ENV,
            quote(target),
            provider_replay::RECORD_OUT_ENV,
            quote(out),
            provider_replay::RECORD_TARGET_ENV,
            provider_replay::RECORD_OUT_ENV,
            quote(&built_binary("provider-record")),
        ),
    };
    let path = directory.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("wrapper script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("wrapper mode");
    path
}

/// One turn's events for one session, in order.
#[derive(Debug, Default)]
pub(crate) struct Turn {
    pub(crate) text: String,
    pub(crate) kinds: Vec<String>,
    pub(crate) approvals: Vec<String>,
    pub(crate) tool_calls: Vec<String>,
    pub(crate) tool_results: usize,
}

/// The host as the app composes it around one engine.
pub(crate) struct Host {
    _temp: TempDir,
    /// The sessions' working directory (canonical, as a real project's).
    pub(crate) work: PathBuf,
    report: PathBuf,
    /// The thread journal's directory (`thread-journal/` in the data root).
    journal: PathBuf,
    limit: Duration,
    runtime: Arc<SessionRuntime>,
    dispatcher: Arc<RuntimeHostDispatcher<SessionRuntime>>,
    threads: ThreadHost,
    pushes: mpsc::Receiver<PushFrame>,
    /// Every session event pushed so far, and how far waits have read.
    events: Vec<(String, EngineEventKind)>,
    read: usize,
    /// Every thread view pushed so far: a wait looks for the view a commit
    /// pushed, not only the latest, since later events can move a thread on.
    views: Vec<ThreadsView>,
}

impl Host {
    pub(crate) fn new(engine: Engine, provider: &Provider, limit: Duration) -> Self {
        let temp = tempfile::tempdir().expect("host temp directory");
        let root = std::fs::canonicalize(temp.path()).expect("canonical temp root");
        let work = root.join("work");
        let bin = root.join("bin");
        let state = root.join("state");
        for directory in [&work, &bin, &state] {
            std::fs::create_dir_all(directory).expect("host directory");
        }
        let report = root.join("provider-stderr.txt");
        std::fs::write(&report, "").expect("report file");
        let executable = wrapper(&bin, engine.folder(), provider, &report);
        let replaying = matches!(provider, Provider::Replay { .. });

        let runtime_root = state.join("runtime");
        drop(PrivateDirectory::open_or_create(&runtime_root).expect("private runtime root"));
        let sessions = SessionRegistry::open(&runtime_root).expect("session registry");
        let history = EngineHistoryStore::open(&runtime_root, HistoryLimits::default())
            .expect("engine history");
        let mut builder = SessionRuntime::builder(RuntimeServices::new(sessions, history));
        let configuration = match engine {
            Engine::Claude => NativeAdapterConfiguration {
                claude_executable: Some(executable),
                ..NativeAdapterConfiguration::default()
            },
            Engine::Codex => {
                let mut codex = CodexAdapterConfig::new(executable);
                if replaying {
                    codex.transport.request_timeout = ADAPTER_DEADLINE;
                }
                NativeAdapterConfiguration {
                    codex: Some(codex),
                    ..NativeAdapterConfiguration::default()
                }
            }
            Engine::Grok => {
                let mut grok = AcpAdapterConfig::builtin(BuiltinAcpProvider::Grok, executable)
                    .expect("grok profile");
                if replaying {
                    grok.prompt_timeout = ADAPTER_DEADLINE;
                    grok.transport.request_timeout = ADAPTER_DEADLINE;
                }
                NativeAdapterConfiguration {
                    acp: vec![grok],
                    ..NativeAdapterConfiguration::default()
                }
            }
        };
        builder
            .register_native_adapters(configuration.with_client_version("0.0.0-replay"))
            .expect("native adapter");
        let runtime = Arc::new(builder.build().expect("runtime"));

        let (pushes, receiver) = mpsc::channel(4_096);
        let claims = Arc::new(SessionClaims::new(Arc::new(NoPanes)));
        let threads = ThreadHost::new(ThreadHostConfig {
            store: ThreadHost::open_store(&state),
            ports: ThreadPorts {
                sessions: Arc::new(RuntimeThreadSessions(Arc::clone(&runtime))),
                hidden: Arc::new(Fakes),
                authority: Arc::new(Fakes),
                git: Arc::new(SystemThreadGit),
                pull_requests: Arc::new(Fakes),
                ship: Arc::new(Fakes),
                titles: Arc::new(Fakes),
                clock: Arc::new(interface_host::threads::now_ms),
                claims: Arc::clone(&claims),
                consults: Arc::new(Fakes),
            },
            pushes: Some(pushes.clone()),
            registry_pushes: None,
            registry: Arc::new(ThreadRegistrySignal::default()),
            journal: Some(ThreadJournalConfig {
                data_root: state.clone(),
                notices: None,
            }),
        });
        threads.start();
        let dispatcher = Arc::new(
            RuntimeHostDispatcher::with_pushes(Arc::clone(&runtime), pushes)
                .with_event_observer(Arc::new(threads.run_tap(Arc::new(NoObserver))))
                .with_session_claims(claims)
                .with_send_hooks(Arc::new(threads.clone())),
        );
        Self {
            _temp: temp,
            work,
            report,
            journal: state.join(interface_settings::thread_journal::JOURNAL_DIR),
            limit,
            runtime,
            dispatcher,
            threads,
            pushes: receiver,
            events: Vec::new(),
            read: 0,
            views: Vec::new(),
        }
    }

    /// What the replayer (or recorder) wrote to stderr: why a replay failed.
    pub(crate) fn provider_stderr(&self) -> String {
        std::fs::read_to_string(&self.report).unwrap_or_default()
    }

    fn fail(&self, message: &str) -> ! {
        let stderr = self.provider_stderr();
        let tail: Vec<String> = self
            .events
            .iter()
            .rev()
            .take(12)
            .rev()
            .map(|(key, event)| format!("  {key}: {}", kind_name(event)))
            .collect();
        panic!(
            "{message}\nprovider stderr:\n{}\nlast events:\n{}",
            if stderr.is_empty() {
                "  (empty)"
            } else {
                &stderr
            },
            tail.join("\n")
        );
    }

    async fn dispatch(&self, operation: &str, payload: Value) -> Value {
        match tokio::time::timeout(self.limit, self.dispatcher.dispatch(operation, payload)).await {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => self.fail(&format!("{operation} failed: {error:?}")),
            Err(_) => self.fail(&format!(
                "{operation} did not answer within {:?}",
                self.limit
            )),
        }
    }

    /// `createSession` as the composer sends it, then the thread bind.
    pub(crate) async fn start(&mut self, engine: Engine, prompt: &str) -> String {
        let response = self
            .dispatch(
                CHANNELS.create_session,
                json!({"engine": engine.id(), "options": engine.launch(&self.work, prompt)}),
            )
            .await;
        let key = response["key"]
            .as_str()
            .unwrap_or_else(|| self.fail(&format!("createSession answered {response}")))
            .to_owned();
        let bound = tokio::time::timeout(
            self.limit,
            self.threads.dispatch(
                CHANNELS.thread_control,
                json!({"kind": "bind", "key": key, "prompt": prompt}),
            ),
        )
        .await;
        if !matches!(bound, Ok(Ok(_))) {
            self.fail(&format!("the thread bind failed: {bound:?}"));
        }
        key
    }

    pub(crate) async fn send(&self, key: &str, text: &str) {
        self.dispatch(CHANNELS.send, json!({"sessionKey": key, "text": text}))
            .await;
    }

    pub(crate) async fn allow(&self, key: &str, approval_id: &str) {
        self.dispatch(
            CHANNELS.respond,
            json!({"sessionKey": key, "approvalId": approval_id, "verdict": "allow"}),
        )
        .await;
    }

    pub(crate) async fn interrupt(&self, key: &str) {
        self.dispatch(CHANNELS.interrupt, json!({"sessionKey": key}))
            .await;
    }

    pub(crate) async fn close(&self, key: &str) {
        self.dispatch(CHANNELS.close_session, json!({"sessionKey": key}))
            .await;
    }

    pub(crate) async fn resume(&self, key: &str) {
        self.dispatch(CHANNELS.resume_session, json!({"sessionKey": key}))
            .await;
    }

    /// Reads one push, keeping session events and thread views. A replayer
    /// that already reported a failure ends the wait at once instead of at
    /// the limit (checked once a second while nothing arrives; the check
    /// only shortens a failing wait, it never decides a passing one).
    async fn pump(&mut self, deadline: tokio::time::Instant, waiting_for: &str) {
        let frame = loop {
            let check = (tokio::time::Instant::now() + REPORT_CHECK).min(deadline);
            match tokio::time::timeout_at(check, self.pushes.recv()).await {
                Ok(Some(frame)) => break frame,
                Ok(None) => self.fail(&format!("the push queue closed waiting for {waiting_for}")),
                Err(_) if tokio::time::Instant::now() >= deadline => self.fail(&format!(
                    "nothing arrived within {:?} waiting for {waiting_for}",
                    self.limit
                )),
                Err(_) if !self.provider_stderr().trim().is_empty() => self.fail(&format!(
                    "the provider stopped while waiting for {waiting_for}"
                )),
                Err(_) => {}
            }
        };
        self.keep(frame);
    }

    /// Keeps a session event or a thread view from one push.
    fn keep(&mut self, frame: PushFrame) {
        if frame.operation == CHANNELS.event {
            let payload = frame.payload.into_value();
            let key = payload["key"].as_str().unwrap_or_default().to_owned();
            let event: EngineEvent =
                serde_json::from_value(payload["event"].clone()).expect("a session event");
            self.events.push((key, event.event));
        } else if frame.operation == CHANNELS.threads
            && let Ok(view) = frame.payload.decode()
        {
            self.views.push(view);
        }
    }

    /// The next event of `key` that `wanted` accepts. A failed status the
    /// scenario did not ask for fails at once.
    pub(crate) async fn next_event(
        &mut self,
        key: &str,
        waiting_for: &str,
        mut wanted: impl FnMut(&EngineEventKind) -> bool,
    ) -> EngineEventKind {
        let deadline = tokio::time::Instant::now() + self.limit;
        loop {
            while self.read < self.events.len() {
                let (event_key, event) = self.events[self.read].clone();
                self.read += 1;
                if event_key != key {
                    continue;
                }
                if wanted(&event) {
                    return event;
                }
                if matches!(
                    event,
                    EngineEventKind::StatusChanged {
                        status: SessionStatus::Failed
                    }
                ) {
                    self.fail(&format!("{key} failed waiting for {waiting_for}"));
                }
            }
            self.pump(deadline, waiting_for).await;
        }
    }

    /// Reads `key`'s events to the end of the turn in progress (idle after
    /// working), allowing each approval it asks for.
    pub(crate) async fn finish_turn(&mut self, key: &str, turn: &mut Turn) {
        let mut started = !turn.kinds.is_empty();
        loop {
            let event = self.next_event(key, "the end of the turn", |_| true).await;
            turn.kinds.push(kind_name(&event));
            match event {
                EngineEventKind::MessageDelta {
                    text,
                    agent_key: None,
                } => {
                    started = true;
                    turn.text.push_str(&text);
                }
                EngineEventKind::ToolCall {
                    tool,
                    agent_key: None,
                    ..
                } => turn.tool_calls.push(tool),
                EngineEventKind::ToolResult { .. } => turn.tool_results += 1,
                EngineEventKind::ApprovalRequested {
                    approval_id, tool, ..
                } => {
                    turn.approvals.push(tool);
                    self.allow(key, &approval_id).await;
                }
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Working | SessionStatus::Waiting,
                } => started = true,
                EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                } if started => return,
                _ => {}
            }
        }
    }

    /// The thread's latest pushed view reads finished: its last run ended
    /// completed, nothing runs, and it is unread. Views already pushed are
    /// read first, so a run the turn's trailing events opened is seen.
    pub(crate) fn assert_finished(&mut self, key: &str) {
        while let Ok(frame) = self.pushes.try_recv() {
            self.keep(frame);
        }
        let latest = self.latest_thread(key);
        assert!(
            ended(&latest, RunOutcome::Completed)
                && latest
                    .turns
                    .iter()
                    .all(|turn| turn.state != TurnState::Running)
                && latest.unread,
            "{key}: the thread does not read finished and unread: last run {:?}, unread {}",
            latest.last_run,
            latest.unread
        );
    }

    /// The first pushed view of thread `key` that `done` accepts.
    pub(crate) async fn thread_once(
        &mut self,
        key: &str,
        waiting_for: &str,
        done: impl Fn(&ThreadView) -> bool,
    ) -> ThreadView {
        let deadline = tokio::time::Instant::now() + self.limit;
        let mut seen = 0;
        loop {
            if let Some(thread) = self.views[seen..]
                .iter()
                .filter_map(|view| view.threads.iter().find(|thread| thread.key == key))
                .find(|thread| done(thread))
            {
                return thread.clone();
            }
            seen = self.views.len();
            self.pump(deadline, waiting_for).await;
        }
    }

    /// Thread `key` in the latest pushed view.
    pub(crate) fn latest_thread(&self, key: &str) -> ThreadView {
        self.views
            .iter()
            .rev()
            .find_map(|view| view.threads.iter().find(|thread| thread.key == key))
            .cloned()
            .unwrap_or_else(|| self.fail(&format!("no view holds thread {key}")))
    }

    /// The registry's status for `key`.
    pub(crate) fn status(&self, key: &str) -> SessionStatus {
        RuntimeThreadSessions(Arc::clone(&self.runtime))
            .session(key)
            .expect("registry")
            .unwrap_or_else(|| self.fail(&format!("{key} is not in the registry")))
            .status
    }

    /// Closes the host the way the app shuts down.
    /// Closes the host the way the app shuts down: providers, then the event
    /// pumps, then the journal drain (everything the pumps saw is written),
    /// then the thread worker. Answers what the provider wrote to stderr and
    /// each thread's journal as written.
    pub(crate) async fn shutdown(self) -> Teardown {
        let _ = tokio::time::timeout(self.limit, self.runtime.shutdown()).await;
        self.dispatcher.close_event_pumps().await;
        self.threads.drain_journal().await;
        self.threads.shutdown().await;
        let keys: BTreeSet<String> = self
            .views
            .iter()
            .flat_map(|view| view.threads.iter().map(|thread| thread.key.clone()))
            .collect();
        let journals = keys
            .into_iter()
            .map(|key| {
                let page = thread_journal::read(&self.journal, &key, None, 1_000);
                let watch = thread_journal::watch(&self.journal, &key);
                let entries = page.entries.into_iter().map(|line| line.entry).collect();
                (key, (entries, watch))
            })
            .collect();
        Teardown {
            stderr: self.provider_stderr(),
            journals,
        }
    }
}

/// What a scenario left behind.
pub(crate) struct Teardown {
    /// What the replayer (or recorder) wrote to stderr.
    pub(crate) stderr: String,
    /// Each thread's journal entries and its watch, after the drain.
    pub(crate) journals: BTreeMap<String, (Vec<JournalEntry>, JournalWatch)>,
}

fn kind_name(event: &EngineEventKind) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("kind").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| format!("{event:?}").chars().take(40).collect())
}

/// One prompt, one short answer.
pub(crate) async fn simple(host: &mut Host, engine: Engine) -> String {
    let key = host.start(engine, SIMPLE_PROMPT).await;
    let mut turn = Turn::default();
    host.finish_turn(&key, &mut turn).await;
    assert!(
        turn.text.to_lowercase().contains("ready"),
        "{key}: the answer was {:?}",
        turn.text
    );
    assert!(
        turn.approvals.is_empty() && turn.tool_calls.is_empty(),
        "{turn:?}"
    );
    assert_eq!(host.status(&key), SessionStatus::Idle);
    let thread = host
        .thread_once(&key, "the run's end", |thread| {
            ended(thread, RunOutcome::Completed)
        })
        .await;
    assert_eq!(thread.turns.len(), 1, "{thread:?}");
    assert_eq!(thread.turns[0].state, TurnState::Done, "{thread:?}");
    host.assert_finished(&key);
    host.close(&key).await;
    key
}

/// A prompt that makes the provider ask to run a harmless command, approved.
pub(crate) async fn tool_approval(host: &mut Host, engine: Engine) -> String {
    let key = host.start(engine, TOOL_PROMPT).await;
    let mut turn = Turn::default();
    host.finish_turn(&key, &mut turn).await;
    assert!(
        !turn.approvals.is_empty(),
        "{key}: no approval was asked: {turn:?}"
    );
    let position = |name: &str| turn.kinds.iter().position(|kind| kind == name);
    let asked = position("approval.requested").expect("approval.requested");
    let result = turn
        .kinds
        .iter()
        .rposition(|kind| kind == "tool.result")
        .unwrap_or_else(|| panic!("{key}: no tool.result after the approval: {turn:?}"));
    assert!(
        asked < result,
        "{key}: the tool ran before it was approved: {turn:?}"
    );
    assert!(
        turn.text.to_lowercase().contains("done"),
        "{key}: the answer was {:?}",
        turn.text
    );
    assert_eq!(host.status(&key), SessionStatus::Idle);
    let thread = host
        .thread_once(&key, "the run's end", |thread| {
            ended(thread, RunOutcome::Completed)
        })
        .await;
    assert_eq!(thread.turns[0].state, TurnState::Done, "{thread:?}");
    host.assert_finished(&key);
    host.close(&key).await;
    key
}

/// A longer answer, stopped mid-turn.
pub(crate) async fn interrupt(host: &mut Host, engine: Engine) -> String {
    let key = host.start(engine, INTERRUPT_PROMPT).await;
    host.next_event(&key, "the first words of the answer", |event| {
        matches!(
            event,
            EngineEventKind::MessageDelta {
                agent_key: None,
                ..
            }
        )
    })
    .await;
    host.interrupt(&key).await;
    let mut turn = Turn {
        kinds: vec!["message.delta".into()],
        ..Turn::default()
    };
    host.finish_turn(&key, &mut turn).await;
    assert!(
        !turn.text.contains("300"),
        "{key}: the answer ran to its end: {:?}",
        turn.text
    );
    if engine == Engine::Claude {
        // Claude answers an acknowledged stop with the stopped turn's own
        // end (its stream's stop, then a result; Claude Code 2.1.325's is an
        // `error_during_execution` with an `[ede_diagnostic]` error). That
        // tail ends the interrupted turn: it reopens no activity and raises
        // no error.
        let mut tail = Vec::new();
        loop {
            let event = host
                .next_event(&key, "the stopped turn's result", |_| true)
                .await;
            tail.push(kind_name(&event));
            if matches!(event, EngineEventKind::UsageUpdated { .. }) {
                break;
            }
        }
        assert_eq!(tail, ["usage.updated"], "{key}: the stopped turn's tail");
    }
    assert_eq!(host.status(&key), SessionStatus::Idle, "{turn:?}");
    let thread = host
        .thread_once(&key, "the interrupted run's end", |thread| {
            ended(thread, RunOutcome::Interrupted)
        })
        .await;
    assert_eq!(thread.turns.len(), 1, "{thread:?}");
    assert_eq!(thread.turns[0].state, TurnState::Interrupted, "{thread:?}");
    if engine == Engine::Claude {
        // No run after the stopped one: the thread still reads interrupted.
        let latest = host.latest_thread(&key);
        assert!(
            ended(&latest, RunOutcome::Interrupted),
            "{key}: a run opened after the stop: {:?}",
            latest.last_run
        );
    }
    host.close(&key).await;
    assert!(
        !host
            .events
            .iter()
            .any(|(event_key, event)| event_key == &key
                && matches!(event, EngineEventKind::Error { .. })),
        "{key}: the stop raised an error"
    );
    key
}

/// A second launch that resumes the first session and takes a second prompt.
pub(crate) async fn resume(host: &mut Host, engine: Engine) -> String {
    let key = host.start(engine, SIMPLE_PROMPT).await;
    let mut first = Turn::default();
    host.finish_turn(&key, &mut first).await;
    assert!(first.text.to_lowercase().contains("ready"), "{first:?}");
    host.thread_once(&key, "the first run's end", |thread| {
        ended(thread, RunOutcome::Completed)
    })
    .await;
    host.close(&key).await;

    host.resume(&key).await;
    host.send(&key, RESUME_PROMPT).await;
    let mut second = Turn::default();
    host.finish_turn(&key, &mut second).await;
    assert!(
        second.text.to_lowercase().contains("ready"),
        "{key}: the resumed session did not recall its first answer: {:?}",
        second.text
    );
    assert_eq!(host.status(&key), SessionStatus::Idle);
    let thread = host
        .thread_once(&key, "the second run's end", |thread| {
            thread.turns.len() == 2 && ended(thread, RunOutcome::Completed)
        })
        .await;
    assert!(
        thread
            .turns
            .iter()
            .all(|turn| turn.state == TurnState::Done),
        "{thread:?}"
    );
    host.assert_finished(&key);
    host.close(&key).await;
    key
}

/// The thread's last run ended with `outcome` and its last turn is over.
fn ended(thread: &ThreadView, outcome: RunOutcome) -> bool {
    thread
        .last_run
        .as_ref()
        .is_some_and(|run| run.ended_at.is_some() && run.outcome == Some(outcome))
        && thread
            .turns
            .last()
            .is_some_and(|turn| turn.state != TurnState::Running)
}

/// Runs `scenario` for `engine` against the provider and answers what the
/// provider wrote to stderr.
pub(crate) async fn run_scenario(
    engine: Engine,
    scenario: &str,
    provider: &Provider,
    limit: Duration,
) -> Teardown {
    let mut host = Host::new(engine, provider, limit);
    match scenario {
        "simple" => drop(simple(&mut host, engine).await),
        "tool-approval" => drop(tool_approval(&mut host, engine).await),
        "interrupt" => drop(interrupt(&mut host, engine).await),
        "resume" => drop(resume(&mut host, engine).await),
        other => panic!("unknown scenario {other}"),
    }
    host.shutdown().await
}

/// The committed transcript for `engine`'s `scenario`.
pub(crate) fn fixture(engine: Engine, scenario: &str) -> PathBuf {
    provider_replay::fixtures_directory()
        .join(engine.folder())
        .join(format!("{scenario}.json"))
}

/// Replays one fixture and fails on anything the replayer reported.
async fn replay(engine: Engine, scenario: &str) {
    let transcript = fixture(engine, scenario);
    assert!(
        transcript.is_file(),
        "{} is missing: record it with provider_record_live.rs",
        transcript.display()
    );
    let teardown = run_scenario(
        engine,
        scenario,
        &Provider::Replay { transcript },
        REPLAY_LIMIT,
    )
    .await;
    assert!(
        teardown.stderr.trim().is_empty(),
        "{} {scenario}: the replayer reported:\n{}",
        engine.folder(),
        teardown.stderr
    );
    if scenario == "simple" {
        assert_simple_journal(engine, &teardown);
    }
}

/// The `simple` thread's journal, after the drain: its prompt, its answer,
/// its turn's end, and no turn left open.
fn assert_simple_journal(engine: Engine, teardown: &Teardown) {
    let [(key, (entries, watch))] = teardown.journals.iter().collect::<Vec<_>>()[..] else {
        panic!("{}: expected one journalled thread", engine.folder());
    };
    let prompt = entries.iter().any(|entry| {
        matches!(entry, JournalEntry::TurnStarted { prompt, .. } if prompt == SIMPLE_PROMPT)
    });
    let answer = entries.iter().any(|entry| {
        matches!(entry, JournalEntry::Message { text, agent_key: None, .. }
            if text.to_lowercase().contains("ready"))
    });
    let ended = entries
        .iter()
        .any(|entry| matches!(entry, JournalEntry::TurnEnded { .. }));
    // The one run is the one turn: nothing after it opened and closed
    // another (an `interrupted` end at the session's close).
    let ends: Vec<&JournalEntry> = entries
        .iter()
        .filter(|entry| matches!(entry, JournalEntry::TurnEnded { .. }))
        .collect();
    assert!(
        matches!(
            ends[..],
            [JournalEntry::TurnEnded {
                outcome: RunOutcome::Completed,
                ..
            }]
        ),
        "{key}: the journal ends more than the one completed turn: {ends:?}"
    );
    assert!(
        prompt && answer && ended,
        "{key}: the journal lacks the prompt ({prompt}), the answer ({answer}) or the \
         turn's end ({ended}): {entries:?}"
    );
    assert!(!watch.turn_open, "{key}: the journal still has a turn open");
}

// Fakes for the ports a replay never reaches (no git workspace, no titles,
// no pull requests, no consults).

struct Fakes;
struct NoPanes;
struct NoObserver;

impl TerminalSessionBindings for NoPanes {
    fn bound_provider_sessions(&self) -> Vec<String> {
        Vec::new()
    }
}

impl SessionEventObserver for NoObserver {
    fn observe_session_event(&self, _: &str, _: &EngineEvent) {}
}

#[async_trait]
impl HiddenSessions for Fakes {
    async fn hidden_keys(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
}

#[async_trait]
impl FolderAuthority for Fakes {
    async fn may_open(&self, _: &str) -> Result<(), DispatchError> {
        Ok(())
    }
}

impl PullRequests for Fakes {
    fn lookup(&self, _: &Path, _: &str) -> PullRequestLookup {
        PullRequestLookup::Unavailable
    }
    fn view(&self, _: &Path, _: u64) -> PullRequestLookup {
        PullRequestLookup::Unavailable
    }
}

impl ShipTools for Fakes {
    fn gh(&self) -> Option<Gh> {
        None
    }
}

#[async_trait]
impl ThreadTitles for Fakes {
    fn available(&self) -> bool {
        false
    }
    async fn generate(&self, _: String, _: String) -> Result<String, String> {
        Err("titles are not generated in provider replay".into())
    }
}

#[async_trait]
impl ThreadConsults for Fakes {
    async fn resume(&self, _: &str) -> Result<(), DispatchError> {
        Err(DispatchError::new(
            "unavailable",
            "no consults here",
            false,
            true,
        ))
    }
    async fn submit(
        &self,
        _: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, DispatchError> {
        Err(DispatchError::new(
            "unavailable",
            "no consults here",
            false,
            true,
        ))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_simple_replays_through_the_runtime_and_threads() {
    replay(Engine::Claude, "simple").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_tool_approval_replays_through_the_runtime_and_threads() {
    replay(Engine::Claude, "tool-approval").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_interrupt_replays_through_the_runtime_and_threads() {
    replay(Engine::Claude, "interrupt").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_resume_replays_through_the_runtime_and_threads() {
    replay(Engine::Claude, "resume").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_simple_replays_through_the_runtime_and_threads() {
    replay(Engine::Codex, "simple").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_tool_approval_replays_through_the_runtime_and_threads() {
    replay(Engine::Codex, "tool-approval").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_interrupt_replays_through_the_runtime_and_threads() {
    replay(Engine::Codex, "interrupt").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_resume_replays_through_the_runtime_and_threads() {
    replay(Engine::Codex, "resume").await;
}
