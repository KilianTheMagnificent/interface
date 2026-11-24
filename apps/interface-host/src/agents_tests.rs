use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use interface_contracts::agents::{
    AgentControlResponse, AgentDecision, AgentDecisionKind, AgentDeliveryOutcome, AgentRunStatus,
    ApprovalFacts,
};
use interface_contracts::engine::AgentAccess;
use interface_desktop_services::{DesktopServiceConfig, DesktopServiceReviewPorts};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use super::*;
use crate::dispatcher::{DEFINITELY_UNSENT_CODE, DispatchError, OUTCOME_UNKNOWN_CODE};

const STILL_CHECKING: &str =
    "Interface is still checking this request with the agent approval policy.";
const READ_ONLY_MESSAGE: &str =
    "Past conversations are read-only. Start a new conversation to continue.";

struct Quiet;
impl SessionEventObserver for Quiet {
    fn observe_session_event(&self, _: &str, _: &EngineEvent) {}
}

/// Provider facts keyed by tool name, the way an engine adapter would build them, the
/// registry's recorded working folders, and which providers a probe finds ready.
#[derive(Default)]
struct Facts {
    cwds: Mutex<Vec<(String, String)>>,
    unreadable: AtomicBool,
    /// For a `Slow` tool: signal on the first sender, then wait on the receiver.
    slow: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    /// Ready providers and their executables.
    ready: Mutex<Vec<(String, String)>>,
    /// Every provider probed, in order.
    probed: Mutex<Vec<String>>,
}
impl Facts {
    fn record(&self, session_key: &str, cwd: &str) {
        self.cwds
            .lock()
            .unwrap()
            .push((session_key.to_owned(), cwd.to_owned()));
    }
}
#[async_trait::async_trait]
impl ApprovalFactsSource for Facts {
    async fn ready_executable(&self, engine: &str) -> Option<String> {
        self.probed.lock().unwrap().push(engine.to_owned());
        self.ready
            .lock()
            .unwrap()
            .iter()
            .find(|(ready, _)| ready == engine)
            .map(|(_, executable)| executable.clone())
    }
    fn session_cwd(&self, session_key: &str) -> Result<Option<String>, String> {
        if self.unreadable.load(Ordering::SeqCst) {
            return Err("registry lock was poisoned".into());
        }
        Ok(self
            .cwds
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(key, _)| key == session_key)
            .map(|(_, cwd)| cwd.clone()))
    }
    fn approval_facts(&self, _session_key: &str, tool: &str, input: &Value) -> ApprovalFacts {
        let text = |key: &str| input[key].as_str().unwrap_or_default().to_owned();
        if tool == "Slow" {
            let slow = self.slow.lock().unwrap().take();
            if let Some((entered, release)) = slow {
                entered.send(()).unwrap();
                release.recv().unwrap();
            }
        }
        match tool {
            "Read" => ApprovalFacts::Read {
                paths: vec![text("path")],
            },
            "Bash" => ApprovalFacts::Shell {
                command: text("command"),
                cwd: None,
            },
            "WebSearch" => ApprovalFacts::WebSearch {},
            _ => ApprovalFacts::Unknown {
                tool: tool.to_owned(),
            },
        }
    }
}

/// A launch paused inside `createSession`, after its early events were observed.
struct Pause {
    reached: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

/// Records every runtime operation. `createSession` observes `early` events through the tap
/// before it answers, as a fast provider does before its key reaches the host.
#[derive(Default)]
struct Runtime {
    calls: Mutex<Vec<(String, Value)>>,
    creates: Mutex<VecDeque<Result<Value, DispatchError>>>,
    responds: Mutex<VecDeque<Result<Value, DispatchError>>>,
    early: Mutex<Vec<(String, EngineEvent)>>,
    pause: Mutex<Option<Pause>>,
    /// Holds the next `respond` until the sender fires.
    respond_gate: Mutex<Option<oneshot::Receiver<()>>>,
    tap: OnceLock<Arc<AgentEventTap>>,
}

impl Runtime {
    fn calls(&self, operation: &str) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(op, _)| op == operation)
            .map(|(_, payload)| payload.clone())
            .collect()
    }
    fn operations(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(op, _)| op.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl HostDispatcher for Runtime {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        self.calls
            .lock()
            .unwrap()
            .push((operation.to_owned(), payload));
        if operation == CHANNELS.create_session {
            let early = std::mem::take(&mut *self.early.lock().unwrap());
            if let Some(tap) = self.tap.get() {
                for (key, event) in &early {
                    tap.observe_session_event(key, event);
                }
            }
            let pause = self.pause.lock().unwrap().take();
            if let Some(pause) = pause {
                let _ = pause.reached.send(());
                let _ = pause.release.await;
            }
            return self
                .creates
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(json!({"key": THREAD})));
        }
        if operation == CHANNELS.respond {
            let gate = self.respond_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                let _ = gate.await;
            }
            return self
                .responds
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(Value::Null));
        }
        Ok(Value::Null)
    }
}

const THREAD: &str = "claude#thread";

struct World {
    temp: tempfile::TempDir,
    work: PathBuf,
    desktop: Arc<DesktopService>,
    runtime: Arc<Runtime>,
    facts: Arc<Facts>,
    host: Arc<AgentHost>,
    tap: Arc<AgentEventTap>,
    guard: AgentRuntimeGuard,
}

fn new_world() -> World {
    world_with(false)
}

/// `corrupt_store` leaves the agent store unreadable, so it never opens.
fn world_with(corrupt_store: bool) -> World {
    // Under /tmp, outside the refused system roots that every /var/folders temp dir falls under.
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let home = temp.path().join("home");
    let state = temp.path().join("state");
    let work = temp.path().join("work");
    for dir in [&home, &state, &work] {
        std::fs::create_dir_all(dir).unwrap();
    }
    if corrupt_store {
        std::fs::write(state.join("agents.v2.json"), b"{").unwrap();
    }
    let desktop = Arc::new(
        DesktopService::open(
            DesktopServiceConfig {
                appearance_file: state.join("appearance.json"),
                acp_profiles_file: state.join("acp.json"),
                terminal_workspaces_file: state.join("terminals.json"),
                home_directory: home.clone(),
                customization_user_root: None,
                review_checkpoint_store_path: None,
            },
            DesktopServiceReviewPorts::default(),
        )
        .unwrap(),
    );
    desktop.set_agents_root(home.join("Interface Agents"));
    if !corrupt_store {
        desktop
            .dispatch_blocking(
                CHANNELS.agent_control,
                json!({"kind":"saveAgent","expectedRevision":0,"profile":{
                    "id":"research","name":"Research","engine":"claude","model":"sonnet","effort":"",
                    "instructions":"Cite primary sources.","color":"sky","shape":"star"
                }}),
            )
            .unwrap();
    }
    let runtime = Arc::new(Runtime::default());
    let facts = Arc::new(Facts::default());
    let host = AgentHost::new(desktop.clone(), runtime.clone(), facts.clone());
    let tap = Arc::new(AgentEventTap {
        desktop: desktop.clone(),
        next: Arc::new(Quiet),
        host: host.clone(),
    });
    assert!(runtime.tap.set(tap.clone()).is_ok());
    let guard = AgentRuntimeGuard::new(runtime.clone(), host.clone());
    World {
        temp,
        work,
        desktop,
        runtime,
        facts,
        host,
        tap,
        guard,
    }
}

impl World {
    fn read(&self) -> AgentControlResponse {
        serde_json::from_value(self.desktop.agent_control_response().unwrap()).unwrap()
    }
    async fn start(&self, request_id: &str) -> Result<Value, DispatchError> {
        self.host
            .dispatch(
                CHANNELS.agent_control,
                json!({"kind":"startAgentThread","agentId":"research","text":"Summarize the project.","requestId":request_id}),
            )
            .await
    }
    fn home(&self) -> String {
        self.read().workspace.agents[0].home.clone().unwrap()
    }
    fn grant_work(&self) -> String {
        let revision = self.read().workspace.revision;
        let response = self
            .desktop
            .dispatch_blocking(
                CHANNELS.agent_control,
                json!({"kind":"saveAgentFolders","expectedRevision":revision,"agentId":"research","folders":[self.work.to_str().unwrap()]}),
            )
            .unwrap();
        response["agents"][0]["folders"][0]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn decisions(&self) -> Vec<AgentDecision> {
        let response = self
            .desktop
            .dispatch_blocking(
                CHANNELS.agent_control,
                json!({"kind":"readAgentDecisions","agentId":"research","limit":50}),
            )
            .unwrap();
        serde_json::from_value(response["decisions"].clone()).unwrap()
    }
    async fn respond(&self, approval_id: &str, verdict: &str) -> Result<Value, DispatchError> {
        self.guard
            .dispatch(
                CHANNELS.respond,
                json!({"sessionKey":THREAD,"approvalId":approval_id,"verdict":verdict}),
            )
            .await
    }
}

fn approval(approval_id: &str, tool: &str, input: &Value) -> EngineEvent {
    serde_json::from_value(json!({
        "kind":"approval.requested","approvalId":approval_id,"tool":tool,
        "summary":format!("{tool} request"),"input":input
    }))
    .unwrap()
}

fn event(value: Value) -> EngineEvent {
    serde_json::from_value(value).unwrap()
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn start_thread_creates_gated_session_in_home_and_binds_it() {
    let world = new_world();
    let response = world.start("thread-1").await.unwrap();
    let home = world.home();
    let creates = world.runtime.calls(CHANNELS.create_session);
    assert_eq!(creates.len(), 1);
    assert_eq!(
        creates[0],
        json!({"engine":"claude","options":{
            "cwd":home,"prompt":"Summarize the project.","name":"Research","kind":"agent",
            "model":"sonnet","approvalGate":true
        }})
    );
    assert_eq!(response["sessionKey"], THREAD);
    let parsed: AgentControlResponse = serde_json::from_value(response.clone()).unwrap();
    assert_eq!(
        parsed.workspace.threads["research"]
            .current
            .as_ref()
            .unwrap()
            .session_key,
        THREAD
    );
    assert_eq!(parsed.workspace.assignments[THREAD], "research");
    assert!(world.desktop.agent_binding(THREAD).unwrap().thread);

    let again = world.start("thread-1").await.unwrap();
    assert_eq!(again, response);
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
}

#[tokio::test]
async fn start_thread_passes_granted_folders_exactly_as_stored() {
    let world = new_world();
    let granted = world.grant_work();
    world.start("thread-1").await.unwrap();
    let options = &world.runtime.calls(CHANNELS.create_session)[0]["options"];
    assert_eq!(options["cwd"], world.home());
    assert_eq!(options["additionalDirectories"], json!([granted]));
}

#[tokio::test]
async fn start_thread_unsent_error_is_replay_safe_and_unknown_is_not() {
    let world = new_world();
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            DEFINITELY_UNSENT_CODE,
            "Claude refused the gate.",
            false,
            true,
        )));
    let error = world.start("thread-1").await.unwrap_err();
    assert!(error.replay_safe);
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    // A definitely-unsent refusal is never cached: the corrected retry with the same id proceeds.
    let response = world.start("thread-1").await.unwrap();
    assert_eq!(response["sessionKey"], THREAD);
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 2);

    let world = new_world();
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            OUTCOME_UNKNOWN_CODE,
            "Connection lost after send.",
            false,
            false,
        )));
    let error = world.start("thread-2").await.unwrap_err();
    assert!(!error.replay_safe);
    let again = world.start("thread-2").await.unwrap_err();
    assert_eq!(again.code, OUTCOME_UNKNOWN_CODE);
    assert!(!again.replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
    assert!(!world.read().workspace.threads.contains_key("research"));
}

#[tokio::test]
async fn start_thread_refusals_before_the_provider_are_replay_safe_and_not_cached() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    // A second thread is refused before any provider effect.
    let refused = world.start("thread-2").await.unwrap_err();
    assert!(refused.replay_safe, "{refused:?}");
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);

    // A missing key or a failed bind after a successful create is never unsent.
    let world = new_world();
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"unexpected": true})));
    let error = world.start("thread-3").await.unwrap_err();
    assert!(!error.replay_safe);
    assert_eq!(error.code, OUTCOME_UNKNOWN_CODE);
}

#[tokio::test]
async fn approval_allowed_by_policy_is_answered_once_and_logged() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    let home = world.home();
    world.tap.observe_session_event(
        THREAD,
        &approval("a1", "Read", &json!({"path": format!("{home}/notes.md")})),
    );
    eventually("auto answer", || {
        world.runtime.calls(CHANNELS.respond).len() == 1
            && world
                .decisions()
                .iter()
                .any(|d| d.outcome == AgentDeliveryOutcome::Sent)
    })
    .await;
    assert_eq!(
        world.runtime.calls(CHANNELS.respond),
        vec![json!({"sessionKey":THREAD,"approvalId":"a1","verdict":"allow"})]
    );
    let decisions = world.decisions();
    assert_eq!(decisions.len(), 2, "{decisions:?}");
    assert!(decisions.iter().all(|d| {
        d.decision == AgentDecisionKind::Allowed && d.approval_id == "a1" && d.reason == "read"
    }));
    assert_eq!(decisions[1].outcome, AgentDeliveryOutcome::Pending);
    assert_eq!(decisions[0].outcome, AgentDeliveryOutcome::Sent);
    assert!(world.read().pending_approvals.is_empty());
}

/// Web searches ask by default (a query can carry what the agent read); the
/// owner's "Search the web without asking" switch in the agent's task
/// settings lets them through in its ordinary conversation too, once its
/// first task is over and no task is active.
#[tokio::test]
async fn a_teammates_web_search_asks_unless_its_owner_switched_searching_on() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    let first = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    assert!(!first.spec.allow_web_search);
    world
        .desktop
        .settle_agent_work(
            "research",
            &first.id,
            AgentExecutionState::Completed,
            "Done",
        )
        .unwrap();
    assert!(world.desktop.active_agent_work(THREAD).unwrap().is_none());
    world.tap.observe_session_event(
        THREAD,
        &approval("s1", "WebSearch", &json!({"query": "notes"})),
    );
    eventually("asked", || !world.read().pending_approvals.is_empty()).await;
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
    assert_eq!(world.read().pending_approvals[0].reason, "webSearch");

    let read = world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"work","agentId":"research","command":{"action":"read"}}),
        )
        .unwrap();
    let mut spec = read["work"]["spec"].clone();
    spec["allowWebSearch"] = json!(true);
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"work","agentId":"research","command":{"action":"configure",
                "expectedRevision":read["work"]["revision"],"spec":spec}}),
        )
        .unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval("s2", "WebSearch", &json!({"query": "notes"})),
    );
    eventually("auto answer", || {
        world.runtime.calls(CHANNELS.respond).len() == 1
    })
    .await;
    assert_eq!(
        world.runtime.calls(CHANNELS.respond),
        vec![json!({"sessionKey":THREAD,"approvalId":"s2","verdict":"allow"})]
    );
    assert!(world.decisions().iter().any(|d| {
        d.approval_id == "s2"
            && d.decision == AgentDecisionKind::Allowed
            && d.reason == "webSearchAllowed"
    }));
    // The switch opens searches only: a risky command still asks.
    world.tap.observe_session_event(
        THREAD,
        &approval("s3", "Bash", &json!({"command": "git push origin main"})),
    );
    eventually("still asked", || {
        world
            .read()
            .pending_approvals
            .iter()
            .any(|pending| pending.approval_id == "s3")
    })
    .await;
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
}

#[tokio::test]
async fn approval_with_unknown_delivery_is_never_answered_again() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world
        .runtime
        .responds
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            OUTCOME_UNKNOWN_CODE,
            "Lost.",
            false,
            false,
        )));
    let home = world.home();
    world.tap.observe_session_event(
        THREAD,
        &approval("a1", "Read", &json!({"path": format!("{home}/notes.md")})),
    );
    eventually("uncertain outcome", || {
        world
            .decisions()
            .iter()
            .any(|d| d.outcome == AgentDeliveryOutcome::Unknown)
    })
    .await;
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
    assert!(world.read().pending_approvals.is_empty());
    let refused = world.respond("a1", "allow").await.unwrap_err();
    assert!(refused.replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
}

#[tokio::test]
async fn approval_risky_becomes_pending_and_generic_respond_is_owner_answer() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval("a2", "Bash", &json!({"command": "git push origin main"})),
    );
    eventually("pending approval", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
    let pending = world.read().pending_approvals;
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            pending[0].agent_id.as_str(),
            pending[0].session_key.as_str(),
            pending[0].approval_id.as_str()
        ),
        ("research", THREAD, "a2")
    );
    assert_eq!(pending[0].reason, "risky:git-push");
    let asked = world.decisions();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].decision, AgentDecisionKind::Asked);

    world.respond("a2", "allow").await.unwrap();
    assert_eq!(
        world.runtime.calls(CHANNELS.respond),
        vec![json!({"sessionKey":THREAD,"approvalId":"a2","verdict":"allow"})]
    );
    let decisions = world.decisions();
    assert_eq!(decisions[0].decision, AgentDecisionKind::OwnerAllowed);
    assert_eq!(decisions[0].outcome, AgentDeliveryOutcome::Sent);
    assert_eq!(
        decisions[0].facts,
        ApprovalFacts::Shell {
            command: "git push origin main".into(),
            cwd: None
        }
    );
    assert!(world.read().pending_approvals.is_empty());

    let second = world.respond("a2", "deny").await.unwrap_err();
    assert_eq!(second.code, DEFINITELY_UNSENT_CODE);
    assert!(second.replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
}

#[tokio::test]
async fn owner_answer_that_was_not_sent_stays_pending() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval("a2", "Bash", &json!({"command": "git push"})),
    );
    eventually("pending approval", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    world
        .runtime
        .responds
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            DEFINITELY_UNSENT_CODE,
            "Not sent.",
            false,
            true,
        )));
    let error = world.respond("a2", "deny").await.unwrap_err();
    assert!(error.replay_safe);
    assert_eq!(world.read().pending_approvals.len(), 1);
    world.respond("a2", "deny").await.unwrap();
    assert!(world.read().pending_approvals.is_empty());
    let decisions = world.decisions();
    assert_eq!(decisions[0].decision, AgentDecisionKind::OwnerDenied);
    assert_eq!(decisions[0].outcome, AgentDeliveryOutcome::Sent);
    assert_eq!(decisions[1].outcome, AgentDeliveryOutcome::Unsent);
}

#[tokio::test]
async fn respond_for_non_pending_agent_approval_is_refused() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    let error = world.respond("never-asked", "allow").await.unwrap_err();
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert_eq!(error.message, STILL_CHECKING);
    assert!(error.replay_safe && !error.retryable);
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());

    // A session no agent owns keeps the generic path.
    world
        .guard
        .dispatch(
            CHANNELS.respond,
            json!({"sessionKey":"codex#plain","approvalId":"x","verdict":"allow"}),
        )
        .await
        .unwrap();
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
}

#[tokio::test]
async fn approval_before_bind_is_buffered_then_handled() {
    let world = new_world();
    let home_hint = world
        .temp
        .path()
        .join("home/Interface Agents/Research/notes.md");
    world.runtime.early.lock().unwrap().extend([
        (
            THREAD.to_owned(),
            event(json!({"kind":"status.changed","status":"working"})),
        ),
        (
            THREAD.to_owned(),
            approval("early", "Read", &json!({"path": home_hint})),
        ),
    ]);
    world.start("thread-1").await.unwrap();
    // The outcome entry is the driver's last effect, after its one answer.
    eventually("buffered approval answered", || {
        world
            .decisions()
            .iter()
            .any(|d| d.outcome == AgentDeliveryOutcome::Sent)
    })
    .await;
    assert_eq!(
        world.runtime.calls(CHANNELS.respond),
        vec![json!({"sessionKey":THREAD,"approvalId":"early","verdict":"allow"})]
    );
}

#[tokio::test]
async fn thread_start_does_not_wait_for_drained_approvals() {
    let world = new_world();
    let home_hint = world
        .temp
        .path()
        .join("home/Interface Agents/Research/notes.md");
    world.runtime.early.lock().unwrap().push((
        THREAD.to_owned(),
        approval("early", "Read", &json!({"path": home_hint})),
    ));
    let (release, gate) = oneshot::channel();
    *world.runtime.respond_gate.lock().unwrap() = Some(gate);
    // The provider's answer is held, yet the start returns.
    tokio::time::timeout(Duration::from_secs(10), world.start("thread-1"))
        .await
        .expect("start held behind a drained approval")
        .unwrap();
    release.send(()).unwrap();
    eventually("drained approval answered", || {
        world
            .decisions()
            .iter()
            .any(|d| d.outcome == AgentDeliveryOutcome::Sent)
    })
    .await;
}

#[tokio::test]
async fn respond_for_an_approval_buffered_during_launch_is_refused() {
    let world = new_world();
    world.runtime.early.lock().unwrap().push((
        THREAD.to_owned(),
        approval("early", "Bash", &json!({"command": "git push"})),
    ));
    let (reached_tx, reached) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    *world.runtime.pause.lock().unwrap() = Some(Pause {
        reached: reached_tx,
        release: release_rx,
    });
    let host = world.host.clone();
    let starting = tokio::spawn(async move {
        host.dispatch(
            CHANNELS.agent_control,
            json!({"kind":"startAgentThread","agentId":"research","text":"Go.","requestId":"r"}),
        )
        .await
    });
    reached.await.unwrap();
    let error = world.respond("early", "allow").await.unwrap_err();
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert_eq!(error.message, STILL_CHECKING);
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
    release.send(()).unwrap();
    starting.await.unwrap().unwrap();
    eventually("buffered risky approval pending", || {
        world.read().pending_approvals.len() == 1
    })
    .await;
    world.respond("early", "deny").await.unwrap();
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
}

#[tokio::test]
async fn log_failure_asks_owner() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    // A file where the agent's decision directory belongs makes every append fail.
    let decisions = world.temp.path().join("state/agents/decisions");
    std::fs::create_dir_all(&decisions).unwrap();
    std::fs::write(decisions.join("7265736561726368"), b"not a directory").unwrap();
    let home = world.home();
    world.tap.observe_session_event(
        THREAD,
        &approval("a3", "Read", &json!({"path": format!("{home}/notes.md")})),
    );
    eventually("failed-closed approval pending", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
    let pending = world.read().pending_approvals;
    assert_eq!(pending[0].approval_id, "a3");
    assert_eq!(pending[0].reason, "policyError");
}

#[tokio::test]
async fn resume_of_agent_thread_injects_gate_and_directories() {
    let world = new_world();
    let granted = world.grant_work();
    world.start("thread-1").await.unwrap();
    world
        .guard
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey":THREAD,"approvalGate":false,"additionalDirectories":["/"]}),
        )
        .await
        .unwrap();
    world
        .guard
        .dispatch(CHANNELS.resume_session, json!({"sessionKey":"codex#plain"}))
        .await
        .unwrap();
    assert_eq!(
        world.runtime.calls(CHANNELS.resume_session),
        vec![
            json!({"sessionKey":THREAD,"approvalGate":true,"additionalDirectories":[granted]}),
            json!({"sessionKey":"codex#plain"}),
        ]
    );
    assert_eq!(
        world.host.gated_resume_directories(THREAD),
        Some(vec![granted])
    );
    assert_eq!(world.host.gated_resume_directories("codex#plain"), None);
}

const MESSAGE_IN_THREAD: &str = "Message this agent in its conversation instead.";

/// A renderer session started or forked in an agent's home would run ungated, then turn
/// read-only with its approvals refused; agents take new work in their thread.
#[tokio::test]
async fn sessions_created_or_forked_inside_the_agents_root_are_refused() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.runtime.calls.lock().unwrap().clear();
    let home = world.home();
    let agents_root = world.temp.path().join("home/Interface Agents");
    let linked_root = world.temp.path().join("agents-link");
    std::os::unix::fs::symlink(&agents_root, &linked_root).unwrap();
    let refused_cwds = [
        home.clone(),
        agents_root.to_string_lossy().into_owned(),
        format!("{home}/not-created-yet"),
        linked_root.join("Research").to_string_lossy().into_owned(),
    ];
    for cwd in &refused_cwds {
        let error = world
            .guard
            .dispatch(
                CHANNELS.create_session,
                json!({"engine":"claude","options":{"cwd":cwd,"prompt":"hi"}}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, DEFINITELY_UNSENT_CODE, "{cwd}");
        assert_eq!(error.message, MESSAGE_IN_THREAD);
        assert!(error.replay_safe);
    }
    world.facts.record("claude#old", &home);
    let error = world
        .guard
        .dispatch(CHANNELS.fork_session, json!({"sessionKey":"claude#old"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert_eq!(error.message, MESSAGE_IN_THREAD);
    assert!(error.replay_safe);
    assert!(world.runtime.calls.lock().unwrap().is_empty());

    // Sessions elsewhere are unaffected.
    let work = world.work.to_str().unwrap();
    world
        .guard
        .dispatch(
            CHANNELS.create_session,
            json!({"engine":"claude","options":{"cwd":work,"prompt":"hi"}}),
        )
        .await
        .unwrap();
    world.facts.record("codex#plain", work);
    world
        .guard
        .dispatch(CHANNELS.fork_session, json!({"sessionKey":"codex#plain"}))
        .await
        .unwrap();
    assert_eq!(
        world.runtime.operations(),
        vec![CHANNELS.create_session, CHANNELS.fork_session]
    );
    // The agent host's own launches in the home do not pass through the guard.
    let revision = world.read().workspace.revision;
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"newThread","expectedRevision":revision,"agentId":"research"}),
        )
        .await
        .unwrap();
    world.start("thread-2").await.unwrap();
    let creates = world.runtime.calls(CHANNELS.create_session);
    assert_eq!(creates.last().unwrap()["options"]["cwd"], json!(home));
}

#[tokio::test]
async fn desktop_names_agent_sessions_for_the_collaboration_broker() {
    use crate::collaboration::CollaborationAgentSessions;
    let world = new_world();
    let work = world.work.clone();
    assert!(!world.desktop.is_agent_session(THREAD, &work));
    world.start("thread-1").await.unwrap();
    let home = PathBuf::from(world.home());
    // The current thread, wherever its registry says it works.
    assert!(world.desktop.is_agent_session(THREAD, &home));
    assert!(world.desktop.is_agent_session(THREAD, &work));
    // Any session inside the agents root, bound or not.
    assert!(world.desktop.is_agent_session("codex#stray", &home));
    assert!(
        world
            .desktop
            .is_agent_session("codex#stray", &home.join("not-created-yet"))
    );
    // An ordinary session elsewhere is not an agent's.
    assert!(!world.desktop.is_agent_session("codex#plain", &work));
    // A past thread stays an agent's after the agent moves on.
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"newThread","expectedRevision":revision,"agentId":"research"}),
        )
        .unwrap();
    assert!(world.desktop.is_agent_session(THREAD, &work));
}

fn plain_effects(session_key: &str) -> Vec<(&'static str, Value)> {
    vec![
        (CHANNELS.resume_session, json!({"sessionKey":session_key})),
        (
            CHANNELS.send,
            json!({"sessionKey":session_key,"text":"more"}),
        ),
        (
            SEND_LIVE_OPERATION,
            json!({"sessionKey":session_key,"text":"more"}),
        ),
        (
            CHANNELS.set_model,
            json!({"sessionKey":session_key,"model":"opus"}),
        ),
        (
            CHANNELS.set_effort,
            json!({"sessionKey":session_key,"effort":"high"}),
        ),
        (
            CHANNELS.set_fast_mode,
            json!({"sessionKey":session_key,"enabled":true}),
        ),
        (
            CHANNELS.set_permission_mode,
            json!({"sessionKey":session_key,"mode":"bypassPermissions"}),
        ),
    ]
}

fn assert_read_only(error: &DispatchError) {
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert_eq!(error.message, READ_ONLY_MESSAGE);
    assert!(error.replay_safe);
}

#[tokio::test]
async fn past_thread_effects_are_refused_but_stops_and_reads_pass() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.facts.record(THREAD, &world.home());
    world
        .facts
        .record("codex#plain", world.work.to_str().unwrap());
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"newThread","expectedRevision":revision,"agentId":"research"}),
        )
        .unwrap();
    world.runtime.calls.lock().unwrap().clear();
    for (operation, payload) in plain_effects(THREAD) {
        let error = world.guard.dispatch(operation, payload).await.unwrap_err();
        assert_read_only(&error);
    }
    assert!(world.runtime.calls.lock().unwrap().is_empty());
    // Stops and history reads still reach a past thread.
    for (operation, payload) in [
        (CHANNELS.interrupt, json!({"sessionKey":THREAD})),
        (CHANNELS.close_session, json!({"sessionKey":THREAD})),
        (CHANNELS.read_history, json!({"sessionKey":THREAD})),
    ] {
        world.guard.dispatch(operation, payload).await.unwrap();
    }
    assert_eq!(
        world.runtime.operations(),
        vec![
            CHANNELS.interrupt,
            CHANNELS.close_session,
            CHANNELS.read_history
        ]
    );
    // A session outside the agents root is unaffected.
    for (operation, payload) in plain_effects("codex#plain") {
        world.guard.dispatch(operation, payload).await.unwrap();
    }
    assert_eq!(world.runtime.operations().len(), 3 + 7);
}

#[tokio::test]
async fn agent_home_sessions_are_read_only_without_the_store_or_the_past_list() {
    let world = world_with(true);
    assert!(world.desktop.agent_control_response().is_err());
    let old = "claude#old";
    let cwd = world.temp.path().join("home/Interface Agents/Research");
    std::fs::create_dir_all(&cwd).unwrap();
    world.facts.record(old, cwd.to_str().unwrap());
    world
        .facts
        .record("codex#plain", world.work.to_str().unwrap());
    let error = world
        .guard
        .dispatch(CHANNELS.resume_session, json!({"sessionKey":old}))
        .await
        .unwrap_err();
    assert_read_only(&error);
    assert_eq!(
        world.host.resume_requirement(old, cwd.to_str().unwrap()),
        ResumeRequirement::Refused(READ_ONLY_MESSAGE.into())
    );
    // An unanswered approval on it is refused too, never forwarded unguarded.
    let error = world
        .guard
        .dispatch(
            CHANNELS.respond,
            json!({"sessionKey":old,"approvalId":"x","verdict":"allow"}),
        )
        .await
        .unwrap_err();
    assert_read_only(&error);
    assert!(world.runtime.calls.lock().unwrap().is_empty());

    world
        .guard
        .dispatch(CHANNELS.resume_session, json!({"sessionKey":"codex#plain"}))
        .await
        .unwrap();
    assert_eq!(
        world
            .host
            .resume_requirement("codex#plain", world.work.to_str().unwrap()),
        ResumeRequirement::Unrestricted
    );

    // An unreadable registry refuses the effect rather than guessing.
    world.facts.unreadable.store(true, Ordering::SeqCst);
    let error = world
        .guard
        .dispatch(
            CHANNELS.send,
            json!({"sessionKey":"codex#plain","text":"x"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.send).len(), 0);
}

#[tokio::test]
async fn an_inline_send_whose_binding_lost_startup_events_is_not_sent() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    // The thread's first task is over, so the next send begins a task inline.
    let first = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    world
        .desktop
        .settle_agent_work(
            "research",
            &first.id,
            AgentExecutionState::Completed,
            "Done",
        )
        .unwrap();
    // Another agent is starting while more unbound events arrive than its buffer holds.
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveAgent","expectedRevision":world.read().workspace.revision,"profile":{
                "id":"second","name":"Second","engine":"claude","model":"sonnet","effort":"",
                "instructions":"Cite primary sources.","color":"sky","shape":"star"
            }}),
        )
        .unwrap();
    let other = world.temp.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    world
        .desktop
        .begin_agent_work("second", "starting", "Other goal", other.to_str().unwrap())
        .unwrap();
    let working: EngineEvent =
        serde_json::from_value(json!({"kind":"status.changed","status":"working"})).unwrap();
    for _ in 0..=4096 {
        world
            .desktop
            .observe_agent_work("codex#unbound", &working)
            .unwrap();
    }

    let sends = world.runtime.calls(CHANNELS.send).len();
    let error = world
        .guard
        .dispatch(
            CHANNELS.send,
            json!({"sessionKey":THREAD,"text":"Next goal"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, DEFINITELY_UNSENT_CODE);
    assert!(error.replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.send).len(), sends);
    let task = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    assert_ne!(task.id, first.id);
    assert!(task.stop_requested);
}

#[tokio::test]
async fn new_thread_and_provider_change_retire_the_old_session() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval("a2", "Bash", &json!({"command": "git push"})),
    );
    eventually("pending approval", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    let revision = world.read().workspace.revision;
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"newThread","expectedRevision":revision,"agentId":"research"}),
        )
        .await
        .unwrap();
    assert_eq!(
        world.runtime.calls(CHANNELS.close_session),
        vec![json!({"sessionKey":THREAD})]
    );
    assert!(world.read().pending_approvals.is_empty());

    let second = "claude#second";
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"key": second})));
    world.start("thread-2").await.unwrap();
    // Saving the profile unchanged keeps the thread.
    let profile = json!({"id":"research","name":"Research","engine":"claude","model":"sonnet",
        "effort":"","instructions":"Cite primary sources.","color":"sky","shape":"star"});
    let revision = world.read().workspace.revision;
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"saveAgent","expectedRevision":revision,"profile":profile}),
        )
        .await
        .unwrap();
    assert_eq!(world.runtime.calls(CHANNELS.close_session).len(), 1);
    let mut changed = profile;
    changed["engine"] = json!("codex");
    let revision = world.read().workspace.revision;
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"saveAgent","expectedRevision":revision,"profile":changed}),
        )
        .await
        .unwrap();
    assert_eq!(
        world.runtime.calls(CHANNELS.close_session),
        vec![json!({"sessionKey":THREAD}), json!({"sessionKey":second})]
    );
}

#[tokio::test]
async fn owner_answers_a_pending_approval_after_its_session_is_unbound() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.facts.record(THREAD, &world.home());
    world.tap.observe_session_event(
        THREAD,
        &approval("a2", "Bash", &json!({"command": "git push"})),
    );
    eventually("pending approval", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    // The store moves the thread to past without the host clearing its approvals.
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"newThread","expectedRevision":revision,"agentId":"research"}),
        )
        .unwrap();
    assert!(world.desktop.agent_binding(THREAD).is_none());
    world.respond("a2", "deny").await.unwrap();
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
    assert!(world.read().pending_approvals.is_empty());
    let decisions = world.decisions();
    assert_eq!(decisions[0].decision, AgentDecisionKind::OwnerDenied);
    assert_eq!(decisions[0].outcome, AgentDeliveryOutcome::Sent);
    // Without a pending entry the unbound agent-home session is read-only.
    let error = world.respond("a2", "deny").await.unwrap_err();
    assert_read_only(&error);
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
}

#[tokio::test]
async fn approvals_dropped_after_a_failed_bind_are_logged() {
    let world = new_world();
    // A key the store refuses to record, after the provider already started.
    let key = format!("claude#{}", "x".repeat(300));
    world.runtime.early.lock().unwrap().push((
        key.clone(),
        approval("early", "Bash", &json!({"command": "git push"})),
    ));
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({ "key": key })));
    let error = world.start("thread-1").await.unwrap_err();
    assert_eq!(error.code, OUTCOME_UNKNOWN_CODE);
    assert!(!world.host.is_buffered(&key));
    eventually("dropped approval logged", || !world.decisions().is_empty()).await;
    let decisions = world.decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].approval_id, "early");
    assert_eq!(decisions[0].decision, AgentDecisionKind::FailedClosed);
    assert_eq!(decisions[0].reason, "droppedBeforeBinding");
    assert_eq!(decisions[0].outcome, AgentDeliveryOutcome::Unsent);
    assert!(world.read().pending_approvals.is_empty());
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
}

#[tokio::test]
async fn launch_buffer_retains_unresolved_approvals_and_drops_closed_sessions() {
    let world = new_world();
    let (reached_tx, reached) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    *world.runtime.pause.lock().unwrap() = Some(Pause {
        reached: reached_tx,
        release: release_rx,
    });
    let host = world.host.clone();
    let starting = tokio::spawn(async move {
        host.dispatch(
            CHANNELS.agent_control,
            json!({"kind":"startAgentThread","agentId":"research","text":"Go.","requestId":"r"}),
        )
        .await
    });
    reached.await.unwrap();
    for n in 0..=64 {
        world.tap.observe_session_event(
            "codex#other",
            &approval(&format!("o{n}"), "Bash", &json!({"command": "ls"})),
        );
    }
    let ids = |world: &World| -> Vec<String> {
        lock(&world.host.buffered)
            .iter()
            .filter_map(|(_, event)| match &event.event {
                EngineEventKind::ApprovalRequested { approval_id, .. } => Some(approval_id.clone()),
                _ => None,
            })
            .collect()
    };
    let buffered = ids(&world);
    assert_eq!(buffered.len(), 65);
    assert_eq!((buffered[0].as_str(), buffered[64].as_str()), ("o0", "o64"));
    world.tap.observe_session_event(
        "codex#closing",
        &approval("c1", "Bash", &json!({"command": "ls"})),
    );
    world
        .tap
        .observe_session_event("codex#closing", &event(json!({"kind":"session.closed"})));
    assert!(!world.host.is_buffered("codex#closing"));
    assert_eq!(ids(&world).len(), 65);
    release.send(()).unwrap();
    starting.await.unwrap().unwrap();
    // No agent owns the other session: its approvals leave the buffer once no launch runs.
    assert!(ids(&world).is_empty());
}

// One worker: store work in an event pump must not hold every other task.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn store_work_in_a_pump_hands_the_worker_to_other_tasks() {
    let (entered, store_entered) = mpsc::channel();
    let (release, held) = mpsc::channel::<()>();
    let pump = tokio::spawn(async move {
        off_the_async_worker(|| {
            entered.send(()).unwrap();
            held.recv_timeout(Duration::from_secs(10)).is_ok()
        })
    });
    tokio::task::spawn_blocking(move || store_entered.recv())
        .await
        .unwrap()
        .unwrap();
    let other = tokio::spawn(async { "delivered" });
    let delivered = tokio::time::timeout(Duration::from_secs(5), other).await;
    release.send(()).unwrap();
    assert_eq!(
        delivered
            .expect("another task ran during the store work")
            .unwrap(),
        "delivered"
    );
    assert!(pump.await.unwrap());
    // A current-thread runtime has no worker to hand off: the work runs inline.
    assert_eq!(
        tokio::task::spawn_blocking(|| {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(async { off_the_async_worker(|| 7) })
        })
        .await
        .unwrap(),
        7
    );
}

// A commit holds the agent store across fsync. A send for any session waits for it
// off the async thread, so other sessions' work keeps running meanwhile.
#[test]
fn a_send_waits_for_the_agent_store_off_the_async_thread() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let world = new_world();
        let (held_tx, held) = mpsc::channel();
        let (release, release_rx) = mpsc::channel::<()>();
        let desktop = world.desktop.clone();
        let holder = std::thread::spawn(move || {
            let _store = desktop.hold_agent_store();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        held.recv().unwrap();
        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = tokio::spawn({
            let ticks = ticks.clone();
            async move {
                loop {
                    ticks.fetch_add(1, Ordering::SeqCst);
                    tokio::task::yield_now().await;
                }
            }
        });
        let started = std::time::Instant::now();
        let guard = world.guard;
        let sending = tokio::spawn(async move {
            guard
                .dispatch(
                    CHANNELS.send,
                    json!({"sessionKey":"claude#chat","text":"Hello"}),
                )
                .await
        });
        tokio::task::yield_now().await;
        let before = ticks.load(Ordering::SeqCst);
        while ticks.load(Ordering::SeqCst) < before + 50 {
            tokio::task::yield_now().await;
        }
        let ticked_within = started.elapsed();
        let still_waiting = !sending.is_finished();
        // The holder gives up after three seconds when a stalled runtime never releases it.
        let _ = release.send(());
        holder.join().unwrap();
        sending.await.unwrap().unwrap();
        ticker.abort();
        assert!(still_waiting, "the send waits for the held store");
        assert!(
            ticked_within < Duration::from_secs(2),
            "other tasks stalled for {ticked_within:?} behind the store"
        );
        assert_eq!(world.runtime.calls(CHANNELS.send).len(), 1);
    });
}

#[tokio::test]
async fn closed_sessions_are_remembered_newest_first_within_a_bound() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world
        .tap
        .observe_session_event(THREAD, &event(json!({"kind":"session.closed"})));
    for index in 0..2_000 {
        world.tap.observe_session_event(
            &format!("claude#chat-{index}"),
            &event(json!({"kind":"session.closed"})),
        );
    }
    let closed = lock(&world.host.closed);
    assert_eq!(closed.len(), RECENTLY_CLOSED);
    assert!(closed.contains("claude#chat-1999"));
    assert!(closed.contains("claude#chat-976"));
    assert!(!closed.contains("claude#chat-975"));
    assert!(!closed.contains(THREAD));
    drop(closed);

    // A repeated close does not push a newer key out.
    world
        .tap
        .observe_session_event("claude#chat-1999", &event(json!({"kind":"session.closed"})));
    assert!(lock(&world.host.closed).contains("claude#chat-976"));
}

#[tokio::test]
async fn automatic_answer_that_was_not_sent_goes_to_the_owner() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world
        .runtime
        .responds
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            DEFINITELY_UNSENT_CODE,
            "Not sent.",
            false,
            true,
        )));
    let home = world.home();
    world.tap.observe_session_event(
        THREAD,
        &approval("a5", "Read", &json!({"path": format!("{home}/notes.md")})),
    );
    eventually("unsent answer handed to the owner", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    let pending = world.read().pending_approvals;
    assert_eq!(pending[0].approval_id, "a5");
    assert_eq!(pending[0].reason, "read");
    let decisions = world.decisions();
    assert_eq!(decisions[0].outcome, AgentDeliveryOutcome::Unsent);
    world.respond("a5", "allow").await.unwrap();
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_that_closes_mid_check_gets_no_pending_approval_back() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    *world.facts.slow.lock().unwrap() = Some((entered_tx, release_rx));
    let host = world.host.clone();
    let checking = tokio::spawn(async move {
        host.handle_approval(THREAD, "slow", "Slow", "Slow request", &json!({}))
            .await;
    });
    tokio::task::spawn_blocking(move || entered.recv())
        .await
        .unwrap()
        .unwrap();
    world
        .tap
        .observe_session_event(THREAD, &event(json!({"kind":"session.closed"})));
    release.send(()).unwrap();
    checking.await.unwrap();
    assert!(world.read().pending_approvals.is_empty());
    assert_eq!(world.decisions()[0].decision, AgentDecisionKind::Asked);

    // A late approval cannot revive a closed generation. Only explicit resume can.
    world.tap.observe_session_event(
        THREAD,
        &approval("late", "Bash", &json!({"command":"git push"})),
    );
    assert!(world.read().pending_approvals.is_empty());
    let task = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    world.host.dispatch(CHANNELS.agent_control, json!({"kind":"work","agentId":"research","command":{"action":"acknowledge","taskId":task.id}})).await.unwrap();
    world
        .guard
        .dispatch(
            CHANNELS.resume_session,
            json!({"sessionKey":THREAD,"options":{}}),
        )
        .await
        .unwrap();
    world
        .guard
        .dispatch(
            CHANNELS.send,
            json!({"sessionKey":THREAD,"text":"Continue inspected work"}),
        )
        .await
        .unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval("again", "Bash", &json!({"command": "git push"})),
    );
    eventually("approval after resume pending", || {
        world.read().pending_approvals.len() == 1
    })
    .await;
}

#[test]
fn routine_folder_check_failures_are_told_apart() {
    assert_eq!(routine_folder(Some((true, true))), Ok(true));
    assert_eq!(routine_folder(Some((false, true))), Ok(false));
    assert_eq!(routine_folder(Some((false, false))), Err(ROUTINE_TOO_BROAD));
    assert_eq!(
        routine_folder(None),
        Err("Interface could not check this routine's folder. It was not run.")
    );
}

#[tokio::test]
async fn remove_agent_closes_thread_session_first() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    let revision = world.read().workspace.revision;
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"removeAgent","expectedRevision":revision,"id":"research"}),
        )
        .await
        .unwrap();
    assert_eq!(
        world.runtime.calls(CHANNELS.close_session),
        vec![json!({"sessionKey":THREAD})]
    );
    assert!(world.read().workspace.agents.is_empty());

    // A stale removal is refused by the store and leaves the thread attached.
    let world = new_world();
    world.start("thread-1").await.unwrap();
    let error = world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"removeAgent","expectedRevision":0,"id":"research"}),
        )
        .await
        .unwrap_err();
    assert!(error.replay_safe);
    assert!(world.runtime.calls(CHANNELS.close_session).is_empty());
    assert_eq!(world.read().workspace.agents.len(), 1);
}

#[tokio::test]
async fn session_closed_clears_pending_approvals() {
    let world = new_world();
    world.start("thread-1").await.unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval("a4", "Bash", &json!({"command": "git push"})),
    );
    eventually("pending approval", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    world
        .tap
        .observe_session_event(THREAD, &event(json!({"kind":"session.closed"})));
    assert!(world.read().pending_approvals.is_empty());
    let error = world.respond("a4", "allow").await.unwrap_err();
    assert_eq!(error.message, STILL_CHECKING);
}

fn save_routine(world: &World, id: &str, cwd: &str) {
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveRoutine","expectedRevision":revision,"routine":{
                "id":id,"agentId":"research","name":"Morning brief","prompt":"Summarize the project.",
                "cwd":cwd,"enabled":false,"startAtMs":1000,"intervalMinutes":null,"nextRunAtMs":null
            }}),
        )
        .unwrap();
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"runNow","expectedRevision":revision,"routineId":id,"requestId":id}),
        )
        .unwrap();
}

#[tokio::test]
async fn routine_run_launch_is_gated() {
    let world = new_world();
    let granted = world.grant_work();
    // The home folder exists once the agent has been messaged (revealing would open Finder).
    world.desktop.prepare_agent_thread("research").unwrap();
    let home = world.home();

    save_routine(&world, "in-home", &home);
    let run = world.desktop.claim_agent_run().unwrap().unwrap();
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"key":"claude#home-run"})));
    launch(&world.host, run).await;
    let options = world.runtime.calls(CHANNELS.create_session)[0]["options"].clone();
    assert_eq!(options["prompt"], "Summarize the project.");
    assert_eq!(options["cwd"], home);
    assert_eq!(options["approvalGate"], true);
    assert_eq!(options["kind"], "agent");
    assert_eq!(options["additionalDirectories"], json!([granted]));
    let id = world.read().workspace.runs[0].id.clone();
    world.tap.observe_session_event(
        "claude#home-run",
        &event(json!({"kind":"status.changed","status":"working"})),
    );
    world.tap.observe_session_event(
        "claude#home-run",
        &event(json!({"kind":"status.changed","status":"idle"})),
    );
    assert_eq!(
        world
            .read()
            .workspace
            .runs
            .iter()
            .find(|run| run.id == id)
            .unwrap()
            .status,
        AgentRunStatus::Succeeded
    );

    let project = world.work.join("project");
    std::fs::create_dir_all(&project).unwrap();
    save_routine(&world, "in-project", project.to_str().unwrap());
    let run = world.desktop.claim_agent_run().unwrap().unwrap();
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"key":"claude#project-run"})));
    launch(&world.host, run).await;
    let options = world.runtime.calls(CHANNELS.create_session)[1]["options"].clone();
    assert_eq!(
        options["prompt"],
        "Cite primary sources.\n\nSummarize the project."
    );
    assert_eq!(options["cwd"], project.to_str().unwrap());
    assert_eq!(options["approvalGate"], true);
    assert_eq!(options["additionalDirectories"], json!([home, granted]));
    assert!(options.get("permissionMode").is_none());
    assert!(options.get("sandboxMode").is_none());
    let binding = world.desktop.agent_binding("claude#project-run").unwrap();
    assert!(!binding.thread);
}

#[tokio::test]
async fn routine_in_an_ungrantable_folder_is_not_launched() {
    let world = new_world();
    let home_directory = world.temp.path().join("home");
    save_routine(&world, "too-broad", home_directory.to_str().unwrap());
    let run = world.desktop.claim_agent_run().unwrap().unwrap();
    launch(&world.host, run).await;
    assert!(world.runtime.calls(CHANNELS.create_session).is_empty());
    let run = &world.read().workspace.runs[0];
    assert_eq!(run.status, AgentRunStatus::Failed);
    assert_eq!(
        run.detail,
        "This routine's folder is too broad to run unattended. Choose a project folder."
    );
    assert!(Path::new(&home_directory).is_dir());
}

#[tokio::test]
async fn late_dispatcher_refuses_until_attached() {
    let late = LateDispatcher::default();
    let error = late
        .dispatch(CHANNELS.send, json!({"sessionKey":THREAD,"text":"x"}))
        .await
        .unwrap_err();
    assert!(error.replay_safe);
    let runtime = Arc::new(Runtime::default());
    assert!(late.attach(runtime.clone()));
    late.dispatch(CHANNELS.send, json!({"sessionKey":THREAD,"text":"x"}))
        .await
        .unwrap();
    assert_eq!(runtime.calls(CHANNELS.send).len(), 1);
}

/// The v14 fast-provider cases, now behind the gate in a grantable project folder.
struct Provider {
    desktop: Arc<DesktopService>,
    fail: bool,
}
#[async_trait::async_trait]
impl HostDispatcher for Provider {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        assert_eq!(operation, CHANNELS.create_session); // One prompt, never a second send.
        assert_eq!(
            payload["options"]["prompt"],
            "Cite primary sources.\n\nSummarize the project."
        );
        assert_eq!(payload["options"]["kind"], "agent");
        assert_eq!(payload["options"]["approvalGate"], true);
        assert!(payload["options"].get("permissionMode").is_none());
        assert!(payload["options"].get("sandboxMode").is_none());
        if self.fail {
            return Err(DispatchError::new(
                "unknown",
                "Connection lost after send.",
                false,
                false,
            ));
        }
        for status in ["working", "idle"] {
            self.desktop
                .observe_agent_event(
                    "codex#fast",
                    &event(json!({"kind":"status.changed","status":status})),
                )
                .unwrap();
        }
        Ok(json!({"key":"codex#fast"}))
    }
}
async fn exercise(fail: bool) -> AgentControlResponse {
    let world = new_world();
    let project = world.work.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/agent-workspace-v14.json"
    ))
    .unwrap();
    let mut routine = fixture["commands"][3].clone();
    routine["expectedRevision"] = json!(world.read().workspace.revision);
    routine["routine"]["cwd"] = json!(project.to_str().unwrap());
    world
        .desktop
        .dispatch_blocking(CHANNELS.agent_control, routine)
        .unwrap();
    let mut run_now = fixture["commands"][4].clone();
    run_now["expectedRevision"] = json!(world.read().workspace.revision);
    world
        .desktop
        .dispatch_blocking(CHANNELS.agent_control, run_now)
        .unwrap();
    let run = world.desktop.claim_agent_run().unwrap().unwrap();
    let host = AgentHost::new(
        world.desktop.clone(),
        Arc::new(Provider {
            desktop: world.desktop.clone(),
            fail,
        }),
        Arc::new(Facts::default()),
    );
    launch(&host, run).await;
    world.read()
}
#[tokio::test]
async fn fast_provider_completion_is_bound_and_recorded_without_a_second_prompt() {
    let state = exercise(false).await;
    assert_eq!(state.workspace.runs[0].status, AgentRunStatus::Succeeded);
    assert_eq!(state.workspace.assignments["codex#fast"], "research");
}
#[tokio::test]
async fn ambiguous_launch_is_never_replayed_or_reported_as_unsent() {
    let state = exercise(true).await;
    assert_eq!(state.workspace.runs[0].status, AgentRunStatus::Unknown);
    assert!(state.workspace.runs[0].session_key.is_none());
}

#[test]
fn the_scheduler_waits_for_the_next_due_run_within_its_bounds() {
    assert_eq!(claim_wait(None), RUNNER_MAX_WAIT, "nothing scheduled");
    assert_eq!(
        claim_wait(Some(0)),
        RUNNER_MIN_WAIT,
        "a due routine whose agent is busy cannot spin"
    );
    assert_eq!(claim_wait(Some(10_000)), Duration::from_secs(10));
    assert_eq!(claim_wait(Some(3_600_000)), RUNNER_MAX_WAIT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_now_request_wakes_the_sleeping_scheduler() {
    let world = new_world();
    let project = world.work.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveRoutine","expectedRevision":revision,"routine":{
                "id":"woken","agentId":"research","name":"Morning brief","prompt":"Summarize the project.",
                "cwd":project,"enabled":false,"startAtMs":1000,"intervalMinutes":null,"nextRunAtMs":null
            }}),
        )
        .unwrap();
    let runner = AgentRunner::start(world.host.clone());
    // With nothing due, the startup claim leaves the scheduler asleep for a minute.
    tokio::time::sleep(Duration::from_millis(200)).await;
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"key":"claude#woken-run"})));
    let revision = world.read().workspace.revision;
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"runNow","expectedRevision":revision,"routineId":"woken","requestId":"woken"}),
        )
        .await
        .unwrap();
    assert_eq!(
        world.read().workspace.runs[0].status,
        AgentRunStatus::Queued
    );
    // The command wakes it; the claim still respects the two-second floor.
    for _ in 0..100 {
        if world.read().workspace.runs[0].status != AgentRunStatus::Queued {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_ne!(
        world.read().workspace.runs[0].status,
        AgentRunStatus::Queued,
        "the woken scheduler claimed the run"
    );
    runner.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_budget_watch_sleeps_until_a_task_is_admitted_and_then_enforces_its_budget() {
    use interface_contracts::agent_work::AgentWorkSpec;
    async fn until(what: &str, check: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let world = new_world();
    let mut spec = AgentWorkSpec::default();
    spec.budget.max_actions = 1;
    let read = world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"work","agentId":"research","command":{"action":"read"}}),
        )
        .unwrap();
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"work","agentId":"research","command":{"action":"configure",
                "expectedRevision":read["work"]["revision"],"spec":spec}}),
        )
        .unwrap();
    let scans = || world.host.budget_scans.load(Ordering::SeqCst);
    let runner = AgentRunner::start(world.host.clone());
    // The check a second after startup finds no task under a budget, and the
    // watch then sleeps instead of checking every second.
    until("the startup budget check", || scans() == 1).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(scans(), 1, "an idle host keeps checking budgets");
    // An admission wakes it; the task's first action exhausts its budget.
    world.start("budgeted").await.unwrap();
    let task = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    world.tap.observe_session_event(
        THREAD,
        &event(json!({"kind":"tool.call","callId":"1","tool":"Read","summary":"Read","input":{}})),
    );
    until("the budget stop", || {
        world
            .desktop
            .agent_work_record("research", &task.id)
            .unwrap()
            .stop_requested
    })
    .await;
    runner.shutdown().await;
}

#[tokio::test]
async fn unknown_thread_start_is_not_reissued_after_host_reconstruction() {
    let world = new_world();
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            OUTCOME_UNKNOWN_CODE,
            "Synthetic delivery unknown",
            false,
            false,
        )));
    assert!(!world.start("audit-unknown").await.unwrap_err().replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
    let reconstructed = AgentHost::new(
        world.desktop.clone(),
        world.runtime.clone(),
        world.facts.clone(),
    );
    reconstructed
        .dispatch(
            CHANNELS.agent_control,
            json!({
                "kind":"startAgentThread", "agentId":"research", "text":"Summarize the project.",
                "requestId":"audit-unknown"
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
}

#[tokio::test]
async fn every_pending_approval_remains_answerable_above_old_limit() {
    let world = new_world();
    world.start("audit-start").await.unwrap();
    for n in 0..65 {
        world
            .host
            .handle_approval(
                THREAD,
                &format!("audit-{n}"),
                "Bash",
                "Synthetic approval",
                &json!({"command":"git push"}),
            )
            .await;
    }
    let pending = world.read().pending_approvals;
    assert_eq!(pending.len(), 65);
    assert!(pending.iter().any(|p| p.approval_id == "audit-0"));
    world.respond("audit-0", "deny").await.unwrap();
    assert_eq!(world.runtime.calls(CHANNELS.respond).len(), 1);
    world.respond("audit-64", "deny").await.unwrap();
}

#[tokio::test]
async fn folder_removal_closes_provider_before_narrowing_saved_grants() {
    let world = new_world();
    let work = world.grant_work();
    world.start("audit-start").await.unwrap();
    assert_eq!(
        world.runtime.calls(CHANNELS.create_session)[0]["options"]["additionalDirectories"],
        json!([work])
    );
    world.runtime.calls.lock().unwrap().clear();
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({
                "kind":"saveAgentFolders", "expectedRevision":world.read().workspace.revision,
                "agentId":"research", "folders":[]
            }),
        )
        .await
        .unwrap();
    assert_eq!(world.runtime.operations(), vec![CHANNELS.close_session]);
    assert!(
        !world
            .desktop
            .agent_binding(THREAD)
            .unwrap()
            .roots
            .contains(&PathBuf::from(work))
    );
}

#[tokio::test]
async fn agent_host_mcp_has_exact_tools_one_attempt_mutations_and_stop_fencing() {
    use interface_contracts::engine::AgentToolsProvider;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let world = new_world();
    world.start("mcp-parent").await.unwrap();
    let tools = crate::agent_tools::AgentToolServers::new(&world.host);
    let descriptor = tools.descriptor_for(Path::new(&world.home())).unwrap();
    let stream = tokio::net::UnixStream::connect(&descriptor.args[1])
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = tokio::io::BufReader::new(reader).lines();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"notification_request","arguments":{"requestId":"note","text":"Owner note"}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"notification_request","arguments":{"requestId":"note","text":"Owner note"}}}),
    ];
    for request in requests {
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(2), reader.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        assert!(response.get("error").is_none(), "{response}");
        if request["id"] == 2 {
            assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 8);
        } else if request["id"] == 3 || request["id"] == 4 {
            assert_ne!(response["result"]["isError"], true, "{response}");
        }
    }
    let task = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    assert_eq!(task.notes.len(), 1);
    world
        .host
        .stop_work("research", &task.id, "Stop test")
        .await
        .unwrap();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"notification_request\",\"arguments\":{\"requestId\":\"late\",\"text\":\"Must refuse\"}}}\n").await.unwrap();
    let response: Value =
        serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        world
            .desktop
            .agent_work_record("research", &task.id)
            .unwrap()
            .notes
            .len(),
        1
    );
}

async fn call_scoped_status(
    descriptor: &interface_contracts::engine::CollaborationMcpStdioDescriptor,
) -> Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let stream = tokio::net::UnixStream::connect(&descriptor.args[1])
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(reader).lines();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}\n").await.unwrap();
    lines.next_line().await.unwrap().unwrap();
    writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"notification_request\",\"arguments\":{\"requestId\":\"resumed-note\",\"text\":\"Scoped note\"}}}\n").await.unwrap();
    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn agent_resume_tools_are_idle_until_new_work_and_old_generations_stay_revoked() {
    use interface_contracts::{agent_work::AgentExecutionState, engine::AgentToolsProvider};
    let world = new_world();
    world.start("resume-tools").await.unwrap();
    let first = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    world
        .desktop
        .settle_agent_work(
            "research",
            &first.id,
            AgentExecutionState::Completed,
            "Completed",
        )
        .unwrap();
    world
        .host
        .stop_work("research", &first.id, "Close completed conversation")
        .await
        .unwrap();
    assert_eq!(
        world
            .desktop
            .agent_work_record("research", &first.id)
            .unwrap()
            .execution,
        AgentExecutionState::Completed
    );
    let tools = crate::agent_tools::AgentToolServers::new(&world.host);
    assert!(tools.descriptor_for(Path::new(&world.home())).is_none());
    world.host.prepare_resumed_host_tools(THREAD).unwrap();
    assert!(
        tools
            .descriptor_for_resume(Path::new(&world.home()), "claude#other")
            .is_none()
    );
    let resumed = tools
        .descriptor_for_resume(Path::new(&world.home()), THREAD)
        .unwrap();
    assert_eq!(
        call_scoped_status(&resumed).await["result"]["isError"],
        true
    );
    world
        .desktop
        .begin_agent_work("research", "new-turn", "New bounded goal", &world.home())
        .unwrap();
    world.desktop.bind_agent_work("new-turn", THREAD).unwrap();
    let response = call_scoped_status(&resumed).await;
    assert_ne!(response["result"]["isError"], true, "{response}");
    assert!(world.host.resumed_host_tools_admit(THREAD));
    assert_eq!(
        world
            .desktop
            .agent_work_record("research", "new-turn")
            .unwrap()
            .notes,
        vec!["Agent request: Scoped note"]
    );
    world
        .host
        .stop_work("research", "new-turn", "Stop resumed work")
        .await
        .unwrap();
    assert_eq!(
        call_scoped_status(&resumed).await["result"]["isError"],
        true
    );
    world.host.prepare_resumed_host_tools(THREAD).unwrap();
    world
        .desktop
        .begin_agent_work(
            "research",
            "third-turn",
            "Third bounded goal",
            &world.home(),
        )
        .unwrap();
    world.desktop.bind_agent_work("third-turn", THREAD).unwrap();
    assert_eq!(
        call_scoped_status(&resumed).await["result"]["isError"],
        true
    );
    let latest = tools
        .descriptor_for_resume(Path::new(&world.home()), THREAD)
        .unwrap();
    assert_ne!(call_scoped_status(&latest).await["result"]["isError"], true);
}

#[tokio::test]
async fn a_codex_interpreter_approval_cannot_silently_restore_revoked_write_access() {
    let world = new_world();
    world.start("opaque-command").await.unwrap();
    world.tap.observe_session_event(
        THREAD,
        &approval(
            "opaque",
            "Bash",
            &json!({"command":"python3 -c 'print(1)'"}),
        ),
    );
    eventually("opaque command asks its owner", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
    assert_eq!(
        world.decisions().last().unwrap().reason,
        "opaqueCommandNeedsOwner"
    );
}

fn set_permission_mode(world: &World, mode: &str) {
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"savePermissionMode","agentId":"research","mode":mode}),
        )
        .unwrap();
}

#[tokio::test]
async fn auto_and_bypass_agents_launch_threads_routines_and_resumes_with_their_access() {
    for mode in ["auto", "bypass"] {
        let world = new_world();
        set_permission_mode(&world, mode);
        world.start("thread-1").await.unwrap();
        let options = world.runtime.calls(CHANNELS.create_session)[0]["options"].clone();
        assert_eq!(options["agentAccess"], mode);
        assert!(options.get("approvalGate").is_none(), "{options}");
        assert_eq!(options["kind"], "agent");
        let access = if mode == "auto" {
            AgentAccess::Auto
        } else {
            AgentAccess::Bypass
        };
        let home = world.home();
        assert_eq!(
            world.host.resume_requirement(THREAD, &home),
            ResumeRequirement::Access(access, Vec::new())
        );
        // A later mode change reaches the next resume, not the live session.
        set_permission_mode(&world, "gated");
        assert_eq!(
            world.host.resume_requirement(THREAD, &home),
            ResumeRequirement::Gated(Vec::new())
        );

        // Routines follow the same setting (a fresh agent: a live thread holds claims).
        let world = new_world();
        set_permission_mode(&world, mode);
        world.desktop.prepare_agent_thread("research").unwrap();
        let home = world.home();
        save_routine(&world, "in-home", &home);
        let run = world.desktop.claim_agent_run().unwrap().unwrap();
        world
            .runtime
            .creates
            .lock()
            .unwrap()
            .push_back(Ok(json!({"key":"claude#home-run"})));
        launch(&world.host, run).await;
        let options = world.runtime.calls(CHANNELS.create_session)[0]["options"].clone();
        assert_eq!(options["agentAccess"], mode);
        assert!(options.get("approvalGate").is_none(), "{options}");
    }
}

#[tokio::test]
async fn provider_mode_agents_ask_the_owner_instead_of_interface_policy() {
    for (mode, reason) in [("auto", "providerAuto"), ("bypass", "bypass")] {
        let world = new_world();
        set_permission_mode(&world, mode);
        world.start("thread-1").await.unwrap();
        let home = world.home();
        // The gated policy would allow this in-scope read; the provider escalated it.
        world.tap.observe_session_event(
            THREAD,
            &approval("a1", "Read", &json!({"path": format!("{home}/notes.md")})),
        );
        eventually("owner approval", || {
            !world.read().pending_approvals.is_empty()
        })
        .await;
        assert!(world.runtime.calls(CHANNELS.respond).is_empty());
        let pending = world.read().pending_approvals;
        assert_eq!(pending[0].reason, reason);
        let decisions = world.decisions();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].decision, AgentDecisionKind::Asked);
        assert_eq!(decisions[0].reason, reason);
        world.respond("a1", "allow").await.unwrap();
        assert_eq!(
            world.runtime.calls(CHANNELS.respond),
            vec![json!({"sessionKey":THREAD,"approvalId":"a1","verdict":"allow"})]
        );
    }
}

#[tokio::test]
async fn approvals_follow_the_mode_the_live_session_launched_with() {
    // Launched gated, switched to Auto while live: its gate hook still asks for
    // everything, so Interface's policy keeps answering until the next launch.
    let world = new_world();
    world.start("thread-1").await.unwrap();
    set_permission_mode(&world, "auto");
    let home = world.home();
    world.tap.observe_session_event(
        THREAD,
        &approval("a1", "Read", &json!({"path": format!("{home}/notes.md")})),
    );
    eventually("policy answer", || {
        world.runtime.calls(CHANNELS.respond).len() == 1
    })
    .await;
    assert!(world.decisions().iter().all(|d| d.reason == "read"));

    // Launched in Auto, switched to Ask while live: Claude's escalations still
    // go to the owner, never to a policy the session was not launched under.
    let world = new_world();
    set_permission_mode(&world, "auto");
    world.start("thread-1").await.unwrap();
    set_permission_mode(&world, "gated");
    let home = world.home();
    world.tap.observe_session_event(
        THREAD,
        &approval("a2", "Read", &json!({"path": format!("{home}/notes.md")})),
    );
    eventually("owner approval", || {
        !world.read().pending_approvals.is_empty()
    })
    .await;
    assert!(world.runtime.calls(CHANNELS.respond).is_empty());
    assert_eq!(world.read().pending_approvals[0].reason, "providerAuto");
}

#[tokio::test]
async fn an_unreadable_agent_store_never_closes_an_ordinary_chat() {
    let world = world_with(true);
    for value in [
        json!({"kind":"status.changed","status":"working"}),
        json!({"kind":"tool.call","callId":"1","tool":"Read","summary":"Read","input":{}}),
        json!({"kind":"status.changed","status":"idle"}),
    ] {
        world
            .tap
            .observe_session_event("codex#plain", &event(value));
    }
    // Any retirement is spawned; give it the chance to run.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(world.runtime.calls(CHANNELS.close_session).is_empty());
}

#[tokio::test]
async fn only_a_committed_agent_change_wakes_the_scheduler() {
    let world = new_world();
    world.desktop.prepare_agent_thread("research").unwrap();
    world.desktop.bind_agent_thread("research", THREAD).unwrap();
    let woken = || async {
        tokio::time::timeout(Duration::from_millis(50), world.host.runner_wake.notified())
            .await
            .is_ok()
    };
    let _ = woken().await;
    world.tap.observe_session_event(
        THREAD,
        &event(json!({"kind":"message.delta","text":"token"})),
    );
    assert!(!woken().await, "a text delta");
    // A newly bound thread is working; its turn ending commits the first change.
    let idle = event(json!({"kind":"status.changed","status":"idle"}));
    world.tap.observe_session_event(THREAD, &idle);
    assert!(woken().await, "the thread went idle");
    world.tap.observe_session_event(THREAD, &idle);
    assert!(!woken().await, "the thread was already idle");
}

/// A consultation probes only the owner's chosen provider: every other provider stays
/// unlaunched, and another ready provider is never used in its place.
#[tokio::test]
async fn a_consultation_probes_only_its_provider_and_refuses_when_it_is_unavailable() {
    use interface_contracts::agent_work::{AgentConsultationGrant, AgentWorkSpec};
    let world = new_world();
    let spec = AgentWorkSpec {
        consultation: Some(AgentConsultationGrant {
            model: "codex/test-model".into(),
            files: vec!["selected.txt".into()],
            writable_files: Vec::new(),
            max_children: 2,
        }),
        ..Default::default()
    };
    let read = world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"work","agentId":"research","command":{"action":"read"}}),
        )
        .unwrap();
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"work","agentId":"research","command":{"action":"configure",
                "expectedRevision":read["work"]["revision"],"spec":spec}}),
        )
        .unwrap();
    world.start("consulting").await.unwrap();
    std::fs::write(Path::new(&world.home()).join("selected.txt"), "before").unwrap();
    let task = world.desktop.active_agent_work(THREAD).unwrap().unwrap();
    world
        .facts
        .ready
        .lock()
        .unwrap()
        .push(("claude".into(), "/usr/local/bin/claude".into()));

    let result = crate::agent_consultation::consult(
        world.host.clone(),
        "research",
        &task.id,
        &json!({"requestId":"consult-1","brief":"Improve the selected text"}),
    )
    .await
    .unwrap();

    let result: Value = serde_json::from_str(&result).unwrap();
    assert_eq!(
        result["error"],
        "Consultation provider is unavailable; no alternate provider was used."
    );
    assert_eq!(*world.facts.probed.lock().unwrap(), ["codex"]);
    assert!(world.runtime.calls(CHANNELS.probe_engines).is_empty());
}

async fn start_task(world: &World, request_id: &str, cwd: &str) -> Result<Value, DispatchError> {
    world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"startAgentTask","agentId":"research","text":"Summarize the open issues.",
                "cwd":cwd,"requestId":request_id,"title":"Summarize the open issues","model":"opus"}),
        )
        .await
}

/// A project folder outside the owner's home directory and the agents root, so it is grantable.
fn elsewhere(world: &World) -> String {
    let project = world.temp.path().join("elsewhere");
    std::fs::create_dir_all(&project).unwrap();
    project.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_teammate_task_launches_behind_the_gate_with_home_folders_and_job() {
    let world = new_world();
    let granted = world.grant_work();
    let project = elsewhere(&world);
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"key":"claude#task"})));
    let response = start_task(&world, "task-1", &project).await.unwrap();
    assert_eq!(response["sessionKey"], "claude#task");
    // The home exists now, with its instruction files and MEMORY.md.
    let home = world.home();
    assert!(Path::new(&home).join("MEMORY.md").is_file());
    let creates = world.runtime.calls(CHANNELS.create_session);
    assert_eq!(creates.len(), 1);
    assert_eq!(creates[0]["engine"], "claude");
    let options = &creates[0]["options"];
    assert_eq!(options["cwd"], project);
    assert_eq!(options["kind"], "agent");
    assert_eq!(options["approvalGate"], true);
    assert_eq!(options["additionalDirectories"], json!([home, granted]));
    // Outside its home the agent's job rides in front of the owner's words.
    assert_eq!(
        options["prompt"],
        "Cite primary sources.\n\nSummarize the open issues."
    );
    assert_eq!(options["name"], "Summarize the open issues");
    assert_eq!(options["model"], "opus");
    for provider_side in [
        "permissionMode",
        "agentAccess",
        "settingSources",
        "allowedTools",
        "worktree",
    ] {
        assert!(options.get(provider_side).is_none(), "{provider_side}");
    }
    let state = world.read();
    let run = state
        .workspace
        .runs
        .iter()
        .find(|run| run.id == "task:task-1")
        .unwrap();
    assert_eq!(run.status, AgentRunStatus::Running);
    assert_eq!(run.session_key.as_deref(), Some("claude#task"));
    assert_eq!(state.workspace.assignments["claude#task"], "research");
    let binding = world.desktop.agent_binding("claude#task").unwrap();
    assert!(!binding.thread);
    assert_eq!(binding.cwd, Path::new(&project));

    // Its approvals go through the agent's policy, like a thread's.
    world.tap.observe_session_event(
        "claude#task",
        &approval(
            "t1",
            "Read",
            &json!({"path": format!("{project}/README.md")}),
        ),
    );
    eventually("policy answer", || {
        world.runtime.calls(CHANNELS.respond).len() == 1
    })
    .await;
    assert_eq!(
        world.runtime.calls(CHANNELS.respond),
        vec![json!({"sessionKey":"claude#task","approvalId":"t1","verdict":"allow"})]
    );

    // The same request reports its task and never launches again.
    let again = start_task(&world, "task-1", &project).await.unwrap();
    assert_eq!(again["sessionKey"], "claude#task");
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
}

#[tokio::test]
async fn a_teammate_task_in_a_bypass_agent_launches_with_its_access() {
    let world = new_world();
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"savePermissionMode","agentId":"research","mode":"bypass"}),
        )
        .unwrap();
    let project = elsewhere(&world);
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Ok(json!({"key":"claude#task"})));
    start_task(&world, "task-1", &project).await.unwrap();
    let options = &world.runtime.calls(CHANNELS.create_session)[0]["options"];
    assert_eq!(options["agentAccess"], "bypass");
    assert!(options.get("approvalGate").is_none());
}

#[tokio::test]
async fn a_teammate_task_is_refused_before_the_provider_and_leaves_no_run() {
    let world = new_world();
    let refused = |error: &DispatchError| {
        assert!(error.replay_safe, "{error:?}");
        assert_eq!(error.code, DEFINITELY_UNSENT_CODE, "{error:?}");
    };
    let home_directory = world.temp.path().join("home");
    let error = start_task(&world, "broad", home_directory.to_str().unwrap())
        .await
        .unwrap_err();
    refused(&error);
    assert!(error.message.contains("too broad"), "{}", error.message);
    world.desktop.prepare_agent_thread("research").unwrap();
    let error = start_task(&world, "home", &world.home()).await.unwrap_err();
    refused(&error);
    assert!(error.message.contains("conversation"), "{}", error.message);
    let missing = world.temp.path().join("missing");
    refused(
        &start_task(&world, "missing", missing.to_str().unwrap())
            .await
            .unwrap_err(),
    );
    assert!(world.read().workspace.runs.is_empty());

    // An ACP teammate cannot enforce the gate, so its task never reaches a run either.
    let revision = world.read().workspace.revision;
    world
        .desktop
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveAgent","expectedRevision":revision,"profile":{
                "id":"grok","name":"Grok","engine":"acp:grok","model":"","effort":"",
                "instructions":"","color":"coral","shape":"star"
            }}),
        )
        .unwrap();
    let error = world
        .host
        .dispatch(
            CHANNELS.agent_control,
            json!({"kind":"startAgentTask","agentId":"grok","text":"Go.",
                "cwd":elsewhere(&world),"requestId":"acp"}),
        )
        .await
        .unwrap_err();
    refused(&error);
    assert!(error.message.contains("approval gate"), "{}", error.message);
    assert!(world.read().workspace.runs.is_empty());
    assert!(world.runtime.calls(CHANNELS.create_session).is_empty());

    // An agent still working on its thread is not given a second task alongside it.
    world.start("thread-1").await.unwrap();
    let error = start_task(&world, "busy", &elsewhere(&world))
        .await
        .unwrap_err();
    refused(&error);
    assert!(error.message.contains("still working"), "{}", error.message);
    assert!(world.read().workspace.runs.is_empty());
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
}

#[tokio::test]
async fn a_teammate_task_whose_provider_answer_is_unknown_is_never_relaunched() {
    let world = new_world();
    let project = elsewhere(&world);
    world
        .runtime
        .creates
        .lock()
        .unwrap()
        .push_back(Err(DispatchError::new(
            OUTCOME_UNKNOWN_CODE,
            "lost",
            false,
            false,
        )));
    let error = start_task(&world, "task-1", &project).await.unwrap_err();
    assert!(!error.replay_safe);
    let state = world.read();
    let run = state
        .workspace
        .runs
        .iter()
        .find(|run| run.id == "task:task-1")
        .unwrap();
    assert_eq!(run.status, AgentRunStatus::Unknown);
    let again = start_task(&world, "task-1", &project).await.unwrap_err();
    assert!(!again.replay_safe);
    assert_eq!(world.runtime.calls(CHANNELS.create_session).len(), 1);
}
