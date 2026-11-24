//! Durable contracts, resource leases and evidence for agent work.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::{
    agent_work::{
        AgentCalendarSchedule, AgentConsultation, AgentEvidence, AgentExecutionState,
        AgentFactState, AgentMemoryFact, AgentMemoryRevision, AgentTaskRecord, AgentToolReceipt,
        AgentVerificationState, AgentWorkSpec, AgentWorkView,
    },
    engine::{EngineEvent, EngineEventKind, SessionStatus},
};
use interface_storage::{JsonLimits, sha256_hex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Instant,
};

const BYTES: usize = 32 * 1024 * 1024;
const TASKS: usize = 500;
fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}
fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 24, 500_000)?)
}
fn bounded(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.contains('\0')
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    revision: u64,
    specs: BTreeMap<String, AgentWorkSpec>,
    #[serde(default)]
    calendars: BTreeMap<String, AgentCalendarSchedule>,
    tasks: Vec<AgentTaskRecord>,
    history: BTreeMap<String, Vec<AgentMemoryRevision>>,
    facts: BTreeMap<String, Vec<AgentMemoryFact>>,
    #[serde(default)]
    tool_calls: BTreeMap<String, BTreeMap<String, AgentToolReceipt>>,
    #[serde(default)]
    pending_approvals: Vec<interface_contracts::agents::AgentPendingApproval>,
    #[serde(default)]
    expired_approvals: Vec<interface_contracts::agents::AgentPendingApproval>,
    action_ids: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    consultations: BTreeMap<String, AgentConsultation>,
}
pub struct AgentWorkStore {
    document: LeasedDocument,
    state: State,
    early: BTreeMap<String, Vec<EngineEvent>>,
    early_overflow: bool,
    clocks: BTreeMap<String, (Instant, Instant)>,
}

/// # Errors
/// Rejects impossible or unbounded contracts before any provider receives them.
pub fn validate_agent_work_spec(spec: &AgentWorkSpec) -> Result<(), SettingsError> {
    if !bounded(&spec.expected_output, 8000)
        || spec.artifacts.len() > 32
        || spec.inputs.len() > 32
        || !(10..=86_400).contains(&spec.budget.wall_seconds)
        || !(10..=spec.budget.wall_seconds).contains(&spec.budget.no_progress_seconds)
        || !(1..=2000).contains(&spec.budget.max_actions)
        || spec
            .budget
            .max_reported_tokens
            .is_some_and(|n| !(1..=10_000_000).contains(&n))
        || spec.artifacts.iter().any(|v| {
            v.path.is_empty()
                || !bounded(&v.path, 4096)
                || v.contains.as_ref().is_some_and(|s| !bounded(s, 8000))
        })
        || spec.inputs.iter().any(|v| {
            v.path.is_empty()
                || !bounded(&v.path, 4096)
                || v.max_age_seconds == 0
                || v.max_age_seconds > 31_536_000
        })
    {
        return Err(invalid(
            "Use bounded deadlines, actions, input freshness and artifact checks.",
        ));
    }
    if let Some(grant) = &spec.consultation {
        let model = grant
            .model
            .strip_prefix("claude/")
            .or_else(|| grant.model.strip_prefix("codex/"));
        if model.is_none_or(|m| {
            m.is_empty()
                || m.len() > 100
                || !m
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_ .".contains(&b))
                || m.contains(' ')
        }) || grant.files.is_empty()
            || grant.files.len() > 16
            || !(1..=4).contains(&grant.max_children)
            || grant
                .files
                .iter()
                .any(|p| p.is_empty() || !bounded(p, 4096))
            || grant
                .writable_files
                .iter()
                .any(|p| !grant.files.contains(p))
        {
            return Err(invalid(
                "Choose an explicit Claude/Codex model, up to 16 selected files and 1–4 consultations; writable files must be selected inputs.",
            ));
        }
    }
    Ok(())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_state(state: &State) -> Result<(), SettingsError> {
    let ids: BTreeSet<_> = state.tasks.iter().map(|t| &t.id).collect();
    if state.version != 1
        || state.tasks.len() > TASKS
        || ids.len() != state.tasks.len()
        || state.specs.len() > 50
        || state.calendars.len() > 200
        || state.history.len() > 50
        || state.facts.len() > 50
        || state.consultations.len() > 256
        || state.pending_approvals.len() > 4096
        || state.expired_approvals.len() > 2000
        || state.tasks.iter().any(|t| {
            t.id.is_empty()
                || !bounded(&t.id, 128)
                || t.agent_id.is_empty()
                || !bounded(&t.agent_id, 128)
                || !bounded(&t.goal, 32_000)
                || t.roots.len() > 18
                || !Path::new(&t.cwd).is_absolute()
                || t.roots
                    .iter()
                    .any(|r| !Path::new(r).is_absolute() || !bounded(r, 4096))
                || t.notes.len() > 20
                || t.notes.iter().any(|n| !bounded(n, 2000))
                || t.artifacts.len() > 32
                || t.input_evidence.len() > 32
                || t.artifacts
                    .iter()
                    .chain(&t.input_evidence)
                    .any(|e| !valid_hash(&e.sha256) || !Path::new(&e.path).is_absolute())
                || (t.verification == AgentVerificationState::Verified
                    && (t.execution != AgentExecutionState::Completed
                        || t.artifacts.is_empty()
                        || t.artifacts.len() != t.spec.artifacts.len()))
        })
        || state.history.values().any(|h| {
            h.len() > 20
                || h.iter().any(|r| {
                    !bounded(&r.text, 256 * 1024) || r.sha256 != sha256_hex(r.text.as_bytes())
                })
        })
        || state.facts.values().any(|f| f.len() > 100)
        || state.tool_calls.iter().any(|(id, calls)| {
            !ids.contains(id)
                || calls.len() > 2000
                || calls.values().any(|r| {
                    !valid_hash(&r.fingerprint)
                        || r.result.as_ref().is_some_and(|s| s.len() > 256 * 1024)
                })
        })
    {
        return Err(invalid("Invalid agent work ledger."));
    }
    for spec in state
        .specs
        .values()
        .chain(state.tasks.iter().map(|t| &t.spec))
    {
        validate_agent_work_spec(spec)?;
    }
    for calendar in state.calendars.values() {
        crate::validate_calendar(calendar)?;
    }
    for child in state.consultations.values() {
        if !ids.contains(&child.task_id)
            || !matches!(
                child.status.as_str(),
                "pending" | "completed" | "failed" | "unknown"
            )
            || child.inputs.len() > 16
            || child.proposals.len() > 16
            || !bounded(&child.report, 16_384)
            || child.proposals.iter().map(|p| p.text.len()).sum::<usize>() > 64 * 1024
        {
            return Err(invalid("Invalid consultation ledger."));
        }
    }
    Ok(())
}

impl AgentWorkStore {
    /// # Errors
    /// Refuses corrupt/unsafe stores; recovers active effects as unknown without replay.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state) = LeasedDocument::open::<State>(
            path,
            ".agent-work.lock",
            limits()?,
            "Agent work storage is uncertain. Admission is closed until restart.",
        )?;
        let state = state.unwrap_or_else(|| State {
            version: 1,
            revision: 0,
            specs: BTreeMap::new(),
            calendars: BTreeMap::new(),
            tasks: vec![],
            history: BTreeMap::new(),
            facts: BTreeMap::new(),
            tool_calls: BTreeMap::new(),
            action_ids: BTreeMap::new(),
            pending_approvals: vec![],
            expired_approvals: vec![],
            consultations: BTreeMap::new(),
        });
        validate_state(&state)?;
        let mut store = Self {
            document,
            state,
            early: BTreeMap::new(),
            early_overflow: false,
            clocks: BTreeMap::new(),
        };
        let mut recovered = store.state.clone();
        let mut changed = false;
        for task in &mut recovered.tasks {
            if matches!(
                task.execution,
                AgentExecutionState::Starting | AgentExecutionState::Running
            ) {
                task.execution = AgentExecutionState::Unknown;
                task.stop_requested = true;
                task.detail = "Host stopped during this task. No work was replayed.".into();
                task.next_action =
                    "Inspect provider history and artifacts, then acknowledge the uncertain task."
                        .into();
                changed = true;
            }
        }
        for child in recovered.consultations.values_mut() {
            if child.status == "pending" {
                child.status = "unknown".into();
                child.report = "Host stopped during consultation; no request was replayed.".into();
                changed = true;
            }
        }
        if !recovered.pending_approvals.is_empty() {
            recovered
                .expired_approvals
                .append(&mut recovered.pending_approvals);
            let excess = recovered.expired_approvals.len().saturating_sub(2000);
            recovered.expired_approvals.drain(..excess);
            changed = true;
        }
        if changed {
            store.commit(recovered)?;
        }
        Ok(store)
    }
    /// The refusal `commit` makes before writing; an event that changes nothing still meets it.
    fn writable(&self) -> Result<(), SettingsError> {
        self.document.ensure_writable()
    }
    fn commit(&mut self, mut state: State) -> Result<(), SettingsError> {
        self.writable()?;
        state.revision = self
            .state
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("Work revision exhausted."))?;
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
    #[must_use]
    pub fn view(&self, agent: &str) -> AgentWorkView {
        AgentWorkView {
            version: 1,
            revision: self.state.revision,
            spec: self.spec(agent),
            tasks: self
                .state
                .tasks
                .iter()
                .filter(|t| t.agent_id == agent)
                .cloned()
                .collect(),
            memory_history: self.state.history.get(agent).cloned().unwrap_or_default(),
            facts: self.state.facts.get(agent).cloned().unwrap_or_default(),
            expired_approvals: self
                .state
                .expired_approvals
                .iter()
                .filter(|p| p.agent_id == agent)
                .cloned()
                .collect(),
            consultations: self
                .state
                .consultations
                .values()
                .filter(|child| {
                    self.state
                        .tasks
                        .iter()
                        .any(|t| t.id == child.task_id && t.agent_id == agent)
                })
                .cloned()
                .collect(),
        }
    }
    /// # Errors
    /// Unresolved owner decisions are retained; capacity refuses admission, never evicts them.
    pub fn pending_approval(
        &mut self,
        pending: interface_contracts::agents::AgentPendingApproval,
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        next.pending_approvals.retain(|p| {
            p.session_key != pending.session_key || p.approval_id != pending.approval_id
        });
        if next.pending_approvals.len() >= 4096 {
            return Err(invalid(
                "Unresolved approval capacity reached; close the provider.",
            ));
        }
        next.pending_approvals.push(pending);
        self.commit(next)
    }
    /// # Errors
    /// Explicit resolution or expiration is durable; startup expires all prior process requests.
    pub fn finish_approvals(
        &mut self,
        key: &str,
        approval: Option<&str>,
        expired: bool,
    ) -> Result<(), SettingsError> {
        let matches = |p: &interface_contracts::agents::AgentPendingApproval| {
            p.session_key == key && approval.is_none_or(|id| p.approval_id == id)
        };
        // An automatically answered approval never became pending: answer
        // that from the ledger before copying it.
        if !self.state.pending_approvals.iter().any(matches) {
            return Ok(());
        }
        let mut next = self.state.clone();
        let mut removed = Vec::new();
        next.pending_approvals.retain(|p| {
            if matches(p) {
                removed.push(p.clone());
                false
            } else {
                true
            }
        });
        if removed.is_empty() {
            return Ok(());
        }
        if expired {
            next.expired_approvals.extend(removed);
            let excess = next.expired_approvals.len().saturating_sub(2000);
            next.expired_approvals.drain(..excess);
        }
        self.commit(next)
    }
    #[must_use]
    pub fn calendar(&self, routine: &str) -> Option<AgentCalendarSchedule> {
        self.state.calendars.get(routine).cloned()
    }
    /// # Errors
    /// Rejects malformed timezone schedules before persistence.
    pub fn save_calendar(
        &mut self,
        routine: &str,
        schedule: Option<AgentCalendarSchedule>,
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        if let Some(schedule) = schedule {
            crate::validate_calendar(&schedule)?;
            if !next.calendars.contains_key(routine) && next.calendars.len() >= 200 {
                return Err(invalid("Calendar capacity reached."));
            }
            next.calendars.insert(routine.into(), schedule);
        } else {
            next.calendars.remove(routine);
        }
        self.commit(next)
    }
    /// # Errors
    /// Removes a deleted owner's private metadata only after every resource lease settled.
    pub fn forget_agent(&mut self, agent: &str, routines: &[String]) -> Result<(), SettingsError> {
        if self
            .state
            .tasks
            .iter()
            .any(|t| t.agent_id == agent && t.execution.holds_resources())
        {
            return Err(invalid(
                "Inspect and acknowledge this agent's unfinished work before removing it.",
            ));
        }
        let mut next = self.state.clone();
        let ids: BTreeSet<_> = next
            .tasks
            .iter()
            .filter(|t| t.agent_id == agent)
            .map(|t| t.id.clone())
            .collect();
        next.tasks.retain(|t| t.agent_id != agent);
        next.specs.remove(agent);
        next.history.remove(agent);
        next.facts.remove(agent);
        next.calendars.retain(|id, _| !routines.contains(id));
        next.consultations.retain(|_, c| !ids.contains(&c.task_id));
        next.tool_calls.retain(|id, _| !ids.contains(id));
        next.action_ids.retain(|id, _| !ids.contains(id));
        next.pending_approvals.retain(|p| p.agent_id != agent);
        next.expired_approvals.retain(|p| p.agent_id != agent);
        self.clocks.retain(|id, _| !ids.contains(id));
        self.commit(next)
    }
    #[must_use]
    pub fn spec(&self, agent: &str) -> AgentWorkSpec {
        self.state.specs.get(agent).cloned().unwrap_or_default()
    }
    #[must_use]
    pub fn consultation(&self, task: &str, id: &str) -> Option<AgentConsultation> {
        self.state
            .consultations
            .get(id)
            .filter(|child| child.task_id == task)
            .cloned()
    }
    /// # Errors
    /// Admission and report updates are durable; unresolved effects never become retryable.
    pub fn save_consultation(&mut self, child: AgentConsultation) -> Result<(), SettingsError> {
        let parent = self
            .state
            .tasks
            .iter()
            .find(|task| task.id == child.task_id)
            .ok_or_else(|| invalid("Missing parent task."))?;
        let grant = parent
            .spec
            .consultation
            .as_ref()
            .ok_or_else(|| invalid("No owner consultation grant."))?;
        let existing = self.state.consultations.contains_key(&child.id);
        if child.model != grant.model
            || child.inputs.len() > 16
            || child.proposals.len() > 16
            || !bounded(&child.report, 16_384)
            || !bounded(&child.brief, 8000)
            || child.proposals.iter().map(|p| p.text.len()).sum::<usize>() > 64 * 1024
            || (!existing
                && (self.state.consultations.len() >= 256
                    || self
                        .state
                        .consultations
                        .values()
                        .filter(|c| c.task_id == child.task_id)
                        .count()
                        >= usize::from(grant.max_children)))
        {
            return Err(invalid("Consultation scope or capacity exceeded."));
        }
        let mut next = self.state.clone();
        next.consultations.insert(child.id.clone(), child);
        self.commit(next)
    }
    /// # Errors
    /// Refuses stale revisions or invalid specifications.
    pub fn configure(
        &mut self,
        agent: &str,
        revision: u64,
        spec: AgentWorkSpec,
    ) -> Result<(), SettingsError> {
        if revision != self.state.revision {
            return Err(invalid("Task settings changed. Reload before saving."));
        }
        validate_agent_work_spec(&spec)?;
        let mut next = self.state.clone();
        if !next.specs.contains_key(agent) && next.specs.len() >= 50 {
            return Err(invalid("Task settings capacity reached."));
        }
        next.specs.insert(agent.into(), spec);
        self.commit(next)
    }
    /// # Errors
    /// Refuses overlapping writers, unresolved work and exhausted retention capacity.
    pub fn admit(&mut self, mut task: AgentTaskRecord) -> Result<(), SettingsError> {
        validate_agent_work_spec(&task.spec)?;
        if self.state.tasks.iter().any(|t| t.id == task.id) {
            return Err(invalid(
                "Task identity already exists; no work was repeated.",
            ));
        }
        if self
            .state
            .tasks
            .iter()
            .filter(|t| t.execution.holds_resources())
            .any(|t| {
                t.agent_id == task.agent_id
                    || t.roots.iter().any(|a| {
                        task.roots
                            .iter()
                            .any(|b| Path::new(a).starts_with(b) || Path::new(b).starts_with(a))
                    })
            })
        {
            return Err(invalid(
                "Another active or uncertain task holds these folders. Finish or acknowledge it before starting.",
            ));
        }
        let mut next = self.state.clone();
        if next.tasks.len() >= TASKS {
            let index = next
                .tasks
                .iter()
                .position(|t| !t.execution.holds_resources())
                .ok_or_else(|| invalid("Task history is full of unresolved work."))?;
            let removed = next.tasks.remove(index);
            next.action_ids.remove(&removed.id);
            next.tool_calls.remove(&removed.id);
            next.consultations
                .retain(|_, child| child.task_id != removed.id);
            self.clocks.remove(&removed.id);
        }
        task.deadline_at_ms = task
            .started_at_ms
            .saturating_add(u64::from(task.spec.budget.wall_seconds) * 1000);
        task.progress_at_ms = task.started_at_ms;
        let id = task.id.clone();
        next.tasks.push(task);
        self.commit(next)?;
        self.clocks.insert(id, (Instant::now(), Instant::now()));
        Ok(())
    }
    /// # Errors
    /// Refuses unknown task identity or unsafe persistence.
    pub fn bind(&mut self, id: &str, key: &str) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| invalid("Unknown task."))?;
        task.session_key = Some(key.into());
        task.execution = if self.early_overflow {
            task.stop_requested = true;
            task.detail = "Startup event capacity exceeded; provider closure is required.".into();
            AgentExecutionState::Unknown
        } else {
            AgentExecutionState::Running
        };
        self.commit(next)?;
        let events = self.early.remove(key).unwrap_or_default();
        for event in events {
            self.observe(
                key,
                &event,
                self.state
                    .tasks
                    .iter()
                    .find(|t| t.id == id)
                    .map_or(0, |t| t.started_at_ms),
            )?;
        }
        self.release_startup_buffer();
        Ok(())
    }
    /// Startup events are buffered only while some task is starting. Once none
    /// is, neither the events nor an overflow may reach a later task, whether
    /// the last starting task bound or failed before it could.
    fn release_startup_buffer(&mut self) {
        if !self
            .state
            .tasks
            .iter()
            .any(|t| t.execution == AgentExecutionState::Starting)
        {
            self.early.clear();
            self.early_overflow = false;
        }
    }
    /// One agent's tasks in stored order, borrowed.
    pub fn tasks_of(&self, agent: &str) -> impl Iterator<Item = &AgentTaskRecord> {
        self.state.tasks.iter().filter(move |t| t.agent_id == agent)
    }
    /// One agent's remembered memory revisions, oldest first, borrowed.
    #[must_use]
    pub fn memory_history_of(&self, agent: &str) -> &[AgentMemoryRevision] {
        self.state.history.get(agent).map_or(&[], Vec::as_slice)
    }
    #[must_use]
    pub fn task(&self, agent: &str, id: &str) -> Option<AgentTaskRecord> {
        self.state
            .tasks
            .iter()
            .find(|t| t.agent_id == agent && t.id == id)
            .cloned()
    }
    #[must_use]
    pub fn active(&self, key: &str) -> Option<AgentTaskRecord> {
        self.state
            .tasks
            .iter()
            .rev()
            .find(|t| t.session_key.as_deref() == Some(key) && t.execution.holds_resources())
            .cloned()
    }
    /// # Errors
    /// An uncertain write closes admission; no model claim changes verification state.
    #[allow(clippy::too_many_lines)] // Keep the task event transition table together.
    pub fn observe(
        &mut self,
        key: &str,
        event: &EngineEvent,
        now: u64,
    ) -> Result<(), SettingsError> {
        let Some(index) =
            self.state.tasks.iter().rposition(|t| {
                t.session_key.as_deref() == Some(key) && t.execution.holds_resources()
            })
        else {
            if self
                .state
                .tasks
                .iter()
                .any(|t| t.execution == AgentExecutionState::Starting)
            {
                if self.early.values().map(Vec::len).sum::<usize>() >= 4096 {
                    // Any session's key can land here, so the overflow never fails this one:
                    // `bind` turns it into an uncertain, stop-requested starting task.
                    self.early_overflow = true;
                    return Ok(());
                }
                self.early
                    .entry(key.into())
                    .or_default()
                    .push(event.clone());
            }
            return Ok(());
        };
        if self.state.tasks[index].execution == AgentExecutionState::Unknown {
            return Ok(());
        }
        // Decide on a copy of the one task; the whole store is cloned only for a real change.
        let mut task = self.state.tasks[index].clone();
        let mut action = None;
        match &event.event {
            EngineEventKind::ToolCall { call_id, .. } => {
                if self
                    .state
                    .action_ids
                    .get(&task.id)
                    .is_some_and(|ids| ids.contains(call_id))
                {
                    return Ok(());
                }
                action = Some(call_id.clone());
                task.began = true;
                task.actions = task.actions.saturating_add(1);
            }
            EngineEventKind::ToolResult { ok, summary, .. } if *ok => {
                let hash = sha256_hex(summary.as_bytes());
                if hash == task.progress_sha256 {
                    return Ok(());
                }
                task.progress_sha256 = hash;
                task.progress_at_ms = now;
                if let Some((_, progress)) = self.clocks.get_mut(&task.id) {
                    *progress = Instant::now();
                }
            }
            EngineEventKind::UsageUpdated {
                input_tokens,
                output_tokens,
                ..
            } => {
                task.reported_tokens = Some(
                    task.reported_tokens
                        .unwrap_or(0)
                        .max(input_tokens.saturating_add(*output_tokens)),
                );
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Working,
            }
            | EngineEventKind::UserMessage { .. } => {
                task.began = true;
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } if task.began => {
                task.execution = if task.stop_requested {
                    AgentExecutionState::Stopped
                } else {
                    AgentExecutionState::Completed
                };
                if !task.stop_requested {
                    task.detail = "Provider turn ended; artifact verification is separate.".into();
                    task.next_action =
                        "Verify expected outputs and inspect the conversation.".into();
                }
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Failed,
            } => {
                task.execution = AgentExecutionState::Failed;
                task.detail = "Provider reported failure.".into();
            }
            EngineEventKind::SessionClosed => {
                task.execution = if task.stop_requested {
                    AgentExecutionState::Stopped
                } else {
                    AgentExecutionState::Unknown
                };
                if !task.stop_requested {
                    task.detail =
                        "Provider session closed. Inspect artifacts before continuing.".into();
                }
            }
            _ => return Ok(()),
        }
        if action.is_none() && task == self.state.tasks[index] {
            // A repeated turn start or an unchanged token count: nothing to write.
            return self.writable();
        }
        let mut next = self.state.clone();
        if let Some(call_id) = action {
            next.action_ids
                .entry(task.id.clone())
                .or_default()
                .insert(call_id);
        }
        next.tasks[index] = task;
        self.commit(next)
    }
    /// # Errors
    /// Stop is durable before the host sends an interrupt or closes a process.
    pub fn stop(
        &mut self,
        agent: &str,
        id: &str,
        detail: &str,
    ) -> Result<Option<String>, SettingsError> {
        let mut next = self.state.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.agent_id == agent && t.id == id)
            .ok_or_else(|| invalid("Unknown task."))?;
        task.stop_requested = true;
        task.detail = detail.into();
        task.next_action =
            "Inspect the last artifacts and unresolved decisions before starting new work.".into();
        let key = task.session_key.clone();
        self.commit(next)?;
        Ok(key)
    }
    /// # Errors
    /// Used only after confirmed session closure or an explicit owner acknowledgement.
    pub fn settle(
        &mut self,
        agent: &str,
        id: &str,
        execution: AgentExecutionState,
        detail: &str,
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.agent_id == agent && t.id == id)
            .ok_or_else(|| invalid("Unknown task."))?;
        task.execution = execution;
        if execution != AgentExecutionState::Completed
            && task.verification == AgentVerificationState::Verified
        {
            task.verification = AgentVerificationState::Stale;
        }
        task.detail = detail.into();
        self.commit(next)?;
        self.release_startup_buffer();
        Ok(())
    }
    #[must_use]
    pub fn expired(&self, now: u64) -> Vec<AgentTaskRecord> {
        self.budgeted()
            .filter(|t| self.is_expired(t, now))
            .cloned()
            .collect()
    }
    /// Whether a budgeted task has run out of wall time, progress time,
    /// actions or reported tokens.
    fn is_expired(&self, t: &AgentTaskRecord, now: u64) -> bool {
        self.clock_expired(t)
            || now >= t.deadline_at_ms
            || now.saturating_sub(t.progress_at_ms)
                >= u64::from(t.spec.budget.no_progress_seconds) * 1000
            || t.actions >= t.spec.budget.max_actions
            || t.spec.budget.max_reported_tokens.is_some_and(|max| {
                t.reported_tokens.unwrap_or(0).saturating_add(
                    self.state
                        .consultations
                        .values()
                        .filter(|c| c.task_id == t.id)
                        .filter_map(|c| c.reported_tokens)
                        .sum::<u64>(),
                ) >= max
            })
    }
    /// Whether any task is still under its budgets. Only [`Self::admit`] adds
    /// one, so with none, [`Self::expired`] stays empty until an admission.
    #[must_use]
    pub fn has_budgeted(&self) -> bool {
        self.budgeted().next().is_some()
    }
    /// The only tasks [`Self::expired`] can return.
    fn budgeted(&self) -> impl Iterator<Item = &AgentTaskRecord> {
        self.state.tasks.iter().filter(|t| {
            matches!(
                t.execution,
                AgentExecutionState::Starting | AgentExecutionState::Running
            ) && !t.stop_requested
        })
    }
    fn clock_expired(&self, task: &AgentTaskRecord) -> bool {
        self.clocks.get(&task.id).is_some_and(|(start, progress)| {
            start.elapsed().as_secs() >= u64::from(task.spec.budget.wall_seconds)
                || progress.elapsed().as_secs() >= u64::from(task.spec.budget.no_progress_seconds)
        })
    }
    /// Admission uses both persisted limits and monotonic process time.
    #[must_use]
    pub fn admits(&self, task: &AgentTaskRecord, now: u64) -> bool {
        !self.document.is_uncertain()
            && !task.stop_requested
            && matches!(
                task.execution,
                AgentExecutionState::Starting | AgentExecutionState::Running
            )
            && !self
                .budgeted()
                .any(|stored| stored.id == task.id && self.is_expired(stored, now))
    }
    /// # Errors
    /// Only deterministic host checks call this; execution must have completed.
    pub fn verification(
        &mut self,
        agent: &str,
        id: &str,
        status: AgentVerificationState,
        evidence: Vec<AgentEvidence>,
        detail: String,
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.agent_id == agent && t.id == id)
            .ok_or_else(|| invalid("Unknown task."))?;
        if status == AgentVerificationState::Verified
            && (task.execution != AgentExecutionState::Completed
                || evidence.is_empty()
                || evidence.len() != task.spec.artifacts.len())
        {
            return Err(invalid(
                "Only completed tasks with all expected artifacts can be verified.",
            ));
        }
        task.verification = status;
        task.artifacts = evidence;
        task.detail = detail;
        self.commit(next)
    }
    /// # Errors
    /// Retains bounded owner-private revisions without trusting an agent claim as approval.
    pub fn remember_memory(
        &mut self,
        agent: &str,
        text: &str,
        source: &str,
        now: u64,
    ) -> Result<(), SettingsError> {
        if text.len() > 256 * 1024 {
            return Err(invalid("Memory exceeds 256 KiB."));
        }
        let hash = sha256_hex(text.as_bytes());
        if self
            .state
            .history
            .get(agent)
            .and_then(|h| h.last())
            .is_some_and(|r| r.sha256 == hash)
        {
            return Ok(());
        }
        let mut next = self.state.clone();
        if !next.history.contains_key(agent) && next.history.len() >= 50 {
            return Err(invalid("Memory owner capacity exhausted."));
        }
        let history = next.history.entry(agent.into()).or_default();
        history.push(AgentMemoryRevision {
            sha256: hash,
            text: text.into(),
            source: source.into(),
            observed_at_ms: now,
        });
        if history.len() > 20 {
            history.remove(0);
        }
        self.commit(next)
    }
    /// # Errors
    /// Reserves each tool request once. An interrupted attempt stays unknown after restart.
    pub fn reserve_tool(
        &mut self,
        task: &str,
        request: &str,
        input: &[u8],
    ) -> Result<Option<String>, SettingsError> {
        if request.trim().is_empty() || !bounded(request, 128) {
            return Err(invalid("Tool request needs a bounded requestId."));
        }
        let hash = sha256_hex(input);
        if let Some(receipt) = self
            .state
            .tool_calls
            .get(task)
            .and_then(|calls| calls.get(request))
        {
            if receipt.fingerprint != hash {
                return Err(invalid(
                    "Tool requestId already belongs to different input.",
                ));
            }
            return receipt.result.clone().map(Some).ok_or_else(|| {
                invalid("This tool request has an unknown outcome and will not be repeated.")
            });
        }
        let mut next = self.state.clone();
        let parent = self
            .state
            .tasks
            .iter()
            .find(|t| t.id == task)
            .ok_or_else(|| invalid("Missing tool task."))?;
        let calls = next.tool_calls.entry(task.into()).or_default();
        if calls.len() >= parent.spec.budget.max_actions as usize {
            return Err(invalid("Task tool request capacity exhausted."));
        }
        calls.insert(
            request.into(),
            AgentToolReceipt {
                fingerprint: hash,
                result: None,
            },
        );
        self.commit(next)?;
        Ok(None)
    }
    /// # Errors
    /// Outcome persistence failures keep the one-attempt fence in place.
    pub fn settle_tool(
        &mut self,
        task: &str,
        request: &str,
        result: String,
    ) -> Result<(), SettingsError> {
        if !bounded(&result, 256 * 1024) {
            return Err(invalid("Tool result exceeds 256 KiB."));
        }
        let mut next = self.state.clone();
        let receipt = next
            .tool_calls
            .get_mut(task)
            .and_then(|calls| calls.get_mut(request))
            .ok_or_else(|| invalid("Missing tool intent."))?;
        receipt.result = Some(result);
        self.commit(next)
    }
    /// # Errors
    /// Captures an artifact without upgrading model claims into verified outcomes.
    pub fn register_artifact(
        &mut self,
        agent: &str,
        id: &str,
        proof: AgentEvidence,
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.agent_id == agent && t.id == id)
            .ok_or_else(|| invalid("Unknown task."))?;
        task.artifacts.retain(|prior| prior.path != proof.path);
        if task.artifacts.len() >= 32 {
            return Err(invalid("Task artifact capacity exhausted."));
        }
        task.progress_at_ms = proof.checked_at_ms;
        task.artifacts.push(proof);
        task.verification = AgentVerificationState::Unverified;
        self.commit(next)
    }
    /// # Errors
    /// Owner-directed requests are retained in the result for review, never sent to third parties.
    pub fn note(&mut self, agent: &str, id: &str, note: &str) -> Result<(), SettingsError> {
        if note.trim().is_empty() || !bounded(note, 2000) {
            return Err(invalid(
                "A task note must be nonempty and at most 2000 bytes.",
            ));
        }
        let mut next = self.state.clone();
        let task = next
            .tasks
            .iter_mut()
            .find(|t| t.agent_id == agent && t.id == id)
            .ok_or_else(|| invalid("Unknown task."))?;
        if task.notes.len() >= 20 {
            return Err(invalid("Task note capacity exhausted."));
        }
        task.notes.push(note.into());
        self.commit(next)
    }
    /// # Errors
    /// Facts enter as unverified proposals; only a separate owner command confirms them.
    pub fn propose_fact(
        &mut self,
        agent: &str,
        mut fact: AgentMemoryFact,
        now: u64,
    ) -> Result<(), SettingsError> {
        if fact.id.is_empty()
            || !bounded(&fact.id, 128)
            || fact.text.trim().is_empty()
            || !bounded(&fact.text, 8000)
            || fact.source.trim().is_empty()
            || !bounded(&fact.source, 2000)
            || fact.expires_at_ms.is_some_and(|at| at <= now)
        {
            return Err(invalid(
                "A memory fact needs bounded text, a source and a future expiry.",
            ));
        }
        fact.state = AgentFactState::Proposed;
        fact.observed_at_ms = now;
        let mut next = self.state.clone();
        if !next.facts.contains_key(agent) && next.facts.len() >= 50 {
            return Err(invalid("Fact owner capacity exhausted."));
        }
        let facts = next.facts.entry(agent.into()).or_default();
        if facts.len() >= 100 || facts.iter().any(|f| f.id == fact.id) {
            return Err(invalid(
                "Fact id already exists or memory fact capacity is full.",
            ));
        }
        if let Some(id) = &fact.supersedes
            && !facts.iter().any(|f| &f.id == id)
        {
            return Err(invalid("Superseded fact does not exist."));
        }
        facts.push(fact);
        self.commit(next)
    }
    /// # Errors
    /// This is owner-only, never exposed through the agent toolbelt.
    pub fn confirm_fact(&mut self, agent: &str, id: &str) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let facts = next
            .facts
            .get_mut(agent)
            .ok_or_else(|| invalid("Unknown fact."))?;
        let fact = facts
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or_else(|| invalid("Unknown fact."))?;
        fact.state = AgentFactState::OwnerConfirmed;
        let supersedes = fact.supersedes.clone();
        if let Some(old) = facts
            .iter_mut()
            .find(|f| Some(&f.id) == supersedes.as_ref())
        {
            old.state = AgentFactState::Superseded;
        }
        self.commit(next)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn task(id: &str, agent: &str, root: &str) -> AgentTaskRecord {
        AgentTaskRecord {
            id: id.into(),
            agent_id: agent.into(),
            session_key: None,
            goal: "Produce report".into(),
            spec: AgentWorkSpec::default(),
            roots: vec![root.into()],
            cwd: root.into(),
            started_at_ms: 1000,
            deadline_at_ms: 0,
            progress_at_ms: 0,
            progress_sha256: String::new(),
            began: false,
            actions: 0,
            reported_tokens: None,
            execution: AgentExecutionState::Starting,
            verification: AgentVerificationState::Unverified,
            input_evidence: vec![],
            artifacts: vec![],
            detail: String::new(),
            next_action: String::new(),
            stop_requested: false,
            notes: vec![],
        }
    }
    pub(crate) fn calendar_task(id: &str, agent: &str, cwd: &str, goal: &str) -> AgentTaskRecord {
        let mut task = task(id, agent, cwd);
        task.cwd = cwd.into();
        task.goal = goal.into();
        task
    }
    #[test]
    fn written_bytes_match_the_pinned_format() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("agent-work.v1.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        store.admit(task("1", "a", "/project")).unwrap();
        assert_eq!(store.reserve_tool("1", "call", b"input").unwrap(), None);
        crate::golden::assert_golden("agent-work.admitted.json", root.path(), &path);
        drop(store);
        // Reopening recovers the starting task as unknown and rewrites it.
        let store = AgentWorkStore::open(&path).unwrap();
        crate::golden::assert_golden("agent-work.recovered.json", root.path(), &path);
        assert!(root.path().join(".agent-work.lock").exists());
        drop(store);
    }
    #[test]
    fn finishing_an_approval_that_never_became_pending_commits_nothing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("agent-work.v1.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        store
            .pending_approval(interface_contracts::agents::AgentPendingApproval {
                agent_id: "one".into(),
                session_key: "claude#asked".into(),
                approval_id: "asked".into(),
                tool: "Bash".into(),
                summary: "needs review".into(),
                reason: "risky".into(),
                asked_at_ms: 1,
            })
            .unwrap();
        let revision = store.state.revision;
        let bytes = std::fs::read(&path).unwrap();
        // An automatic answer, and another session's approval id.
        store
            .finish_approvals("claude#auto", Some("asked"), false)
            .unwrap();
        store
            .finish_approvals("claude#asked", Some("other"), true)
            .unwrap();
        assert_eq!(store.state.revision, revision);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        store
            .finish_approvals("claude#asked", Some("asked"), false)
            .unwrap();
        assert_eq!(store.state.revision, revision + 1);
        assert!(store.state.pending_approvals.is_empty());
    }
    #[test]
    fn agent_approval_recovery_expires_all_pending_without_replaying_answers() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("work.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        for n in 0..70 {
            store
                .pending_approval(interface_contracts::agents::AgentPendingApproval {
                    agent_id: "one".into(),
                    session_key: "claude#closed".into(),
                    approval_id: format!("a{n}"),
                    tool: "Bash".into(),
                    summary: "needs review".into(),
                    reason: "risky".into(),
                    asked_at_ms: 1,
                })
                .unwrap();
        }
        drop(store);
        let store = AgentWorkStore::open(&path).unwrap();
        assert!(store.state.pending_approvals.is_empty());
        assert_eq!(store.view("one").expired_approvals.len(), 70);
    }
    #[test]
    fn agent_tool_intents_fence_unknown_outcomes_and_changed_input() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("work.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        store.admit(task("task", "one", "/fixture")).unwrap();
        assert_eq!(
            store.reserve_tool("task", "one", b"original").unwrap(),
            None
        );
        assert!(store.reserve_tool("task", "one", b"changed").is_err());
        drop(store);
        let mut store = AgentWorkStore::open(&path).unwrap();
        assert!(store.reserve_tool("task", "one", b"original").is_err());
    }
    #[test]
    fn admits_agrees_with_the_expired_listing_for_every_task() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = AgentWorkStore::open(&temp.path().join("work.json")).unwrap();
        for (n, agent) in ["a", "b", "c", "d"].into_iter().enumerate() {
            store
                .admit(task(&n.to_string(), agent, &format!("/fixture{n}")))
                .unwrap();
        }
        store.state.tasks[1].actions = store.state.tasks[1].spec.budget.max_actions;
        store.state.tasks[2].stop_requested = true;
        store.state.tasks[3].execution = AgentExecutionState::Completed;
        // A caller's stale copy of the exhausted task still answers from the store.
        let mut stale = store.state.tasks[1].clone();
        stale.actions = 0;
        let mut candidates = store.state.tasks.clone();
        candidates.push(stale);
        candidates.push(task("unknown", "z", "/elsewhere"));
        let deadline = store.state.tasks[0].deadline_at_ms;
        for now in [
            1000,
            1001,
            deadline.saturating_sub(1),
            deadline,
            deadline + 1,
        ] {
            let expired = store.expired(now);
            for candidate in &candidates {
                let listed = !store.document.is_uncertain()
                    && !candidate.stop_requested
                    && matches!(
                        candidate.execution,
                        AgentExecutionState::Starting | AgentExecutionState::Running
                    )
                    && !expired.iter().any(|e| e.id == candidate.id);
                assert_eq!(
                    store.admits(candidate, now),
                    listed,
                    "{} at {now}",
                    candidate.id
                );
            }
        }
        assert!(store.admits(&candidates[0], 1000));
        assert!(!store.admits(&candidates[4], 1000));
        assert!(!store.admits(&candidates[0], deadline));
    }
    fn event(value: serde_json::Value) -> EngineEvent {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn agent_work_recovers_unknown_and_retains_exclusive_resource_leases() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("work.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        store.admit(task("1", "a", "/project")).unwrap();
        assert!(store.admit(task("2", "b", "/project/sub")).is_err());
        store.admit(task("3", "c", "/different")).unwrap();
        drop(store);
        let mut store = AgentWorkStore::open(&path).unwrap();
        assert_eq!(
            store.task("a", "1").unwrap().execution,
            AgentExecutionState::Unknown
        );
        assert!(store.admit(task("2", "b", "/project")).is_err());
        store
            .settle(
                "a",
                "1",
                AgentExecutionState::Stopped,
                "Acknowledged after inspection",
            )
            .unwrap();
        store.admit(task("2", "b", "/project")).unwrap();
    }
    #[test]
    fn agent_work_idle_does_not_imply_verified_and_early_events_are_reconciled() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = AgentWorkStore::open(&temp.path().join("work.json")).unwrap();
        store.admit(task("1", "a", "/project")).unwrap();
        store
            .observe(
                "claude#1",
                &event(serde_json::json!({"kind":"status.changed","status":"working"})),
                1200,
            )
            .unwrap();
        store
            .observe(
                "claude#1",
                &event(serde_json::json!({"kind":"status.changed","status":"idle"})),
                1300,
            )
            .unwrap();
        store.bind("1", "claude#1").unwrap();
        let finished = store.task("a", "1").unwrap();
        assert_eq!(finished.execution, AgentExecutionState::Completed);
        assert_eq!(finished.verification, AgentVerificationState::Unverified);
        assert!(
            store
                .verification(
                    "a",
                    "1",
                    AgentVerificationState::Verified,
                    vec![],
                    "Claimed done".into()
                )
                .is_err()
        );
    }
    #[test]
    fn unchanged_turn_starts_and_token_counts_write_nothing() {
        use std::os::unix::fs::MetadataExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("work.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        store.admit(task("1", "a", "/project")).unwrap();
        store.bind("1", "claude#1").unwrap();
        let working = event(serde_json::json!({"kind":"status.changed","status":"working"}));
        let usage = |input: u64, output: u64| {
            event(
                serde_json::json!({"kind":"usage.updated","inputTokens":input,"outputTokens":output}),
            )
        };
        store.observe("claude#1", &working, 1100).unwrap();
        store.observe("claude#1", &usage(10, 5), 1150).unwrap();
        let written = || {
            (
                store_revision(&path),
                std::fs::metadata(&path).unwrap().ino(),
            )
        };
        let before = written();
        store.observe("claude#1", &working, 1200).unwrap();
        store.observe("claude#1", &usage(10, 5), 1250).unwrap();
        store.observe("claude#1", &usage(4, 1), 1300).unwrap();
        assert_eq!(written(), before, "no-op events leave the file untouched");
        assert_eq!(store.state.revision, before.0);
        assert_eq!(store.task("a", "1").unwrap().reported_tokens, Some(15));
        store.observe("claude#1", &usage(20, 5), 1350).unwrap();
        assert_eq!(written().0, before.0 + 1);
        assert_eq!(store.task("a", "1").unwrap().reported_tokens, Some(25));
    }
    fn store_revision(path: &Path) -> u64 {
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        value["revision"].as_u64().unwrap()
    }
    #[test]
    fn startup_event_overflow_never_fails_another_session() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = AgentWorkStore::open(&temp.path().join("work.json")).unwrap();
        store.admit(task("1", "a", "/project")).unwrap();
        let working = event(serde_json::json!({"kind":"status.changed","status":"working"}));
        for _ in 0..=4096 {
            store.observe("codex#plain", &working, 1100).unwrap();
        }
        store.bind("1", "claude#1").unwrap();
        let started = store.task("a", "1").unwrap();
        assert_eq!(started.execution, AgentExecutionState::Unknown);
        assert!(started.stop_requested);
        assert!(store.early.is_empty() && !store.early_overflow);
    }
    #[test]
    fn a_start_that_fails_before_binding_leaves_no_startup_state_behind() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = AgentWorkStore::open(&temp.path().join("work.json")).unwrap();
        store.admit(task("1", "a", "/project")).unwrap();
        let working = event(serde_json::json!({"kind":"status.changed","status":"working"}));
        store.observe("claude#2", &working, 1100).unwrap();
        for _ in 0..=4096 {
            store.observe("codex#plain", &working, 1100).unwrap();
        }
        store
            .settle(
                "a",
                "1",
                AgentExecutionState::Failed,
                "Provider did not start.",
            )
            .unwrap();
        assert!(store.early.is_empty() && !store.early_overflow);
        store.admit(task("2", "a", "/project")).unwrap();
        store.bind("2", "claude#2").unwrap();
        let started = store.task("a", "2").unwrap();
        assert_eq!(started.execution, AgentExecutionState::Running);
        assert!(!started.stop_requested);
    }
    #[test]
    fn agent_work_deadline_action_deduplication_and_stop_are_durable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("work.json");
        let mut store = AgentWorkStore::open(&path).unwrap();
        let mut record = task("1", "a", "/project");
        record.spec.budget.max_actions = 1;
        store.admit(record).unwrap();
        store.bind("1", "claude#1").unwrap();
        let call = event(
            serde_json::json!({"kind":"tool.call","callId":"1","tool":"Read","summary":"Read input","input":{}}),
        );
        store.observe("claude#1", &call, 1100).unwrap();
        store.observe("claude#1", &call, 1200).unwrap();
        assert_eq!(store.task("a", "1").unwrap().actions, 1);
        assert_eq!(store.expired(1200).len(), 1);
        assert_eq!(
            store.stop("a", "1", "Budget exhausted").unwrap().as_deref(),
            Some("claude#1")
        );
        assert!(store.expired(1300).is_empty());
        drop(store);
        assert!(
            AgentWorkStore::open(&path)
                .unwrap()
                .task("a", "1")
                .unwrap()
                .stop_requested
        );
    }
    #[test]
    fn stopping_events_preserve_the_durable_stop_reason() {
        for completion in [
            serde_json::json!({"kind":"session.closed"}),
            serde_json::json!({"kind":"status.changed","status":"idle"}),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("work.json");
            let mut store = AgentWorkStore::open(&path).unwrap();
            let mut record = task("1", "a", "/project");
            record.began = true;
            store.admit(record).unwrap();
            store.bind("1", "claude#1").unwrap();
            store
                .stop("a", "1", "Task budget or progress deadline reached.")
                .unwrap();
            let stopped = store.task("a", "1").unwrap();
            store.observe("claude#1", &event(completion), 1500).unwrap();
            let observed = store.task("a", "1").unwrap();
            assert_eq!(observed.execution, AgentExecutionState::Stopped);
            assert_eq!(observed.detail, stopped.detail);
            assert_eq!(observed.next_action, stopped.next_action);
            drop(store);
            let reopened = AgentWorkStore::open(&path).unwrap();
            assert_eq!(reopened.task("a", "1").unwrap().detail, stopped.detail);
        }
    }
    #[test]
    fn agent_memory_proposals_cannot_self_authorize_and_history_preserves_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = AgentWorkStore::open(&temp.path().join("work.json")).unwrap();
        store
            .remember_memory("a", "before", "observed", 1000)
            .unwrap();
        store
            .remember_memory("a", "after", "owner save", 1100)
            .unwrap();
        store
            .propose_fact(
                "a",
                AgentMemoryFact {
                    id: "1".into(),
                    text: "I can publish".into(),
                    source: "Model claim".into(),
                    observed_at_ms: 0,
                    expires_at_ms: None,
                    supersedes: None,
                    state: AgentFactState::OwnerConfirmed,
                },
                1100,
            )
            .unwrap();
        let view = store.view("a");
        assert_eq!(view.memory_history[0].text, "before");
        assert_eq!(view.facts[0].state, AgentFactState::Proposed);
        store.confirm_fact("a", "1").unwrap();
        assert_eq!(
            store.view("a").facts[0].state,
            AgentFactState::OwnerConfirmed
        );
    }
}
