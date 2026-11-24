//! Adapter between durable Rust agent claims and the existing provider runtime.
//!
//! `AgentHost` starts an agent's thread behind the approval gate, answers each approval an
//! agent-bound session raises with the Rust policy (or hands it to the owner), and serves
//! `iface:agentControl` ahead of the desktop dispatcher. `AgentRuntimeGuard` wraps the runtime
//! and session-management bridges desktop callers use, so owner answers, read-only agent
//! sessions and resumes of agent sessions cannot bypass that policy.
use crate::dispatcher::{
    DEFINITELY_UNSENT_CODE, DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE,
    OUTCOME_UNKNOWN_CODE, ResumeRequirement, ResumeRequirements, RuntimeCommandHandler,
    SessionEventObserver,
};
use crate::router::DesktopHostDispatcher;
use crate::session_management::SEND_LIVE_OPERATION;
use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::{
    agent_work::{AgentExecutionState, AgentWorkCommand},
    agents::{
        AgentCommand, AgentDecision, AgentDecisionKind, AgentDeliveryOutcome, AgentPendingApproval,
        AgentRun, ApprovalFacts,
    },
    engine::{AgentAccess, EngineEvent, EngineEventKind, EngineId},
    ipc::CHANNELS,
};
use interface_desktop_services::{
    AGENT_WORK_PROJECTION_FAILED, AgentThreadLaunch, DesktopService, DesktopServiceError,
    agent_policy::{self, AgentScope, PolicyDecision},
};
use interface_runtime::ProviderReadiness;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

const SUMMARY_CHARS: usize = 512;
const POLICY_ERROR: &str = "policyError";
const DROPPED_BEFORE_BINDING: &str = "droppedBeforeBinding";
const STILL_CHECKING: &str =
    "Interface is still checking this request with the agent approval policy.";
const READ_ONLY: &str = "Past conversations are read-only. Start a new conversation to continue.";
const MESSAGE_IN_THREAD: &str = "Message this agent in its conversation instead.";
const REGISTRY_UNREADABLE: &str =
    "Interface could not read this session's record, so nothing was sent.";
/// A store check on the blocking pool that did not finish, before anything was sent.
const STORE_UNCHECKED: &str = "Interface could not check agent storage, so nothing was sent.";
/// A task outcome that could not be recorded after the provider call.
const SETTLE_UNRECORDED: &str =
    "Interface could not record this task's outcome. Inspect the session before sending again.";
const ROUTINE_TOO_BROAD: &str =
    "This routine's folder is too broad to run unattended. Choose a project folder.";
const ROUTINE_UNCHECKED: &str = "Interface could not check this routine's folder. It was not run.";
const TASK_TOO_BROAD: &str = "This folder is too broad for an agent task. Choose a project folder, or grant one in the agent's Computer tab.";
const TASK_IN_HOME: &str =
    "This is the agent's own folder. Message it in its conversation to work there.";
const TASK_UNRESOLVED: &str = "This task may already have started. Inspect the agent's runs before starting it again; Interface will not resend it.";
const ACP_AGENT_REFUSED: &str =
    "This ACP agent cannot enforce Interface's approval gate yet, so it can't run as an agent.";

/// Operations that change what a session does. Refused for read-only agent sessions;
/// interrupting, closing and reading history stay available.
const SESSION_EFFECT_OPERATIONS: [&str; 7] = [
    CHANNELS.resume_session,
    CHANNELS.send,
    SEND_LIVE_OPERATION,
    CHANNELS.set_model,
    CHANNELS.set_effort,
    CHANNELS.set_fast_mode,
    CHANNELS.set_permission_mode,
];

type Buffered = VecDeque<(String, EngineEvent)>;

/// What the host reads from the provider runtime: facts about a session, and
/// whether one provider is ready.
#[async_trait]
pub trait ApprovalFactsSource: Send + Sync {
    /// Provider-neutral facts for one approval, from the adapter that owns the session.
    fn approval_facts(&self, session_key: &str, tool: &str, input: &Value) -> ApprovalFacts;

    /// The working folder the session registry recorded, `None` without a record.
    ///
    /// # Errors
    /// The registry could not be read; the caller refuses the effect.
    fn session_cwd(&self, session_key: &str) -> Result<Option<String>, String>;

    /// The executable of `engine` when a probe of that provider succeeds, `None`
    /// when it is unknown or unavailable. No other provider is probed.
    async fn ready_executable(&self, engine: &str) -> Option<String>;
}

#[async_trait]
impl ApprovalFactsSource for interface_runtime::SessionRuntime {
    fn approval_facts(&self, session_key: &str, tool: &str, input: &Value) -> ApprovalFacts {
        Self::approval_facts(self, session_key, tool, input)
    }

    fn session_cwd(&self, session_key: &str) -> Result<Option<String>, String> {
        RuntimeCommandHandler::session_summary(self, session_key)
            .map(|summary| summary.map(|summary| summary.cwd))
            .map_err(|error| error.message)
    }

    async fn ready_executable(&self, engine: &str) -> Option<String> {
        let engine = EngineId::new(engine).ok()?;
        match self.probe_provider(&engine).await {
            ProviderReadiness::Ready { info, .. } => Some(info.binary_path),
            ProviderReadiness::Unavailable { .. } => None,
        }
    }
}

/// A dispatcher attached after construction: the runtime dispatcher's event tap needs the
/// agent host, and the agent host needs the runtime dispatcher.
#[derive(Default)]
pub struct LateDispatcher(OnceLock<Arc<dyn HostDispatcher>>);

impl LateDispatcher {
    /// Returns false when a dispatcher was already attached.
    pub fn attach(&self, inner: Arc<dyn HostDispatcher>) -> bool {
        self.0.set(inner).is_ok()
    }
}

#[async_trait]
impl HostDispatcher for LateDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        match self.0.get() {
            Some(inner) => inner.dispatch(operation, payload).await,
            None => Err(unsent("The provider runtime is not ready yet.", true)),
        }
    }
}

/// A created session key (when the provider returned one) and whether it was recorded.
type Launched = (Option<String>, Result<String, DispatchError>);

pub struct AgentHost {
    this: Weak<Self>,
    pub(crate) desktop: Arc<DesktopService>,
    desktop_owner: DesktopHostDispatcher,
    runtime: Arc<dyn HostDispatcher>,
    facts: Arc<dyn ApprovalFactsSource>,
    /// Serializes admission and scope changes per agent, without blocking other agents.
    pub(crate) bound_tool_tasks: Mutex<BTreeSet<String>>,
    owned_sessions: Mutex<BTreeSet<String>>,
    /// The access each agent session last launched or resumed with. Approvals follow
    /// it, not the agent's current setting, which applies from the next launch.
    launched_access: Mutex<BTreeMap<String, Option<AgentAccess>>>,
    pub(crate) resumed_tools: Mutex<BTreeMap<String, crate::agent_tools::ResumedTools>>,
    admissions: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Provider launches whose session key is not bound yet.
    launching: AtomicUsize,
    /// Approvals seen for unbound keys during a launch. Taken before the agent store lock.
    buffered: Mutex<Buffered>,
    /// Facts of approvals handed to the owner, for the owner's decision log entry.
    asked: Mutex<VecDeque<(String, String, ApprovalFacts)>>,
    answered: Mutex<BTreeSet<(String, String)>>,
    /// Sessions that closed since their last approval. Lock order: `closed` before `asked`,
    /// `buffered` and the desktop's pending approvals.
    closed: Mutex<RecentlyClosed>,
    /// Wakes the routine scheduler after an agent command or a committed run change.
    runner_wake: Arc<tokio::sync::Notify>,
    /// Wakes the budget watch, which sleeps while no task is under a budget.
    budget_wake: tokio::sync::Notify,
    /// Budget checks the watch has made.
    #[cfg(test)]
    budget_scans: AtomicUsize,
}

/// How many closed sessions [`RecentlyClosed`] remembers. A late approval
/// arrives within moments of its session's close, never a thousand closes later.
const RECENTLY_CLOSED: usize = 1024;

/// The newest closed session keys, with set membership for the approval path.
#[derive(Default)]
struct RecentlyClosed {
    order: VecDeque<String>,
    keys: BTreeSet<String>,
}

impl RecentlyClosed {
    fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    fn insert(&mut self, key: &str) {
        if !self.keys.insert(key.to_owned()) {
            return;
        }
        self.order.push_back(key.to_owned());
        if self.order.len() > RECENTLY_CLOSED
            && let Some(oldest) = self.order.pop_front()
        {
            self.keys.remove(&oldest);
        }
    }

    fn remove(&mut self, key: &str) {
        if self.keys.remove(key) {
            self.order.retain(|closed| closed != key);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.order.len()
    }
}

impl AgentHost {
    /// The executable of `engine` when a probe of that provider alone succeeds.
    pub(crate) async fn ready_executable(&self, engine: &str) -> Option<String> {
        self.facts.ready_executable(engine).await
    }
    #[must_use]
    pub fn new(
        desktop: Arc<DesktopService>,
        runtime: Arc<dyn HostDispatcher>,
        facts: Arc<dyn ApprovalFactsSource>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            desktop_owner: DesktopHostDispatcher::new(desktop.clone()),
            desktop,
            runtime,
            facts,
            bound_tool_tasks: Mutex::new(BTreeSet::new()),
            owned_sessions: Mutex::new(BTreeSet::new()),
            launched_access: Mutex::new(BTreeMap::new()),
            resumed_tools: Mutex::new(BTreeMap::new()),
            admissions: Mutex::new(BTreeMap::new()),
            launching: AtomicUsize::new(0),
            buffered: Mutex::new(VecDeque::new()),
            asked: Mutex::new(VecDeque::new()),
            answered: Mutex::new(BTreeSet::new()),
            closed: Mutex::default(),
            runner_wake: Arc::new(tokio::sync::Notify::new()),
            budget_wake: tokio::sync::Notify::new(),
            #[cfg(test)]
            budget_scans: AtomicUsize::new(0),
        })
    }

    /// Asks the routine scheduler to claim again soon. A stored permit covers
    /// a scheduler that is busy rather than parked.
    fn wake_runner(&self) {
        self.runner_wake.notify_one();
    }

    /// Admits a task and wakes the budget watch for it. A stored permit
    /// covers a watch that is mid-check rather than asleep.
    fn begin_work(
        &self,
        agent: &str,
        id: &str,
        goal: &str,
        cwd: &str,
    ) -> Result<(), DesktopServiceError> {
        self.desktop.begin_agent_work(agent, id, goal, cwd)?;
        self.budget_wake.notify_one();
        Ok(())
    }

    fn spawn<F>(&self, work: impl FnOnce(Arc<Self>) -> F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if let Some(host) = self.this.upgrade() {
            tokio::spawn(work(host));
        }
    }

    async fn start_thread(&self, payload: Value) -> Result<Value, DispatchError> {
        let Ok(AgentCommand::StartAgentThread {
            agent_id,
            text,
            request_id,
        }) = serde_json::from_value(payload)
        else {
            return Err(invalid("Invalid agent command."));
        };
        if request_id.trim().is_empty() || request_id.chars().count() > 128 {
            return Err(invalid("A conversation start needs a request id."));
        }
        let admission = self.admission(&agent_id);
        let _guard = admission.lock().await;
        let reserved = blocking(&self.desktop, {
            let (request, agent, text) = (request_id.clone(), agent_id.clone(), text.clone());
            move |desktop| desktop.reserve_agent_start(&request, &agent, &text)
        })
        .await
        .ok_or_else(|| unsent("Could not reserve the first message.", false))?
        .map_err(|error| unsent(&error.message, false))?;
        match reserved {
            interface_desktop_services::AgentStartAdmission::Bound(key) => {
                let mut response = self
                    .desktop
                    .agent_control_response()
                    .map_err(|error| unknown(&error.message))?;
                response["sessionKey"] = json!(key);
                return Ok(response);
            }
            interface_desktop_services::AgentStartAdmission::Unresolved => {
                return Err(unknown(
                    "This first message may already have been sent. Inspect the provider's history before starting new work; Interface will not resend it.",
                ));
            }
            interface_desktop_services::AgentStartAdmission::Admitted => {}
        }
        let result = self.start_uncached(&agent_id, &text).await;
        let key = result
            .as_ref()
            .ok()
            .and_then(|value| text_field(value, "sessionKey"));
        let unsent = result.as_ref().is_err_and(|error| error.replay_safe);
        let saved = blocking(&self.desktop, move |desktop| {
            desktop.settle_agent_start(&request_id, key, unsent)
        })
        .await;
        if !matches!(saved, Some(Ok(()))) {
            return Err(unknown(
                "The first-message outcome could not be recorded. Inspect the provider before sending again.",
            ));
        }
        result
    }

    /// The owner's one-off task for a teammate (protocol 38): the same gated launch a routine
    /// run gets, in the folder the owner chose, admitted and launched in this request. Every
    /// refusal before the provider is definitely unsent and leaves no run; a repeated request
    /// id reports what its run became and never launches again.
    async fn start_task(&self, payload: Value) -> Result<Value, DispatchError> {
        let Ok(AgentCommand::StartAgentTask {
            agent_id,
            text,
            cwd,
            request_id,
            title,
            model,
            effort,
            recovery,
        }) = serde_json::from_value(payload)
        else {
            return Err(invalid("Invalid agent command."));
        };
        if request_id.trim().is_empty() || request_id.chars().count() > 64 {
            return Err(invalid("A task start needs a request id."));
        }
        if text.trim().is_empty() {
            return Err(invalid("Describe the task first."));
        }
        if !Path::new(&cwd).is_absolute() {
            return Err(invalid("Choose the task's folder."));
        }
        let admission = self.admission(&agent_id);
        let _guard = admission.lock().await;
        let id = format!("{}{request_id}", interface_settings::AGENT_TASK_RUN_PREFIX);
        let known = blocking(&self.desktop, {
            let id = id.clone();
            move |desktop| {
                desktop
                    .agent_control_response()
                    .ok()
                    .and_then(|state| state["runs"].as_array().cloned())
                    .is_some_and(|runs| runs.iter().any(|run| run["id"] == id.as_str()))
            }
        })
        .await
        .unwrap_or(false);
        // A repeated request skips the checks: its run already says what happened.
        if !known {
            self.refuse_task(&agent_id, &cwd).await?;
        }
        let admitted = blocking(&self.desktop, {
            let (request_id, agent_id, cwd, text) = (
                request_id.clone(),
                agent_id.clone(),
                cwd.clone(),
                text.clone(),
            );
            move |desktop| {
                desktop.admit_agent_task(&interface_desktop_services::AgentTaskRequest {
                    request_id: &request_id,
                    agent_id: &agent_id,
                    cwd: &cwd,
                    title: title.as_deref().unwrap_or(""),
                    prompt: &text,
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                })
            }
        })
        .await
        .ok_or_else(|| unsent("Interface could not admit this task.", false))?
        .map_err(|error| unsent(&error.message, false))?;
        let key = match admitted {
            interface_desktop_services::AgentTaskAdmission::Existing(run) => {
                match (run.session_key, run.status) {
                    (Some(key), _) => key,
                    (None, interface_contracts::agents::AgentRunStatus::Failed) => {
                        return Err(unsent(&run.detail, false));
                    }
                    (None, _) => return Err(unknown(TASK_UNRESOLVED)),
                }
            }
            interface_desktop_services::AgentTaskAdmission::Admitted(run) => {
                launch_run(self, run, Some(&text), recovery).await?
            }
        };
        let mut response = blocking(&self.desktop, DesktopService::agent_control_response)
            .await
            .and_then(Result::ok)
            .ok_or_else(|| {
                DispatchError::new(
                    "agent-unavailable",
                    "The task started, but the agent workspace could not be read. Reload it before sending again.",
                    false,
                    false,
                )
            })?;
        response["sessionKey"] = json!(key);
        Ok(response)
    }

    /// Why a new one-off task must not start, checked before anything is recorded: an ACP
    /// agent, or a folder that is missing, inside the agents root or too broad to grant.
    async fn refuse_task(&self, agent_id: &str, cwd: &str) -> Result<(), DispatchError> {
        let engine = self
            .desktop
            .agent_profile(agent_id)
            .map(|profile| profile.engine)
            .ok_or_else(|| unsent("Agent no longer exists.", false))?;
        if engine.starts_with("acp:") {
            return Err(unsent(ACP_AGENT_REFUSED, false));
        }
        let folder = blocking(&self.desktop, {
            let cwd = cwd.to_owned();
            move |desktop| {
                let path = Path::new(&cwd);
                if !path.is_dir() {
                    Err(format!("The task folder is missing: {cwd}"))
                } else if desktop.is_inside_agents_root(path) {
                    Err(TASK_IN_HOME.to_owned())
                } else if !desktop.is_grantable_root(path) {
                    Err(TASK_TOO_BROAD.to_owned())
                } else {
                    Ok(())
                }
            }
        })
        .await
        .ok_or_else(|| unsent("Interface could not check the task's folder.", false))?;
        folder.map_err(|detail| unsent(&detail, false))
    }

    fn admission(&self, agent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        lock(&self.admissions)
            .entry(agent_id.to_owned())
            .or_default()
            .clone()
    }

    async fn start_uncached(&self, agent_id: &str, text: &str) -> Result<Value, DispatchError> {
        let owner = agent_id.to_owned();
        let launch = blocking(&self.desktop, move |desktop| {
            desktop.prepare_agent_thread(&owner)
        })
        .await
        .ok_or_else(|| {
            unsent(
                "Interface could not prepare this agent's conversation.",
                false,
            )
        })?
        // Nothing reached a provider yet: every refusal here is replay-safe.
        .map_err(|error| DispatchError::new(error.code, error.message, error.retryable, true))?;
        let work_id = uuid::Uuid::new_v4().to_string();
        self.begin_work(agent_id, &work_id, text, &launch.home)
            .map_err(|error| unsent(&error.message, false))?;
        let options = thread_options(
            &launch,
            &self
                .desktop
                .agent_task_prompt(agent_id, text)
                .map_err(|e| unsent(&e.message, false))?,
        );
        let owner = agent_id.to_owned();
        let work = work_id.clone();
        let (created, bound) = {
            let _launching = Launching::enter(&self.launching);
            self.create_and_bind(
                &launch.engine,
                agent_id,
                options,
                None,
                move |desktop, key| {
                    desktop.bind_agent_thread(&owner, key)?;
                    desktop.bind_agent_work(&work, key)
                },
            )
            .await
        };
        if let Err(error) = &bound {
            let state = if error.replay_safe {
                AgentExecutionState::Failed
            } else {
                AgentExecutionState::Unknown
            };
            self.desktop
                .settle_agent_work(agent_id, &work_id, state, &error.message)
                .map_err(|e| unknown(&e.message))?;
        }
        self.drain_buffered(created.as_deref().map(|key| (key, agent_id)));
        let session_key = bound?;
        let mut response = blocking(&self.desktop, DesktopService::agent_control_response)
            .await
            .and_then(Result::ok)
            .ok_or_else(|| {
                DispatchError::new(
                    "agent-unavailable",
                    "The conversation started, but the agent workspace could not be read. Reload it before sending again.",
                    false,
                    false,
                )
            })?;
        response["sessionKey"] = json!(session_key);
        Ok(response)
    }

    /// Dispatches `createSession` and records its key. Callers hold a [`Launching`] scope.
    async fn create_and_bind(
        &self,
        engine: &str,
        agent_id: &str,
        options: Value,
        recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
        bind: impl FnOnce(&DesktopService, &str) -> Result<(), DesktopServiceError> + Send + 'static,
    ) -> Launched {
        let access = serde_json::from_value::<Option<AgentAccess>>(options["agentAccess"].clone())
            .ok()
            .flatten();
        let mut payload = json!({"engine": engine, "options": options});
        if let Some(recovery) = recovery {
            payload["recovery"] = json!(recovery);
        }
        let created = match self
            .runtime
            .dispatch(CHANNELS.create_session, payload)
            .await
        {
            Ok(created) => created,
            Err(error) => return (None, Err(error)),
        };
        let Some(key) = created
            .get("key")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return (
                None,
                Err(unknown(
                    "The provider started without a usable session identity.",
                )),
            );
        };
        lock(&self.owned_sessions).insert(key.clone());
        lock(&self.launched_access).insert(key.clone(), access);
        let recorded = blocking(&self.desktop, {
            let key = key.clone();
            move |desktop| bind(desktop, &key)
        })
        .await;
        let result = match recorded {
            Some(Ok(())) => Ok(key.clone()),
            // The first prompt was already delivered; a failed local binding is never unsent.
            Some(Err(error)) => Err(unknown(&format!(
                "The provider started, but Interface could not record it: {}",
                error.message
            ))),
            None => Err(unknown(
                "The provider started, but Interface could not record it.",
            )),
        };
        if result.is_ok()
            && let Ok(Some(task)) = self.desktop.active_agent_work(&key)
            && task.stop_requested
        {
            if let Err(error) = self
                .stop_work(&task.agent_id, &task.id, "Stopped during startup.")
                .await
            {
                return (Some(key), Err(error));
            }
        }
        if result.is_err() {
            self.drain_buffered(Some((&key, agent_id)));
            self.retire_thread(&key).await;
        }
        (Some(key), result)
    }

    /// Called by the event tap before the desktop sees the approval.
    fn approval_requested(&self, session_key: &str, event: &EngineEvent) {
        // Only an explicit successful resume can reopen admission, never a late event.
        if lock(&self.closed).contains(session_key) {
            return;
        }
        {
            let mut buffered = lock(&self.buffered);
            // Read `launching` before the binding: a launch binds its key before it ends, and
            // the drain after it waits for this lock.
            let launching = self.launching.load(Ordering::SeqCst) > 0;
            if self.desktop.agent_binding(session_key).is_none() {
                if launching {
                    // Unresolved approvals are never evicted. Repeated notifications
                    // replace their exact request instead of growing the queue.
                    if let EngineEventKind::ApprovalRequested { approval_id, .. } = &event.event {
                        buffered.retain(|(key, prior)| key != session_key || !matches!(
                            &prior.event, EngineEventKind::ApprovalRequested { approval_id: id, .. } if id == approval_id
                        ));
                    }
                    buffered.push_back((session_key.to_owned(), event.clone()));
                }
                return;
            }
        }
        let session_key = session_key.to_owned();
        let event = event.clone();
        self.spawn(move |host| async move {
            host.handle_event(&session_key, &event).await;
        });
    }

    /// Hands buffered approvals whose key is now bound to the driver. `launched` is the key
    /// and agent of the launch that just ended: its approvals are logged when they cannot be
    /// handled. Other unbound approvals are kept while another launch runs, then dropped.
    fn drain_buffered(&self, launched: Option<(&str, &str)>) {
        let mut ready = Vec::new();
        let mut dropped = Vec::new();
        {
            let mut buffered = lock(&self.buffered);
            let launching = self.launching.load(Ordering::SeqCst) > 0;
            for (key, event) in std::mem::take(&mut *buffered) {
                let launched_agent = launched
                    .filter(|(launched_key, _)| *launched_key == key)
                    .map(|(_, agent_id)| agent_id.to_owned());
                if self.desktop.agent_binding(&key).is_some() {
                    ready.push((key, event));
                } else if launched_agent.is_some() || !launching {
                    dropped.push((launched_agent, key, event));
                } else {
                    buffered.push_back((key, event));
                }
            }
        }
        if ready.is_empty() && dropped.is_empty() {
            return;
        }
        // Handled off the caller, so a thread start is not held behind provider answers.
        self.spawn(move |host| async move {
            for (key, event) in ready {
                host.handle_event(&key, &event).await;
            }
            for (agent_id, key, event) in dropped {
                host.record_dropped(agent_id.as_deref(), &key, &event).await;
            }
        });
    }

    async fn record_dropped(&self, agent_id: Option<&str>, session_key: &str, event: &EngineEvent) {
        let EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input,
        } = &event.event
        else {
            return;
        };
        let Some(agent_id) = agent_id else {
            tracing::warn!(
                session_key,
                approval_id,
                "an approval raised before its session was bound was dropped; no agent owns it"
            );
            return;
        };
        let entry = AgentDecision {
            at_ms: now_ms(),
            session_key: session_key.to_owned(),
            approval_id: approval_id.clone(),
            tool: tool.clone(),
            summary: summary.chars().take(SUMMARY_CHARS).collect(),
            facts: self.facts.approval_facts(session_key, tool, input),
            decision: AgentDecisionKind::FailedClosed,
            reason: DROPPED_BEFORE_BINDING.to_owned(),
            outcome: AgentDeliveryOutcome::Unsent,
        };
        if !self.record(agent_id, &entry).await {
            tracing::warn!(
                session_key,
                approval_id,
                "a dropped agent approval could not be logged"
            );
        }
    }

    fn is_buffered(&self, session_key: &str) -> bool {
        lock(&self.buffered)
            .iter()
            .any(|(key, _)| key == session_key)
    }

    /// The session ended: its approvals can no longer be answered.
    fn session_closed(&self, session_key: &str) {
        lock(&self.resumed_tools).remove(session_key);
        lock(&self.owned_sessions).remove(session_key);
        lock(&self.launched_access).remove(session_key);
        let mut closed = lock(&self.closed);
        closed.insert(session_key);
        self.desktop.clear_pending_agent_approvals(session_key);
        lock(&self.asked).retain(|(key, _, _)| key != session_key);
        lock(&self.buffered).retain(|(key, _)| key != session_key);
    }

    async fn handle_event(&self, session_key: &str, event: &EngineEvent) {
        if let EngineEventKind::ApprovalRequested {
            approval_id,
            tool,
            summary,
            input,
        } = &event.event
        {
            self.handle_approval(session_key, approval_id, tool, summary, input)
                .await;
        }
    }

    #[allow(clippy::too_many_lines)] // Keep the ordered decision and durable response path together.
    async fn handle_approval(
        &self,
        session_key: &str,
        approval_id: &str,
        tool: &str,
        summary: &str,
        input: &Value,
    ) {
        let Some(binding) = self.desktop.agent_binding(session_key) else {
            return;
        };
        let admission = self.admission(&binding.agent_id);
        let _guard = admission.lock().await;
        if lock(&self.closed).contains(session_key) {
            return;
        }
        let Some(binding) = self.desktop.agent_binding(session_key) else {
            return;
        };
        if !lock(&self.answered).insert((session_key.into(), approval_id.into())) {
            return;
        }
        if let Ok(Some(task)) = self.desktop.active_agent_work(session_key)
            && self
                .desktop
                .agent_tool_task(&task.agent_id, &task.id)
                .is_err()
        {
            let _ = self
                .stop_work(&task.agent_id, &task.id, "Task budget or admission closed.")
                .await;
            return;
        }
        let facts = self.facts.approval_facts(session_key, tool, input);
        let scope = AgentScope {
            roots: binding.roots,
            cwd: binding.cwd,
            home_dir: self.desktop.home_directory().to_path_buf(),
        };
        let bound = lock(&self.bound_tool_tasks).clone();
        let trusted_host_tool = matches!(&facts, ApprovalFacts::Mcp { server, tool } if server == crate::agent_tools::AGENT_TOOL_SERVER
            && crate::agent_tools::validate_arguments(tool, input).is_ok()
            && (bound.iter().any(|id| self.desktop.agent_tool_task(&binding.agent_id, id).is_ok_and(|task| task.session_key.as_deref() == Some(session_key))) || self.resumed_host_tools_admit(session_key)));
        let active = self.desktop.active_agent_work(session_key);
        // Opaque commands stay a task-only opt-in. Searching the web follows
        // the active task's copy of the owner's switch, and otherwise the
        // agent's configured one, so its conversation and routines obey it
        // too. Unreadable settings mean ask.
        let switches = agent_policy::OwnerSwitches {
            allow_opaque: active
                .as_ref()
                .ok()
                .and_then(Option::as_ref)
                .is_some_and(|task| task.spec.allow_opaque_shell_commands),
            allow_web_search: match &active {
                Ok(Some(task)) => task.spec.allow_web_search,
                Ok(None) => self
                    .desktop
                    .agent_work_spec(&binding.agent_id)
                    .is_ok_and(|spec| spec.allow_web_search),
                Err(_) => false,
            },
        };
        let access = lock(&self.launched_access)
            .get(session_key)
            .copied()
            .unwrap_or(binding.access);
        let classified = tokio::task::spawn_blocking({
            let facts = facts.clone();
            move || {
                if trusted_host_tool {
                    PolicyDecision::Allow {
                        reason: "trustedHostTool:v1".into(),
                    }
                } else if let Some(access) = access {
                    // The provider's own mode already decided; what it still asks
                    // about is the owner's to answer, never Interface's policy.
                    PolicyDecision::AskOwner {
                        reason: match access {
                            AgentAccess::Auto => "providerAuto",
                            AgentAccess::Bypass => "bypass",
                        }
                        .into(),
                    }
                } else {
                    agent_policy::classify_for_owner(&facts, &scope, switches)
                }
            }
        })
        .await;
        let (allow, decision, reason) = match classified {
            Ok(PolicyDecision::Allow { reason }) => (true, AgentDecisionKind::Allowed, reason),
            Ok(PolicyDecision::AskOwner { reason }) => (false, AgentDecisionKind::Asked, reason),
            Err(_) => (
                false,
                AgentDecisionKind::FailedClosed,
                POLICY_ERROR.to_owned(),
            ),
        };
        let mut entry = AgentDecision {
            at_ms: now_ms(),
            session_key: session_key.to_owned(),
            approval_id: approval_id.to_owned(),
            tool: tool.to_owned(),
            summary: summary.chars().take(SUMMARY_CHARS).collect(),
            facts,
            decision,
            reason,
            outcome: AgentDeliveryOutcome::Pending,
        };
        if !self.record(&binding.agent_id, &entry).await {
            // An unlogged decision is never acted on: the owner answers it.
            entry.decision = AgentDecisionKind::FailedClosed;
            POLICY_ERROR.clone_into(&mut entry.reason);
            self.ask_owner(&binding.agent_id, &entry);
            return;
        }
        if !allow {
            self.ask_owner(&binding.agent_id, &entry);
            return;
        }
        // Exactly one automatic answer. An unknown outcome is never answered again.
        let result = self
            .runtime
            .dispatch(
                CHANNELS.respond,
                json!({"sessionKey": session_key, "approvalId": approval_id, "verdict": "allow"}),
            )
            .await;
        entry.outcome = outcome_of(&result);
        entry.at_ms = now_ms();
        if !self.record(&binding.agent_id, &entry).await {
            tracing::warn!(
                "an automatic agent approval was answered but its outcome was not logged"
            );
        }
        if entry.outcome == AgentDeliveryOutcome::Unsent {
            // Nothing reached the provider, so the owner can still answer it.
            self.ask_owner(&binding.agent_id, &entry);
        }
    }

    /// Makes the approval an owner question, unless its session closed meanwhile.
    fn ask_owner(&self, agent_id: &str, entry: &AgentDecision) {
        let closed = lock(&self.closed);
        if closed.contains(&entry.session_key) {
            return;
        }
        {
            let mut asked = lock(&self.asked);
            asked.retain(|(key, id, _)| *key != entry.session_key || *id != entry.approval_id);
            asked.push_back((
                entry.session_key.clone(),
                entry.approval_id.clone(),
                entry.facts.clone(),
            ));
        }
        let retained = self
            .desktop
            .add_pending_agent_approval(AgentPendingApproval {
                agent_id: agent_id.to_owned(),
                session_key: entry.session_key.clone(),
                approval_id: entry.approval_id.clone(),
                tool: entry.tool.clone(),
                summary: entry.summary.clone(),
                reason: entry.reason.clone(),
                asked_at_ms: now_ms(),
            });
        drop(closed);
        if !retained {
            let key = entry.session_key.clone();
            self.spawn(move |host| async move {
                host.retire_thread(&key).await;
            });
        }
    }

    async fn record(&self, agent_id: &str, entry: &AgentDecision) -> bool {
        let agent_id = agent_id.to_owned();
        let entry = entry.clone();
        matches!(
            blocking(&self.desktop, move |desktop| {
                desktop.record_agent_decision(&agent_id, &entry)
            })
            .await,
            Some(Ok(()))
        )
    }

    /// Refuses an effect on a read-only agent session: a past thread, or an agent-home session
    /// that is neither a current thread nor an unsettled run. Independent of the agent store.
    async fn refuse_read_only(&self, session_key: &str) -> Result<(), DispatchError> {
        let cwd = self
            .facts
            .session_cwd(session_key)
            .map_err(|_| unsent(REGISTRY_UNREADABLE, false))?;
        let key = session_key.to_owned();
        if blocking(&self.desktop, move |desktop| {
            desktop.is_read_only_agent_session(&key, cwd.as_deref().map(Path::new))
        })
        .await
        .ok_or_else(|| unsent(STORE_UNCHECKED, false))?
        {
            return Err(unsent(READ_ONLY, false));
        }
        Ok(())
    }

    /// A session started or forked in an agent's home would run its first prompt ungated, then
    /// turn read-only with its approvals refused. Agents take new work in their thread; the
    /// agent host's own launches use the unguarded runtime.
    fn refuse_agent_home_launch(
        &self,
        operation: &str,
        payload: &Value,
    ) -> Result<(), DispatchError> {
        let cwd = if operation == CHANNELS.create_session {
            payload
                .pointer("/options/cwd")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        } else {
            let Some(session_key) = text_field(payload, "sessionKey") else {
                return Ok(());
            };
            self.facts
                .session_cwd(&session_key)
                .map_err(|_| unsent(REGISTRY_UNREADABLE, false))?
        };
        if cwd.is_some_and(|cwd| self.desktop.is_inside_agents_root(Path::new(&cwd))) {
            return Err(unsent(MESSAGE_IN_THREAD, false));
        }
        Ok(())
    }

    /// The owner's answer to an approval, sent once through `inner`.
    async fn owner_respond(
        &self,
        inner: &dyn HostDispatcher,
        payload: Value,
    ) -> Result<Value, DispatchError> {
        let (Some(session_key), Some(approval_id), Some(verdict)) = (
            text_field(&payload, "sessionKey"),
            text_field(&payload, "approvalId"),
            text_field(&payload, "verdict"),
        ) else {
            return inner.dispatch(CHANNELS.respond, payload).await;
        };
        if self.is_buffered(&session_key) {
            return Err(still_checking());
        }
        let decision = match verdict.as_str() {
            "allow" => AgentDecisionKind::OwnerAllowed,
            "deny" => AgentDecisionKind::OwnerDenied,
            _ => return inner.dispatch(CHANNELS.respond, payload).await,
        };
        // A pending entry is answered even when its session is no longer bound to the agent.
        let Some(pending) = self
            .desktop
            .take_pending_agent_approval(&session_key, &approval_id)
        else {
            if self.session_binding(&session_key).await?.is_some() {
                return Err(still_checking());
            }
            self.refuse_read_only(&session_key).await?;
            return inner.dispatch(CHANNELS.respond, payload).await;
        };
        let facts = {
            let mut asked = lock(&self.asked);
            let index = asked
                .iter()
                .position(|(key, id, _)| *key == session_key && *id == approval_id);
            index.and_then(|index| asked.remove(index))
        }
        .map_or_else(
            || ApprovalFacts::Unknown {
                tool: pending.tool.clone(),
            },
            |(_, _, facts)| facts,
        );
        let result = inner.dispatch(CHANNELS.respond, payload).await;
        let entry = AgentDecision {
            at_ms: now_ms(),
            session_key,
            approval_id,
            tool: pending.tool.clone(),
            summary: pending.summary.clone(),
            facts,
            decision,
            reason: pending.reason.clone(),
            outcome: outcome_of(&result),
        };
        if !self.record(&pending.agent_id, &entry).await {
            tracing::warn!("an owner agent approval was answered but its outcome was not logged");
        }
        if entry.outcome == AgentDeliveryOutcome::Unsent {
            // Definitely unsent: keep it pending so the replay-safe retry is accepted.
            self.ask_owner(&pending.agent_id, &entry);
        }
        result
    }

    /// The agent's current thread key, read from the store.
    async fn current_thread(&self, agent_id: &str) -> Option<String> {
        let workspace = blocking(&self.desktop, DesktopService::agent_control_response)
            .await?
            .ok()?;
        thread_key(&workspace, agent_id)
    }

    /// Detaches a thread that stopped being the agent's current thread. Failures are ignored.
    async fn retire_thread(&self, session_key: &str) {
        if let Ok(Some(task)) = self.desktop.active_agent_work(session_key) {
            let _ = self.desktop.stop_agent_work(
                &task.agent_id,
                &task.id,
                "Thread retired by the owner.",
            );
        }
        if let Err(error) = self
            .runtime
            .dispatch(CHANNELS.close_session, json!({"sessionKey": session_key}))
            .await
        {
            if let Ok(Some(task)) = self.desktop.active_agent_work(session_key) {
                let _ = self.desktop.settle_agent_work(
                    &task.agent_id,
                    &task.id,
                    AgentExecutionState::Unknown,
                    "Retired provider closure was not confirmed. Inspect and acknowledge.",
                );
            }
            tracing::warn!(code = %error.code, "a retired agent thread session did not close");
            return;
        }
        if let Ok(Some(task)) = self.desktop.active_agent_work(session_key) {
            let _ = self.desktop.settle_agent_work(
                &task.agent_id,
                &task.id,
                AgentExecutionState::Stopped,
                "Thread retired.",
            );
        }
        self.session_closed(session_key);
    }

    /// Close every process before removing the durable identity that owns its permissions.
    async fn remove_agent(&self, mut payload: Value) -> Result<Value, DispatchError> {
        let agent = text_field(&payload, "id").ok_or_else(|| invalid("Missing agent."))?;
        let workspace = self
            .desktop
            .agent_control_response()
            .map_err(|e| unsent(&e.message, false))?;
        if workspace.get("revision") != payload.get("expectedRevision") {
            return Err(unsent(
                "Agent state changed. Reload before removing.",
                false,
            ));
        }
        for key in self
            .desktop
            .agent_live_sessions(&agent)
            .map_err(|e| unsent(&e.message, false))?
        {
            self.runtime
                .dispatch(CHANNELS.close_session, json!({"sessionKey":key}))
                .await?;
            if let Ok(Some(task)) = self.desktop.active_agent_work(&key) {
                self.desktop
                    .settle_agent_work(
                        &agent,
                        &task.id,
                        AgentExecutionState::Stopped,
                        "Agent removed after provider closure.",
                    )
                    .map_err(|e| unknown(&e.message))?;
            }
            self.session_closed(&key);
        }
        payload["expectedRevision"] = json!(
            self.desktop
                .agent_revision()
                .ok_or_else(|| unknown("Agent state unavailable after closure."))?
        );
        self.desktop_owner
            .dispatch(CHANNELS.agent_control, payload)
            .await
    }

    /// `newThread` and a provider change move the current thread to past threads; the old
    /// session is then closed and its pending approvals cleared.
    async fn replace_thread(
        &self,
        agent_id: Option<String>,
        payload: Value,
    ) -> Result<Value, DispatchError> {
        let before = match &agent_id {
            Some(agent_id) => self.current_thread(agent_id).await,
            None => None,
        };
        let response = self
            .desktop_owner
            .dispatch(CHANNELS.agent_control, payload)
            .await?;
        if let (Some(agent_id), Some(before)) = (agent_id, before)
            && thread_key(&response, &agent_id).as_deref() != Some(before.as_str())
        {
            self.retire_thread(&before).await;
        }
        Ok(response)
    }

    /// The session's agent binding, read before anything is sent. While a commit holds the
    /// store (across fsync) the read waits on the blocking pool, not on this async worker; an
    /// uncontended read stays inline. A read that did not finish is definitely unsent.
    async fn session_binding(
        &self,
        session_key: &str,
    ) -> Result<Option<interface_settings::AgentBinding>, DispatchError> {
        if let Some(binding) = self.desktop.try_agent_binding(session_key) {
            return Ok(binding);
        }
        let key = session_key.to_owned();
        blocking(&self.desktop, move |desktop| desktop.agent_binding(&key))
            .await
            .ok_or_else(|| unsent(STORE_UNCHECKED, false))
    }

    /// [`Self::begin_work`] on the blocking pool; `None` when its task did not finish.
    async fn begin_work_off_worker(
        &self,
        agent: &str,
        id: &str,
        goal: &str,
        cwd: &str,
    ) -> Option<Result<(), DesktopServiceError>> {
        let (agent, id, goal, cwd) = (
            agent.to_owned(),
            id.to_owned(),
            goal.to_owned(),
            cwd.to_owned(),
        );
        let begun = blocking(&self.desktop, move |desktop| {
            desktop.begin_agent_work(&agent, &id, &goal, &cwd)
        })
        .await;
        // A task that did not finish may still have admitted the work.
        if !matches!(begun, Some(Err(_))) {
            self.budget_wake.notify_one();
        }
        begun
    }

    fn gated_resume_directories(&self, session_key: &str) -> Option<Vec<String>> {
        let binding = self.desktop.agent_binding(session_key)?;
        // Every granted folder except the session's own working folder.
        Some(
            binding
                .roots
                .iter()
                .filter(|root| **root != binding.cwd)
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
        )
    }
}

impl ResumeRequirements for AgentHost {
    fn resume_requirement(&self, session_key: &str, registry_cwd: &str) -> ResumeRequirement {
        if self
            .desktop
            .is_read_only_agent_session(session_key, Some(Path::new(registry_cwd)))
        {
            return ResumeRequirement::Refused(READ_ONLY.to_owned());
        }
        let Some(directories) = self.gated_resume_directories(session_key) else {
            return ResumeRequirement::Unrestricted;
        };
        let access = self
            .desktop
            .agent_binding(session_key)
            .and_then(|binding| binding.access);
        // Whatever resumes the session resumes it this way, so its approvals follow.
        lock(&self.launched_access).insert(session_key.to_owned(), access);
        match access {
            Some(access) => ResumeRequirement::Access(access, directories),
            None => ResumeRequirement::Gated(directories),
        }
    }
}

#[async_trait]
impl HostDispatcher for AgentHost {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let result = self.dispatch_agent_control(operation, payload).await;
        if operation == CHANNELS.agent_control {
            // Saving or running a routine and acknowledging a run can all make
            // a claim possible sooner than the scheduler's next due time.
            self.wake_runner();
        }
        result
    }
}

impl AgentHost {
    #[allow(clippy::too_many_lines)]
    async fn dispatch_agent_control(
        &self,
        operation: &str,
        mut payload: Value,
    ) -> Result<Value, DispatchError> {
        // The older Runs action and Task results acknowledge the same durable lease.
        if payload["kind"] == "acknowledgeRun" {
            let state = self
                .desktop
                .agent_control_response()
                .map_err(|e| unsent(&e.message, false))?;
            if state["revision"] != payload["expectedRevision"] {
                return Err(unsent(
                    "Agent state changed. Reload before acknowledging.",
                    false,
                ));
            }
            if let Some(run) = state["runs"]
                .as_array()
                .and_then(|runs| runs.iter().find(|r| r["id"] == payload["id"]))
                && let (Some(agent), Some(id)) = (run["agentId"].as_str(), run["id"].as_str())
                && self.desktop.agent_work_record(agent, id).is_ok()
            {
                payload = json!({"kind":"work","agentId":agent,"command":{"action":"acknowledge","taskId":id}});
            }
        }
        let kind = payload.get("kind").and_then(Value::as_str).unwrap_or("");
        let agent = text_field(&payload, "agentId")
            .or_else(|| payload.get("profile").and_then(|p| text_field(p, "id")))
            .or_else(|| {
                payload
                    .get("routine")
                    .and_then(|p| text_field(p, "agentId"))
            })
            .or_else(|| {
                if !matches!(kind, "runNow" | "removeRoutine" | "acknowledgeRun") {
                    return None;
                }
                let state = self.desktop.agent_control_response().ok()?;
                let (list, id) = if kind == "acknowledgeRun" {
                    ("runs", &payload["id"])
                } else if kind == "runNow" {
                    ("routines", &payload["routineId"])
                } else {
                    ("routines", &payload["id"])
                };
                state[list]
                    .as_array()?
                    .iter()
                    .find(|r| &r["id"] == id)?
                    .get("agentId")?
                    .as_str()
                    .map(str::to_owned)
            })
            .or_else(|| {
                (kind == "removeAgent")
                    .then(|| text_field(&payload, "id"))
                    .flatten()
            });
        let stopping = kind == "work" && payload["command"]["action"] == "stop";
        let admission = agent
            .as_deref()
            .filter(|_| {
                !stopping
                    && !matches!(
                        kind,
                        "startAgentThread" | "startAgentTask" | "saveAgentFolders"
                    )
            })
            .map(|agent| self.admission(agent));
        let _guard = match &admission {
            Some(lock) => Some(lock.lock().await),
            None => None,
        };
        if operation == CHANNELS.agent_control {
            match payload.get("kind").and_then(Value::as_str) {
                Some("startAgentThread") => return self.start_thread(payload).await,
                Some("startAgentTask") => return self.start_task(payload).await,
                Some("saveAgentFolders") => return self.change_folders(payload).await,
                Some("work") => match serde_json::from_value(payload.clone()) {
                    Ok(AgentCommand::Work {
                        agent_id,
                        command: AgentWorkCommand::Acknowledge { task_id },
                    }) => {
                        let task = self
                            .desktop
                            .agent_work_record(&agent_id, &task_id)
                            .map_err(|e| unsent(&e.message, false))?;
                        if task.execution != AgentExecutionState::Unknown {
                            return Err(unsent(
                                "Only uncertain work needs acknowledgement.",
                                false,
                            ));
                        }
                        self.stop_work(
                            &agent_id,
                            &task_id,
                            "Owner acknowledged uncertain work after inspection.",
                        )
                        .await?;
                        self.desktop
                            .settle_agent_work(
                                &agent_id,
                                &task_id,
                                AgentExecutionState::Stopped,
                                "Owner acknowledged uncertain work; no replay.",
                            )
                            .map_err(|e| unknown(&e.message))?;
                        return self
                            .desktop
                            .agent_work_command(&agent_id, AgentWorkCommand::Read {})
                            .map_err(|e| unknown(&e.message));
                    }
                    Ok(AgentCommand::Work {
                        agent_id,
                        command: AgentWorkCommand::Stop { task_id },
                    }) => {
                        self.stop_work(&agent_id, &task_id, "Stopped by the owner.")
                            .await?;
                        return self
                            .desktop
                            .agent_work_command(&agent_id, AgentWorkCommand::Read {})
                            .map_err(|e| unknown(&e.message));
                    }
                    _ => {}
                },
                Some("removeAgent") => return self.remove_agent(payload).await,
                Some("newThread") => {
                    let agent_id = text_field(&payload, "agentId");
                    return self.replace_thread(agent_id, payload).await;
                }
                Some("saveAgent") => {
                    let agent_id = payload
                        .get("profile")
                        .and_then(|profile| text_field(profile, "id"));
                    return self.replace_thread(agent_id, payload).await;
                }
                _ => {}
            }
        }
        self.desktop_owner.dispatch(operation, payload).await
    }

    async fn stop_work(&self, agent: &str, id: &str, reason: &str) -> Result<(), DispatchError> {
        let before = self
            .desktop
            .agent_work_record(agent, id)
            .map_err(|e| unsent(&e.message, false))?;
        let key = self
            .desktop
            .stop_agent_work(agent, id, reason)
            .map_err(|e| unsent(&e.message, false))?;
        if let Some(key) = key {
            // Closing the process confirms it can no longer use its sandbox grants.
            if let Err(error) = self
                .runtime
                .dispatch(CHANNELS.close_session, json!({"sessionKey": key}))
                .await
            {
                self.desktop
                    .settle_agent_work(
                        agent,
                        id,
                        AgentExecutionState::Unknown,
                        "Provider closure was not confirmed. Inspect before acknowledging.",
                    )
                    .map_err(|e| unknown(&e.message))?;
                return Err(error);
            }
            self.session_closed(&key);
            self.desktop
                .settle_agent_work(
                    agent,
                    id,
                    if before.execution == AgentExecutionState::Completed {
                        AgentExecutionState::Completed
                    } else {
                        AgentExecutionState::Stopped
                    },
                    reason,
                )
                .map_err(|e| unknown(&e.message))?;
        }
        Ok(())
    }

    async fn change_folders(&self, mut payload: Value) -> Result<Value, DispatchError> {
        let agent_id = text_field(&payload, "agentId").ok_or_else(|| invalid("Missing agent."))?;
        let admission = self.admission(&agent_id);
        let _guard = admission.lock().await;
        let keys = self
            .desktop
            .check_agent_folder_change(payload.clone())
            .map_err(|error| unsent(&error.message, false))?;
        // Close every process holding the previous sandbox before committing the new scope.
        // If close is uncertain, keep the old saved scope and report the uncertainty.
        for key in keys {
            self.runtime
                .dispatch(CHANNELS.close_session, json!({"sessionKey": key}))
                .await?;
            if let Ok(Some(task)) = self.desktop.active_agent_work(&key) {
                self.desktop
                    .settle_agent_work(
                        &task.agent_id,
                        &task.id,
                        AgentExecutionState::Stopped,
                        "Folder grants changed; previous provider process was closed.",
                    )
                    .map_err(|e| unknown(&e.message))?;
            }
            self.session_closed(&key);
        }
        // Closing can commit status events. The owner's revision was checked before any
        // effect, and this agent's admissions remain locked throughout the scope update.
        let revision = self
            .desktop
            .agent_revision()
            .ok_or_else(|| unknown("Could not read agent storage after closing its sessions."))?;
        payload["expectedRevision"] = json!(revision);
        self.desktop_owner
            .dispatch(CHANNELS.agent_control, payload)
            .await
    }
}

/// Wraps the runtime and session-management bridges for every caller outside the agent host.
pub struct AgentRuntimeGuard {
    inner: Arc<dyn HostDispatcher>,
    agents: Arc<AgentHost>,
}

impl AgentRuntimeGuard {
    #[must_use]
    pub fn new(inner: Arc<dyn HostDispatcher>, agents: Arc<AgentHost>) -> Self {
        Self { inner, agents }
    }

    /// A store call before anything is sent, on the blocking pool: a commit holds the store
    /// across fsync. Its refusal, or a call that did not finish, is definitely unsent.
    async fn before_send<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&DesktopService) -> Result<T, DesktopServiceError> + Send + 'static,
    ) -> Result<T, DispatchError> {
        blocking(&self.agents.desktop, operation)
            .await
            .ok_or_else(|| unsent(STORE_UNCHECKED, false))?
            .map_err(|e| unsent(&e.message, false))
    }
}

#[async_trait]
impl HostDispatcher for AgentRuntimeGuard {
    #[allow(clippy::too_many_lines)]
    async fn dispatch(&self, operation: &str, mut payload: Value) -> Result<Value, DispatchError> {
        let admission = match text_field(&payload, "sessionKey") {
            Some(key) => self
                .agents
                .session_binding(&key)
                .await?
                .map(|binding| self.agents.admission(&binding.agent_id)),
            None => None,
        };
        let _guard = match &admission {
            Some(admission) => Some(admission.lock().await),
            None => None,
        };
        if operation == CHANNELS.respond {
            return self
                .agents
                .owner_respond(self.inner.as_ref(), payload)
                .await;
        }
        if operation == CHANNELS.create_session || operation == CHANNELS.fork_session {
            self.agents.refuse_agent_home_launch(operation, &payload)?;
        }
        if SESSION_EFFECT_OPERATIONS.contains(&operation)
            && let Some(session_key) = text_field(&payload, "sessionKey")
        {
            self.agents.refuse_read_only(&session_key).await?;
            if operation == CHANNELS.resume_session {
                let agents = Arc::clone(&self.agents);
                let gated = blocking(&self.agents.desktop, move |desktop| {
                    agents.gated_resume_directories(&session_key).filter(|_| {
                        desktop
                            .agent_binding(&session_key)
                            .is_none_or(|binding| binding.access.is_none())
                    })
                })
                .await
                .ok_or_else(|| unsent(STORE_UNCHECKED, false))?;
                if let Some(directories) = gated
                    && let Some(fields) = payload.as_object_mut()
                {
                    fields.insert("approvalGate".into(), Value::Bool(true));
                    if directories.is_empty() {
                        fields.remove("additionalDirectories");
                    } else {
                        fields.insert("additionalDirectories".into(), json!(directories));
                    }
                }
            }
        }
        let mut begun = None;
        if (operation == CHANNELS.send || operation == SEND_LIVE_OPERATION)
            && let Some(key) = text_field(&payload, "sessionKey")
            && let Some(binding) = self.agents.session_binding(&key).await?
        {
            match self
                .before_send({
                    let key = key.clone();
                    move |desktop| desktop.active_agent_work(&key)
                })
                .await?
            {
                Some(task)
                    if task.stop_requested || task.execution == AgentExecutionState::Unknown =>
                {
                    return Err(unsent(
                        "This task is stopped or uncertain. Inspect and acknowledge it before sending new work.",
                        false,
                    ));
                }
                Some(_) => {}
                None => {
                    let id = uuid::Uuid::new_v4().to_string();
                    let text = text_field(&payload, "text")
                        .ok_or_else(|| invalid("Missing task message."))?;
                    self.agents
                        .begin_work_off_worker(
                            &binding.agent_id,
                            &id,
                            &text,
                            &binding.cwd.to_string_lossy(),
                        )
                        .await
                        .ok_or_else(|| unsent(STORE_UNCHECKED, false))?
                        .map_err(|e| unsent(&e.message, false))?;
                    self.before_send({
                        let (id, key) = (id.clone(), key.clone());
                        move |desktop| desktop.bind_agent_work(&id, &key)
                    })
                    .await?;
                    // Binding marks the task stop-requested when startup
                    // events were lost, as create_and_bind checks. Nothing
                    // has been sent yet, so the send stops here.
                    if self
                        .before_send(move |desktop| desktop.active_agent_work(&key))
                        .await?
                        .is_none_or(|task| task.id != id || task.stop_requested)
                    {
                        return Err(unsent(
                            "This task is stopped or uncertain. Inspect and acknowledge it before sending new work.",
                            false,
                        ));
                    }
                    payload["text"] = json!(
                        self.before_send({
                            let agent = binding.agent_id.clone();
                            move |desktop| desktop.agent_task_prompt(&agent, &text)
                        })
                        .await?
                    );
                    begun = Some((binding.agent_id, id));
                }
            }
        }
        let resume_key = (operation == CHANNELS.resume_session)
            .then(|| text_field(&payload, "sessionKey"))
            .flatten();
        if let Some(key) = &resume_key {
            let (agents, key) = (Arc::clone(&self.agents), key.clone());
            blocking(&self.agents.desktop, move |_| {
                agents.prepare_resumed_host_tools(&key)
            })
            .await
            .ok_or_else(|| unsent(STORE_UNCHECKED, false))??;
        }
        let result = self.inner.dispatch(operation, payload).await;
        if result.is_err()
            && let Some(key) = &resume_key
        {
            lock(&self.agents.resumed_tools).remove(key);
        }
        if result.is_ok()
            && let Some(key) = resume_key
        {
            // Only budget-failure retirement reads this set: a read that did not finish
            // leaves the resumed session out rather than misreport the resume.
            if matches!(self.agents.session_binding(&key).await, Ok(Some(_))) {
                lock(&self.agents.owned_sessions).insert(key.clone());
            }
            lock(&self.agents.closed).remove(&key);
            lock(&self.agents.answered).retain(|(answered, _)| answered != &key);
        }
        if let (Some((agent, id)), Err(error)) = (begun, &result) {
            let state = if error.replay_safe {
                AgentExecutionState::Failed
            } else {
                AgentExecutionState::Unknown
            };
            let detail = error.message.clone();
            // After the provider call: a settlement that did not finish is never unsent.
            blocking(&self.agents.desktop, move |desktop| {
                desktop.settle_agent_work(&agent, &id, state, &detail)
            })
            .await
            .ok_or_else(|| unknown(SETTLE_UNRECORDED))?
            .map_err(|e| unknown(&e.message))?;
        }
        result
    }
}

/// The collaboration broker's agent check: an agent's current thread or run, a recorded past
/// thread, or any session working inside the agents root.
impl crate::collaboration::CollaborationAgentSessions for DesktopService {
    fn is_agent_session(&self, session_key: &str, canonical_cwd: &Path) -> bool {
        self.is_inside_agents_root(canonical_cwd)
            || self.agent_binding(session_key).is_some()
            || self.is_read_only_agent_session(session_key, Some(canonical_cwd))
    }
}

pub struct AgentEventTap {
    pub desktop: Arc<DesktopService>,
    pub next: Arc<dyn SessionEventObserver>,
    pub host: Arc<AgentHost>,
}
impl SessionEventObserver for AgentEventTap {
    fn observe_session_event(&self, key: &str, event: &EngineEvent) {
        self.next.observe_session_event(key, event);
        // The pump calls this inline. An event the agent stores record may
        // commit (a whole-file rewrite and its fsyncs) or wait behind another
        // session's commit, so the worker hands its other tasks off first.
        if may_record_agent_state(event) {
            off_the_async_worker(|| self.record(key, event));
        } else {
            self.record(key, event);
        }
        match &event.event {
            EngineEventKind::ApprovalRequested { .. } => self.host.approval_requested(key, event),
            EngineEventKind::SessionClosed => self.host.session_closed(key),
            _ => {}
        }
    }
}

impl AgentEventTap {
    fn record(&self, key: &str, event: &EngineEvent) {
        if let Err(error) = self.desktop.observe_agent_work(key, event) {
            tracing::warn!(%error, "agent task state could not be recorded");
            // Retire only an agent's own session, never an ordinary chat: the task bound to
            // it failed, or the store is unreadable and this host launched or resumed it.
            if error.code == AGENT_WORK_PROJECTION_FAILED
                || lock(&self.host.owned_sessions).contains(key)
            {
                let key = key.to_owned();
                self.host.spawn(move |host| async move {
                    host.retire_thread(&key).await;
                });
            }
        }
        match self.desktop.observe_agent_event(key, event) {
            // Only a committed run change (a run finishing frees a slot or
            // unblocks its agent) wakes the scheduler, never every stream event.
            Ok(true) => self.host.wake_runner(),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "agent event could not be recorded"),
        }
    }
}

/// The events `observe_agent_work` and `observe_agent_event` look at; every
/// other event (text deltas above all) returns before taking a lock. Only
/// decides where the work runs, never whether it runs.
fn may_record_agent_state(event: &EngineEvent) -> bool {
    matches!(
        event.event,
        EngineEventKind::ToolCall { .. }
            | EngineEventKind::ToolResult { .. }
            | EngineEventKind::UsageUpdated { .. }
            | EngineEventKind::StatusChanged { .. }
            | EngineEventKind::SessionClosed
            | EngineEventKind::UserMessage { .. }
            | EngineEventKind::ApprovalRequested { .. }
            | EngineEventKind::Error {
                retryable: false,
                ..
            }
    )
}

/// Runs blocking store work from synchronous code on an async worker: on the
/// host's multi-thread runtime the worker first hands its queued tasks to
/// another thread. A current-thread runtime (tests) runs it inline.
fn off_the_async_worker<T>(work: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(work)
        }
        _ => work(),
    }
}

pub struct AgentRunner {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}
/// The scheduler never claims more often than this: a due routine whose agent
/// is busy stays due, and each claim clones the agent workspace.
const RUNNER_MIN_WAIT: Duration = Duration::from_secs(2);
/// The longest the scheduler sleeps without a due time or a wake.
const RUNNER_MAX_WAIT: Duration = Duration::from_secs(60);
/// How often the budget watch checks while a task is under a budget.
const BUDGET_POLL: Duration = Duration::from_secs(1);

impl AgentRunner {
    #[must_use]
    pub fn start(agents: Arc<AgentHost>) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let wake = Arc::clone(&agents.runner_wake);
        let budget_task = tokio::spawn(budget_watch(agents.clone(), stopped.clone()));
        let task = tokio::spawn(async move {
            let mut launches = tokio::task::JoinSet::new();
            // Claim at once on startup so routines missed while the app was
            // closed still run.
            let mut next_claim = tokio::time::Instant::now();
            let mut last_claim: Option<tokio::time::Instant> = None;
            loop {
                tokio::select! {
                    biased;
                    _ = stopped.changed() => break,
                    // A finished launch or a woken scheduler may have a claim
                    // to make now: move the next claim up to the rate floor.
                    _ = launches.join_next(), if !launches.is_empty() => {
                        next_claim = next_claim.min(claim_floor(last_claim));
                        continue;
                    }
                    () = wake.notified() => {
                        next_claim = next_claim.min(claim_floor(last_claim));
                        continue;
                    }
                    () = tokio::time::sleep_until(next_claim) => {}
                }
                let now = tokio::time::Instant::now();
                last_claim = Some(now);
                if launches.len() >= 4 {
                    // A finishing launch moves this up.
                    next_claim = now + RUNNER_MAX_WAIT;
                    continue;
                }
                let service = agents.desktop.clone();
                let claim = tokio::task::spawn_blocking(move || {
                    let run = service.claim_agent_run()?;
                    Ok::<_, DesktopServiceError>((run, service.agent_claim_wait_ms()?))
                })
                .await;
                match claim {
                    Ok(Ok((Some(run), _))) => {
                        let agents = agents.clone();
                        launches.spawn(async move {
                            launch(&agents, run).await;
                        });
                        // Another queued run may be claimable right after.
                        next_claim = now + RUNNER_MIN_WAIT;
                    }
                    Ok(Ok((None, wait_ms))) => next_claim = now + claim_wait(wait_ms),
                    // Fail closed without flooding logs every tick. Restart retries storage startup.
                    _ => {
                        tracing::warn!(
                            "agent scheduling stopped because durable storage is unavailable"
                        );
                        break;
                    }
                }
            }
            // Never cancel a provider dispatch with an ambiguous effect.
            while launches.join_next().await.is_some() {}
            let _ = budget_task.await;
        });
        Self { stop, task }
    }
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

/// Stops tasks whose budgets expire. The first check comes a second after
/// startup; after it the watch polls only while a task is under a budget and
/// otherwise sleeps until an admission wakes it, or for the scheduler's
/// longest wait.
async fn budget_watch(agents: Arc<AgentHost>, mut stop: watch::Receiver<bool>) {
    let mut budgeted = true;
    loop {
        let wait = if budgeted {
            BUDGET_POLL
        } else {
            RUNNER_MAX_WAIT
        };
        tokio::select! {
            _ = stop.changed() => break,
            () = agents.budget_wake.notified(), if !budgeted => {}
            () = tokio::time::sleep(wait) => {}
        }
        #[cfg(test)]
        agents.budget_scans.fetch_add(1, Ordering::SeqCst);
        let desktop = Arc::clone(&agents.desktop);
        // Off the runtime: commits hold the store lock across fsync.
        let scan = match tokio::task::spawn_blocking(move || desktop.expired_agent_work()).await {
            Ok(scan) => scan.map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        match scan {
            Ok(tasks) => {
                budgeted = tasks.is_some();
                for task in tasks.into_iter().flatten() {
                    if let Err(error) = agents
                        .stop_work(
                            &task.agent_id,
                            &task.id,
                            "Task budget or progress deadline reached.",
                        )
                        .await
                    {
                        tracing::warn!(%error, "task stop is unconfirmed");
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "task budgets unavailable; closing owned providers");
                let keys = lock(&agents.owned_sessions).clone();
                for key in keys {
                    agents.retire_thread(&key).await;
                }
                break;
            }
        }
    }
}

/// The earliest the scheduler may claim again after its last claim.
fn claim_floor(last_claim: Option<tokio::time::Instant>) -> tokio::time::Instant {
    last_claim.map_or_else(tokio::time::Instant::now, |at| at + RUNNER_MIN_WAIT)
}

/// How long to sleep until the next due routine or queued run, bounded so a
/// busy agent's due routine cannot spin and a missed wake is caught within a
/// minute.
fn claim_wait(wait_ms: Option<u64>) -> Duration {
    wait_ms
        .map_or(RUNNER_MAX_WAIT, Duration::from_millis)
        .clamp(RUNNER_MIN_WAIT, RUNNER_MAX_WAIT)
}

/// Launches a routine's claimed run behind its agent's gate.
async fn launch(host: &AgentHost, run: AgentRun) {
    let admission = host.admission(&run.agent_id);
    let _guard = admission.lock().await;
    // Every failure is recorded on the run; the scheduler has no one to tell.
    let _ = launch_run(host, run, None, None).await;
}

/// Launches an admitted run behind its agent's gate: a routine's, or the owner's one-off
/// task (whose whole prompt is `prompt`; the run keeps only a bounded copy). The caller holds
/// the agent's admission lock. Every failure is recorded on the run and returned; the bound
/// session key is returned on success.
#[allow(clippy::too_many_lines)] // Admission, provider binding and failure settlement are one transaction.
async fn launch_run(
    host: &AgentHost,
    run: AgentRun,
    prompt: Option<&str>,
    recovery: Option<interface_contracts::recovery::PromptRecoveryInput>,
) -> Result<String, DispatchError> {
    let desktop = &host.desktop;
    // Every store call here runs on the blocking pool: a commit holds the store across fsync.
    let current = blocking(desktop, DesktopService::agent_control_response)
        .await
        .and_then(Result::ok)
        .and_then(|value| {
            serde_json::from_value::<interface_contracts::agents::AgentControlResponse>(value).ok()
        });
    if current
        .as_ref()
        .and_then(|state| {
            state
                .workspace
                .agents
                .iter()
                .find(|profile| profile.id == run.agent_id)
        })
        .is_none_or(|profile| profile.folders != run.profile.folders)
    {
        return Err(fail_run(
            desktop,
            &run,
            "Agent folders changed before this routine started. Review it before running again.",
        )
        .await);
    }
    let home = run.profile.home.clone();
    let checked = blocking(desktop, {
        let (cwd, home) = (run.cwd.clone(), home.clone());
        move |desktop| {
            // `is_grantable_root` refuses everything inside the agents root, the home included.
            let in_home = home.as_deref().is_some_and(|home| same_folder(&cwd, home));
            (
                in_home,
                in_home || desktop.is_grantable_root(Path::new(&cwd)),
            )
        }
    })
    .await;
    let in_home = match routine_folder(checked) {
        Ok(in_home) => in_home,
        Err(_) if run.routine_id.is_empty() => {
            return Err(fail_run(desktop, &run, TASK_TOO_BROAD).await);
        }
        Err(detail) => return Err(fail_run(desktop, &run, detail).await),
    };
    let asked = prompt.unwrap_or(&run.prompt);
    // The home folder's instruction files carry the job; any other folder has none.
    let prompt = if in_home || run.profile.instructions.trim().is_empty() {
        asked.to_owned()
    } else {
        format!("{}\n\n{asked}", run.profile.instructions)
    };
    let directories: Vec<String> = home
        .iter()
        .filter(|_| !in_home)
        .chain(&run.profile.folders)
        .filter(|folder| **folder != run.cwd)
        .cloned()
        .collect();
    let mut options = json!({"cwd":run.cwd,"prompt":prompt,"name":run.name,"kind":"agent"});
    let agent = run.agent_id.clone();
    let Some(access) = blocking(desktop, move |desktop| desktop.agent_access(&agent)).await else {
        return Err(fail_run(desktop, &run, STORE_UNCHECKED).await);
    };
    set_agent_access(&mut options, access);
    if !run.profile.model.is_empty() {
        options["model"] = json!(run.profile.model);
    }
    if !run.profile.effort.is_empty() {
        options["effort"] = json!(run.profile.effort);
    }
    if !directories.is_empty() {
        options["additionalDirectories"] = json!(directories);
    }
    match host
        .begin_work_off_worker(&run.agent_id, &run.id, &run.prompt, &run.cwd)
        .await
    {
        Some(Ok(())) => {}
        Some(Err(error)) => return Err(fail_run(desktop, &run, &error.message).await),
        None => return Err(fail_run(desktop, &run, STORE_UNCHECKED).await),
    }
    let asked = blocking(desktop, {
        let (agent, asked) = (
            run.agent_id.clone(),
            options["prompt"].as_str().unwrap_or("").to_owned(),
        );
        move |desktop| desktop.agent_task_prompt(&agent, &asked)
    })
    .await;
    options["prompt"] = match asked {
        Some(Ok(prompt)) => json!(prompt),
        Some(Err(error)) => return Err(fail_run(desktop, &run, &error.message).await),
        None => return Err(fail_run(desktop, &run, STORE_UNCHECKED).await),
    };
    // Unattended runs and one-off tasks retain the same approval policy as interactive work.
    let run_id = run.id.clone();
    let (created, bound) = {
        let _launching = Launching::enter(&host.launching);
        host.create_and_bind(
            &run.profile.engine,
            &run.agent_id,
            options,
            recovery,
            move |desktop, key| {
                desktop.bind_agent_run(&run_id, key)?;
                desktop.bind_agent_work(&run_id, key)
            },
        )
        .await
    };
    host.drain_buffered(created.as_deref().map(|key| (key, run.agent_id.as_str())));
    if let Err(error) = &bound {
        let state = if error.replay_safe {
            AgentExecutionState::Failed
        } else {
            AgentExecutionState::Unknown
        };
        let (agent, id, detail) = (run.agent_id.clone(), run.id.clone(), error.message.clone());
        let _ = blocking(desktop, move |desktop| {
            desktop.settle_agent_work(&agent, &id, state, &detail)
        })
        .await;
        record_failure(desktop, &run, &error.message, error.replay_safe).await;
    }
    bound
}

/// Whether a routine runs in its agent's home, from `(in_home, allowed)`, or why it must not
/// run. `None` means the folder check itself failed.
fn routine_folder(checked: Option<(bool, bool)>) -> Result<bool, &'static str> {
    match checked {
        Some((in_home, true)) => Ok(in_home),
        Some((_, false)) => Err(ROUTINE_TOO_BROAD),
        None => Err(ROUTINE_UNCHECKED),
    }
}

fn thread_options(launch: &AgentThreadLaunch, text: &str) -> Value {
    // Paths go to the provider exactly as the store holds them.
    let mut options = json!({
        "cwd": launch.home, "prompt": text, "name": launch.name, "kind": "agent",
    });
    set_agent_access(&mut options, launch.access);
    if !launch.model.is_empty() {
        options["model"] = json!(launch.model);
    }
    if !launch.effort.is_empty() {
        options["effort"] = json!(launch.effort);
    }
    if !launch.folders.is_empty() {
        options["additionalDirectories"] = json!(launch.folders);
    }
    options
}

/// Gated agents launch behind Interface's approval gate; Auto and Bypass agents in
/// the provider's own mode, read from the agent's settings at every launch.
fn set_agent_access(options: &mut Value, access: Option<interface_contracts::engine::AgentAccess>) {
    match access {
        None => options["approvalGate"] = json!(true),
        Some(access) => options["agentAccess"] = json!(access),
    }
}

fn thread_key(workspace: &Value, agent_id: &str) -> Option<String> {
    workspace["threads"][agent_id]["current"]["sessionKey"]
        .as_str()
        .map(ToOwned::to_owned)
}

fn same_folder(left: &str, right: &str) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(a), Ok(b)) if a == b
        )
}

async fn record_failure(desktop: &Arc<DesktopService>, run: &AgentRun, detail: &str, unsent: bool) {
    let (id, detail) = (run.id.clone(), detail.to_owned());
    match blocking(desktop, move |desktop| {
        desktop.fail_agent_run(&id, &detail, unsent)
    })
    .await
    {
        Some(Ok(())) => {}
        Some(Err(error)) => tracing::error!(%error,"agent run settlement could not be recorded"),
        None => tracing::error!("agent run settlement could not be recorded"),
    }
}

/// Records a run that was refused before its provider launch, and reports it unsent.
async fn fail_run(desktop: &Arc<DesktopService>, run: &AgentRun, detail: &str) -> DispatchError {
    record_failure(desktop, run, detail, true).await;
    unsent(detail, false)
}

/// Counts a provider launch whose key is not bound yet, for the approval buffer.
struct Launching<'a>(&'a AtomicUsize);
impl<'a> Launching<'a> {
    fn enter(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}
impl Drop for Launching<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Runs a store operation off the async workers; `None` when the worker failed.
pub(crate) async fn blocking<T: Send + 'static>(
    desktop: &Arc<DesktopService>,
    operation: impl FnOnce(&DesktopService) -> T + Send + 'static,
) -> Option<T> {
    let desktop = Arc::clone(desktop);
    tokio::task::spawn_blocking(move || operation(&desktop))
        .await
        .ok()
}

fn outcome_of(result: &Result<Value, DispatchError>) -> AgentDeliveryOutcome {
    match result {
        Ok(_) => AgentDeliveryOutcome::Sent,
        Err(error) if error.replay_safe => AgentDeliveryOutcome::Unsent,
        Err(_) => AgentDeliveryOutcome::Unknown,
    }
}

fn text_field(payload: &Value, name: &str) -> Option<String> {
    payload
        .get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn unsent(message: &str, retryable: bool) -> DispatchError {
    DispatchError::new(DEFINITELY_UNSENT_CODE, message, retryable, true)
}

fn still_checking() -> DispatchError {
    unsent(STILL_CHECKING, false)
}

fn unknown(message: &str) -> DispatchError {
    DispatchError::new(OUTCOME_UNKNOWN_CODE, message, false, false)
}

fn invalid(message: &str) -> DispatchError {
    DispatchError::new(INVALID_PAYLOAD_CODE, message, false, true)
}

#[cfg(test)]
#[path = "agents_tests.rs"]
mod tests;
