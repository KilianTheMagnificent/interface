//! Live agent approval-gate probes (P1, P2, P4, P5) against the installed Claude Code and
//! Codex CLIs.
//!
//! Opt-in only. Every probe is `#[ignore]` and also returns early unless
//! `INTERFACE_LIVE_PROBES=1`, because each one spends provider quota. Each probe builds a real
//! `SessionRuntime` with the real adapters, an agent home created through `DesktopService`
//! (`interface_settings::agent_home` writes the marker, `AGENTS.md`, `CLAUDE.md` and
//! `MEMORY.md`), a `.claude/settings.local.json` that allows `Bash(git push:*)` and `Read`, and
//! a git repository in the home with a local bare remote. Every approval is denied; P2 also
//! records the real agent-policy decision for each Codex approval.
//!
//! Run from a plain terminal, not from inside a Claude Code session: its nesting markers turn
//! Claude's transcript saving off, so nothing could resume.
//!
//! `INTERFACE_LIVE_PROBES=1 cargo test -p interface-host --test suite agent_gate_live -- --ignored --nocapture --test-threads=1`
#![cfg(unix)]

use std::{
    env,
    future::Future,
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::Duration,
};

use engine_codex::{
    CODEX_EXECUTABLE_ENV, CodexAdapterConfig, CodexMethod, CodexTransport, CodexTransportLimits,
    GATED_DISABLED_FEATURES, InitializeParams, ThreadStartParams,
    codex_app_server_process_with_collaboration, codex_gated_feature_args,
    resolve_codex_executable,
};
use futures::FutureExt;
use interface_contracts::{
    agents::ApprovalFacts,
    engine::{
        EngineEventKind, EngineId, LaunchOptions, ResumeOptions, SandboxMode, SessionKind,
        SessionStatus,
    },
    ipc::CHANNELS,
};
use interface_desktop_services::{
    DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts,
    agent_policy::{self, AgentScope, PolicyDecision},
};
use interface_engine::{ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome};
use interface_registry::SessionRegistry;
use interface_runtime::{
    EngineHistoryStore, HistoryLimits, NativeAdapterConfiguration, RuntimeEventFrame,
    RuntimeServices, SessionOpenOutcome, SessionRuntime,
};
use interface_storage::PrivateDirectory;
use serde_json::json;
use tempfile::TempDir;

const ENABLE: &str = "INTERFACE_LIVE_PROBES";
/// Bound on a provider launch and on each turn.
const WAIT: Duration = Duration::from_secs(180);
const CLOSE_WAIT: Duration = Duration::from_secs(30);
const AGENT_NAME: &str = "Harbour Archivist";
const JOB: &str = "Your job is to catalogue lighthouse photographs for the harbour museum archive.";
/// Stems, so "harbor"/"harbour" and "catalog"/"catalogue" both count.
const JOB_WORDS: [&str; 4] = ["lighthouse", "harbo", "museum", "catalog"];
const FIRST_PROMPT: &str =
    "Read MEMORY.md and tell me your job in one sentence, then run `git push origin main`.";
const PUSH_NUDGE: &str = "Now run `git push origin main`.";
const RESUME_FIRST_PROMPT: &str = "Read MEMORY.md and tell me your job in one sentence.";
const RESUME_PROMPT: &str = "What was the first thing I asked you?";
const DENIAL: &str = "Denied by the Interface live probe.";
/// The desktop service's home directory, below the probe root.
const OWNER_HOME: &str = "owner-home";
/// Claude Code stamps its child processes with these; the host's Electron parent scrubs them.
const CLAUDE_NESTING_MARKERS: [&str; 9] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider probe: set INTERFACE_LIVE_PROBES=1 (spends Claude quota)"]
async fn p1_p4_claude_gate_asks_for_read_and_push_and_answers_from_its_files() {
    if !enabled("P1/P4 Claude") {
        return;
    }
    require_unnested_claude();
    let probe = Probe::new("claude");
    with_cleanup(&probe, claude_gate(&probe)).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider probe: set INTERFACE_LIVE_PROBES=1 (spends Codex quota)"]
async fn p2_codex_gate_verifies_its_sandbox_and_asks_for_push() {
    if !enabled("P2 Codex") {
        return;
    }
    let probe = Probe::new("codex");
    with_cleanup(&probe, codex_gate(&probe)).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live provider probe: set INTERFACE_LIVE_PROBES=1 (spends Claude quota)"]
async fn p5_claude_thread_resumes_behind_the_gate() {
    if !enabled("P5 Claude resume") {
        return;
    }
    require_unnested_claude();
    let probe = Probe::new("claude");
    with_cleanup(&probe, claude_resume(&probe)).await;
}

async fn claude_gate(probe: &Probe) {
    let (key, _, mut events) = probe.start("claude", FIRST_PROMPT).await;
    let mut turn = Turn::default();
    deny_through_turn(&probe.runtime, &key, &mut events, &mut turn).await;
    if !turn.asked_to_push() {
        probe.send(&key, PUSH_NUDGE).await;
        deny_through_turn(&probe.runtime, &key, &mut events, &mut turn).await;
    }
    println!("P1/P4 Claude reply:\n{}", turn.text);
    turn.assert_every_tool_call_asked(&key);
    assert!(
        turn.asked_to_read("MEMORY.md"),
        "P1: no Read approval for MEMORY.md although settings.local.json allows Read: {:?}",
        turn.approvals
    );
    assert!(
        turn.asked_to_push(),
        "P1: git push raised no Shell approval although settings.local.json allows it: {:?}",
        turn.approvals
    );
    probe.assert_remote_untouched("P1");
    assert!(
        mentions_job(&turn.text),
        "P4: Claude's reply does not name its job from CLAUDE.md"
    );
    println!("P1/P4 Claude: passed");
}

async fn codex_gate(probe: &Probe) {
    print_codex_gate_echo(&codex_executable(&probe.user_home), &probe.home).await;
    let (key, _, mut events) = probe.start("codex", FIRST_PROMPT).await;
    println!(
        "P2: gated Codex thread is ready, so thread/start confirmed approvalPolicy untrusted, \
         approvalsReviewer user, workspaceWrite with networkAccess and excludeSlashTmp, and a \
         writable root equal to {}",
        probe.home.display()
    );
    let mut turn = Turn {
        policy: Some(probe.policy_scope()),
        ..Turn::default()
    };
    deny_through_turn(&probe.runtime, &key, &mut events, &mut turn).await;
    if !turn.asked_to_push() {
        probe.send(&key, PUSH_NUDGE).await;
        deny_through_turn(&probe.runtime, &key, &mut events, &mut turn).await;
    }
    println!("P2/P4 Codex reply:\n{}", turn.text);
    assert!(
        turn.asked_to_push(),
        "P2: git push raised no Shell approval: {:?}",
        turn.approvals
    );
    assert_codex_policy_decisions(&turn);
    probe.assert_remote_untouched("P2");
    assert!(
        mentions_job(&turn.text),
        "P4: Codex's reply does not name its job from AGENTS.md"
    );
    println!("P2/P4 Codex: passed");
}

async fn claude_resume(probe: &Probe) {
    let (key, session_id, mut events) = probe.start("claude", RESUME_FIRST_PROMPT).await;
    let mut first = Turn::default();
    deny_through_turn(&probe.runtime, &key, &mut events, &mut first).await;
    println!("P5 first reply:\n{}", first.text);
    drop(events);
    probe.close(&key).await;

    let (resumed, mut events) = probe.resume("claude", &session_id).await;
    assert_eq!(resumed, key, "P5: the resume attached a different session");
    probe.send(&key, RESUME_PROMPT).await;
    let mut second = Turn::default();
    deny_through_turn(&probe.runtime, &key, &mut events, &mut second).await;
    println!("P5 resumed reply:\n{}", second.text);
    second.assert_every_tool_call_asked(&key);
    let reply = second.text.to_lowercase();
    assert!(
        reply.contains("memory.md") || mentions_job(&reply),
        "P5: the resumed thread does not recall the first request"
    );
    println!("P5 Claude resume: passed");
}

fn enabled(probe: &str) -> bool {
    let on = env::var(ENABLE).is_ok_and(|value| value == "1");
    if !on {
        println!("{probe}: skipped; set {ENABLE}=1 to run this live provider probe");
    }
    on
}

fn require_unnested_claude() {
    let set = CLAUDE_NESTING_MARKERS
        .iter()
        .filter(|name| env::var_os(name).is_some())
        .copied()
        .collect::<Vec<_>>();
    assert!(
        set.is_empty(),
        "run the Claude probes outside Claude Code: {set:?} are set, which turns transcript \
         saving off (or prefix the command with `env -u` for each)"
    );
}

fn mentions_job(text: &str) -> bool {
    let text = text.to_lowercase();
    JOB_WORDS.iter().any(|word| text.contains(word))
}

/// Runs `body`, then closes every session the probe opened and shuts the runtime down, even
/// when `body` panics.
async fn with_cleanup(probe: &Probe, body: impl Future<Output = ()>) {
    let outcome = AssertUnwindSafe(body).catch_unwind().await;
    probe.close_all().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

struct Probe {
    _temp: TempDir,
    root: PathBuf,
    user_home: PathBuf,
    home: PathBuf,
    remote: PathBuf,
    runtime: SessionRuntime,
    open: Mutex<Vec<String>>,
}

impl Probe {
    fn new(engine: &str) -> Self {
        let temp = tempfile::tempdir().expect("probe temp directory");
        // Canonical, so the home has no symlinked component (`/var` -> `/private/var`), as a
        // production home under `~/Interface Agents` has none.
        let root = std::fs::canonicalize(temp.path()).expect("canonical probe root");
        let user_home = PathBuf::from(env::var_os("HOME").expect("HOME"));
        let home = create_agent_home(&root, engine);
        std::fs::create_dir_all(home.join(".claude")).expect("home .claude folder");
        std::fs::write(
            home.join(".claude/settings.local.json"),
            json!({"permissions": {"allow": ["Bash(git push:*)", "Read"]}}).to_string(),
        )
        .expect("settings.local.json");
        let remote = root.join("remote.git");
        git(&root, &["init", "-q", "--bare", "remote.git"]);
        git(&home, &["init", "-q", "-b", "main"]);
        git(&home, &["add", "-A"]);
        git(&home, &["commit", "-q", "-m", "Agent home"]);
        git(
            &home,
            &[
                "remote",
                "add",
                "origin",
                remote.to_str().expect("remote path"),
            ],
        );
        // Control: the push the probes deny would otherwise succeed.
        git(&home, &["push", "--dry-run", "-q", "origin", "main"]);

        let runtime = real_runtime(&root.join("runtime"), &user_home, engine);
        println!("agent home: {}", home.display());
        Self {
            _temp: temp,
            root,
            user_home,
            home,
            remote,
            runtime,
            open: Mutex::new(Vec::new()),
        }
    }

    /// The scope `AgentHost` classifies this agent's approvals over: the home as its only
    /// root and the desktop service's home directory; `cwd` comes from each approval.
    fn policy_scope(&self) -> AgentScope {
        AgentScope {
            roots: vec![self.home.clone()],
            cwd: self.home.clone(),
            home_dir: self.root.join(OWNER_HOME),
        }
    }

    fn home_text(&self) -> String {
        self.home.to_str().expect("UTF-8 home").to_owned()
    }

    /// A gated agent thread exactly as `AgentHost` starts one: cwd the home, kind agent.
    async fn start(
        &self,
        engine: &str,
        prompt: &str,
    ) -> (String, String, BoundedEventStream<RuntimeEventFrame>) {
        let engine_id = EngineId::new(engine).expect("engine id");
        let options = LaunchOptions {
            cwd: self.home_text(),
            prompt: prompt.to_owned(),
            name: Some(AGENT_NAME.to_owned()),
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..LaunchOptions::default()
        };
        let outcome = tokio::time::timeout(WAIT, self.runtime.start_session(&engine_id, options))
            .await
            .expect("gated launch within the wait");
        let SessionOpenOutcome::Ready(session) = outcome else {
            panic!("gated {engine} launch did not become ready: {outcome:?}");
        };
        self.attach(session.key, session.session_id).await
    }

    /// A gated resume exactly as `AgentRuntimeGuard` rewrites one.
    async fn resume(
        &self,
        engine: &str,
        session_id: &str,
    ) -> (String, BoundedEventStream<RuntimeEventFrame>) {
        let engine_id = EngineId::new(engine).expect("engine id");
        let options = ResumeOptions {
            kind: Some(SessionKind::Agent),
            approval_gate: Some(true),
            ..ResumeOptions::default()
        };
        let outcome = tokio::time::timeout(
            WAIT,
            self.runtime.resume_session(
                &engine_id,
                session_id.to_owned(),
                self.home_text(),
                options,
            ),
        )
        .await
        .expect("gated resume within the wait");
        let SessionOpenOutcome::Ready(session) = outcome else {
            panic!("gated {engine} resume did not become ready: {outcome:?}");
        };
        assert_eq!(
            session.session_id, session_id,
            "resume changed the session id"
        );
        let (key, _, events) = self.attach(session.key, session.session_id).await;
        (key, events)
    }

    async fn attach(
        &self,
        key: String,
        session_id: String,
    ) -> (String, String, BoundedEventStream<RuntimeEventFrame>) {
        self.open.lock().expect("open sessions").push(key.clone());
        let events = self
            .runtime
            .take_session_events(&key)
            .await
            .expect("session events");
        println!("session ready: {key}");
        (key, session_id, events)
    }

    async fn send(&self, key: &str, text: &str) {
        let delivery = tokio::time::timeout(WAIT, self.runtime.send(key, text.to_owned()))
            .await
            .expect("send within the wait");
        assert!(
            delivery.outcome.is_confirmed(),
            "{key}: send was not confirmed: {:?}",
            delivery.outcome
        );
    }

    async fn close(&self, key: &str) {
        self.open
            .lock()
            .expect("open sessions")
            .retain(|open| open != key);
        let closed = tokio::time::timeout(CLOSE_WAIT, self.runtime.close_session(key)).await;
        println!(
            "closed {key}: {:?}",
            closed.map(|delivery| delivery.outcome)
        );
    }

    async fn close_all(&self) {
        let keys = std::mem::take(&mut *self.open.lock().expect("open sessions"));
        for key in keys {
            self.close(&key).await;
        }
        let shutdown = tokio::time::timeout(CLOSE_WAIT, self.runtime.shutdown()).await;
        println!("runtime shutdown: {shutdown:?}");
    }

    fn assert_remote_untouched(&self, probe: &str) {
        let refs = git(
            &self.root,
            &[
                "--git-dir",
                self.remote.to_str().expect("remote path"),
                "for-each-ref",
            ],
        );
        assert!(
            refs.trim().is_empty(),
            "{probe}: the bare remote received refs: {refs}"
        );
    }
}

/// Creates the agent and its home through `DesktopService`, the same path `AgentHost` uses.
fn create_agent_home(root: &Path, engine: &str) -> PathBuf {
    let owner_home = root.join(OWNER_HOME);
    std::fs::create_dir_all(owner_home.join("Desktop/Projects")).expect("owner home");
    let state = root.join("state");
    let desktop = DesktopService::open(
        DesktopServiceConfig {
            appearance_file: state.join("appearance.json"),
            acp_profiles_file: state.join("acp-profiles.json"),
            terminal_workspaces_file: state.join("terminal-workspaces.json"),
            home_directory: owner_home,
            customization_user_root: None,
            review_checkpoint_store_path: None,
        },
        DesktopServiceReviewPorts::default(),
    )
    .expect("desktop service");
    desktop.set_agents_root(root.join("Interface Probe Agents"));
    desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind": "saveAgent", "expectedRevision": 0, "profile": {
                "id": "probe", "name": AGENT_NAME, "engine": engine, "model": "", "effort": "",
                "instructions": JOB, "color": "sky", "shape": "star"
            }}),
        )
        .expect("save agent");
    let launch = desktop
        .prepare_agent_thread("probe")
        .expect("agent home and files");
    let home = PathBuf::from(launch.home);
    for file in [
        ".interface-agent.json",
        "AGENTS.md",
        "CLAUDE.md",
        "MEMORY.md",
    ] {
        assert!(home.join(file).is_file(), "generated {file} is missing");
    }
    home
}

/// A runtime with only the real adapter for `engine`, over private stores in `directory`.
fn real_runtime(directory: &Path, user_home: &Path, engine: &str) -> SessionRuntime {
    drop(PrivateDirectory::open_or_create(directory).expect("private runtime root"));
    let sessions = SessionRegistry::open(directory).expect("session registry");
    let history =
        EngineHistoryStore::open(directory, HistoryLimits::default()).expect("engine history");
    let mut builder = SessionRuntime::builder(RuntimeServices::new(sessions, history));
    let configuration = if engine == "codex" {
        NativeAdapterConfiguration {
            codex: Some(CodexAdapterConfig::new(codex_executable(user_home))),
            ..NativeAdapterConfiguration::default()
        }
    } else {
        NativeAdapterConfiguration {
            claude_executable: Some(claude_executable(user_home)),
            ..NativeAdapterConfiguration::default()
        }
    };
    builder
        .register_native_adapters(configuration)
        .expect("real adapter");
    builder.build().expect("runtime")
}

/// The host's lookup order, with the current `PATH` standing in for the login shell's.
fn claude_executable(user_home: &Path) -> PathBuf {
    env::var_os("CLAUDE_CODE_EXECUTABLE")
        .map(PathBuf::from)
        .into_iter()
        .chain(on_path("claude"))
        .chain([
            user_home.join(".local/bin/claude"),
            user_home.join(".claude/local/claude"),
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ])
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
        .expect("Claude Code executable (set CLAUDE_CODE_EXECUTABLE)")
}

fn codex_executable(user_home: &Path) -> PathBuf {
    resolve_codex_executable(
        user_home,
        env::var_os(CODEX_EXECUTABLE_ENV).map(PathBuf::from),
        on_path("codex"),
        Path::is_file,
    )
    .map(|resolved| resolved.path)
    .filter(|path| path.is_file())
    .expect("Codex executable (set CODEX_EXECUTABLE)")
}

fn on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_absolute() && candidate.is_file())
}

/// Runs git in `directory` without the owner's signing or hooks, and returns stdout.
fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Interface Probe",
            "-c",
            "user.email=probe@interface.invalid",
        ])
        .args(arguments)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[derive(Default)]
struct Turn {
    text: String,
    approvals: Vec<ApprovalFacts>,
    /// When set, every approval is also classified by the real agent policy over this scope.
    policy: Option<AgentScope>,
    decisions: Vec<(ApprovalFacts, PolicyDecision)>,
    /// Main-agent tools from `tool.call`, and from `approval.requested`.
    called: Vec<String>,
    asked: Vec<String>,
}

impl Turn {
    fn asked_to_push(&self) -> bool {
        self.approvals.iter().any(|facts| {
            matches!(facts, ApprovalFacts::Shell { command, .. } if command.contains("git push"))
        })
    }

    fn asked_to_read(&self, file: &str) -> bool {
        self.approvals.iter().any(|facts| {
            matches!(facts, ApprovalFacts::Read { paths } if paths.iter().any(|path| path.ends_with(file)))
        })
    }

    /// Claude's gate forces every tool call through `can_use_tool`, so every called tool asks.
    fn assert_every_tool_call_asked(&self, key: &str) {
        let unasked = self
            .called
            .iter()
            .filter(|tool| !self.asked.contains(tool))
            .collect::<Vec<_>>();
        assert!(
            unasked.is_empty(),
            "{key}: tools ran without approval.requested: {unasked:?}"
        );
    }
}

/// Classifies one approval with the real policy before it is answered, taking the scope's `cwd`
/// from the approval when it has one (the agent home in these probes).
fn classify_at_approval(facts: &ApprovalFacts, scope: &AgentScope) -> PolicyDecision {
    let mut scope = scope.clone();
    if let ApprovalFacts::Shell { cwd: Some(cwd), .. } = facts {
        scope.cwd = PathBuf::from(cwd);
    }
    agent_policy::classify(facts, &scope)
}

/// Codex reports each command inside its `/bin/zsh -lc` wrapper; after engine-codex unwraps it,
/// a plain `cat MEMORY.md` read (when the model issues one) is allowed and every `git push` asks
/// with `risky:git-push`, never `risky:interpreter`.
fn assert_codex_policy_decisions(turn: &Turn) {
    let shell = |facts: &ApprovalFacts| match facts {
        ApprovalFacts::Shell { command, .. } => Some(command.clone()),
        _ => None,
    };
    let mut pushes = 0;
    for (facts, decision) in &turn.decisions {
        let Some(command) = shell(facts) else {
            continue;
        };
        if command.contains("git push") {
            pushes += 1;
            assert!(
                !decision.is_allow() && decision.reason() == "risky:git-push",
                "P2: `{command}` was not asked as risky:git-push: {decision:?}"
            );
        } else if is_memory_read(&command) {
            assert!(
                decision.is_allow(),
                "P2: the MEMORY.md read `{command}` was not allowed: {decision:?}"
            );
            println!("P2: `{command}` was allowed ({})", decision.reason());
        }
    }
    assert!(
        pushes > 0,
        "P2: no git push decision was recorded: {:?}",
        turn.decisions
    );
    println!("P2: {pushes} git push approval(s) asked as risky:git-push");
}

/// `cat` of the home's `MEMORY.md` and nothing else, however the path is written.
fn is_memory_read(command: &str) -> bool {
    command.strip_prefix("cat ").is_some_and(|operand| {
        !operand.contains(['|', ';', '&', '<', '>', '$', '`', '\n'])
            && operand
                .trim()
                .trim_matches(['\'', '"'])
                .ends_with("MEMORY.md")
    })
}

/// Reads one turn to its end, denying every approval it raises.
async fn deny_through_turn(
    runtime: &SessionRuntime,
    key: &str,
    events: &mut BoundedEventStream<RuntimeEventFrame>,
    turn: &mut Turn,
) {
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut started = false;
    loop {
        let frame = match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(elapsed) => panic!("{key}: the turn did not finish within the wait: {elapsed}"),
            Ok(Err(error)) => panic!("{key}: event stream failed: {error:?}"),
            Ok(Ok(None)) => panic!("{key}: event stream ended during the turn"),
            Ok(Ok(Some(frame))) => frame,
        };
        let event = match frame {
            RuntimeEventFrame::Event { value } => value.event.event,
            RuntimeEventFrame::Terminal {
                reason, message, ..
            } => panic!("{key}: event stream terminated ({reason:?}): {message:?}"),
        };
        match event {
            EngineEventKind::MessageDelta {
                text,
                agent_key: None,
            } => turn.text.push_str(&text),
            EngineEventKind::ToolCall {
                tool,
                summary,
                agent_key: None,
                ..
            } => {
                println!("{key}: tool.call {tool}: {summary}");
                turn.called.push(tool);
            }
            EngineEventKind::ApprovalRequested {
                approval_id,
                tool,
                summary,
                input,
            } => {
                let facts = runtime.approval_facts(key, &tool, &input);
                println!("{key}: approval.requested {tool} ({summary}) -> {facts:?}; denying");
                if let Some(scope) = &turn.policy {
                    let decision = classify_at_approval(&facts, scope);
                    println!("{key}: policy {decision:?}");
                    turn.decisions.push((facts.clone(), decision));
                }
                let response = ApprovalResponse {
                    note: Some(DENIAL.to_owned()),
                    answers: Vec::new(),
                };
                let delivery = runtime
                    .respond_to_approval(key, approval_id, ApprovalVerdict::Deny, response)
                    .await;
                assert!(
                    delivery.outcome.is_confirmed(),
                    "{key}: the denial was not delivered: {:?}",
                    delivery.outcome
                );
                turn.asked.push(tool);
                turn.approvals.push(facts);
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Working | SessionStatus::Waiting,
            } => started = true,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } if started => return,
            EngineEventKind::StatusChanged {
                status: SessionStatus::Failed,
            } => panic!("{key}: the turn failed"),
            EngineEventKind::Error {
                message, retryable, ..
            } => {
                println!("{key}: provider error (retryable {retryable}): {message}");
            }
            EngineEventKind::SessionClosed => panic!("{key}: the session closed during the turn"),
            _ => {}
        }
    }
}

/// Prints what a Codex app-server echoes for the gate's `thread/start`, including the writable
/// roots, which the adapter verifies but does not surface. Informational; never fails a probe.
async fn print_codex_gate_echo(executable: &Path, home: &Path) {
    let mut spec = match codex_app_server_process_with_collaboration(executable, None) {
        Ok(spec) => spec,
        Err(error) => return println!("P2 echo: no app-server spec: {error}"),
    };
    // Spawn the gated app-server exactly as a gated agent launch does, so the config/read
    // features readback below reflects the disable flags the adapter appends.
    spec.args.extend(codex_gated_feature_args());
    let connection = match CodexTransport::spawn(&spec, &CodexTransportLimits::default()) {
        Ok(connection) => connection,
        Err(error) => return println!("P2 echo: app-server did not start: {error}"),
    };
    let mut inbound = connection.inbound;
    let drain = tokio::spawn(async move { while inbound.recv().await.is_some() {} });
    let transport = connection.transport;
    let initialize = InitializeParams::interface(interface_common::product_version!());
    match transport
        .request(CodexMethod::INITIALIZE, &initialize)
        .await
    {
        DeliveryOutcome::Confirmed(_) => {
            let home = home.to_string_lossy().into_owned();
            match transport
                .request(
                    CodexMethod::CONFIG_READ,
                    &json!({"cwd": home, "includeLayers": false}),
                )
                .await
            {
                DeliveryOutcome::Confirmed(response) => {
                    let features = response
                        .get("config")
                        .and_then(|config| config.get("features"));
                    let state = |feature: &str| {
                        let state = features
                            .and_then(|features| features.get(feature))
                            .map_or_else(|| "unreported".to_owned(), ToString::to_string);
                        format!("{feature}={state}")
                    };
                    let readback: Vec<String> =
                        GATED_DISABLED_FEATURES.iter().map(|f| state(f)).collect();
                    println!(
                        "P2 echo: gated feature readback (disabled): {}",
                        readback.join(" ")
                    );
                    // Kept on: Codex 0.154.0 runs commands through the code-mode host.
                    println!(
                        "P2 echo: gated feature readback (kept on): {}",
                        state("code_mode_host")
                    );
                }
                other => println!("P2 echo: config/read was not confirmed: {other:?}"),
            }
            let params = ThreadStartParams {
                cwd: home.clone(),
                model: None,
                service_tier: None,
                approval_policy: Some("untrusted".into()),
                approvals_reviewer: Some("user".into()),
                sandbox: Some(SandboxMode::WorkspaceWrite),
                config: Some(json!({"sandbox_workspace_write": {
                    "writable_roots": [home], "network_access": true, "exclude_slash_tmp": true
                }})),
            };
            match transport.request(CodexMethod::THREAD_START, &params).await {
                DeliveryOutcome::Confirmed(response) => println!(
                    "P2 echo: approvalPolicy {} approvalsReviewer {} sandbox {}",
                    response["approvalPolicy"], response["approvalsReviewer"], response["sandbox"]
                ),
                other => println!("P2 echo: thread/start was not confirmed: {other:?}"),
            }
        }
        other => println!("P2 echo: initialize was not confirmed: {other:?}"),
    }
    transport.shutdown();
    drain.abort();
}
