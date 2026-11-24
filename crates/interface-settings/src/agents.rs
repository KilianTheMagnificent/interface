//! One leased writer, atomic revisions, durable claims before provider effects.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::{
    agents::{
        AGENT_WORKSPACE_VERSION, AgentCommand, AgentControlResponse, AgentPastThread,
        AgentPendingApproval, AgentPermissionMode, AgentProfile, AgentRoutine, AgentRun,
        AgentRunStatus, AgentStatus, AgentThread, AgentThreadState, AgentThreadStatus,
        AgentWorkspace,
    },
    engine::{EngineEvent, EngineEventKind, SessionStatus},
};
use interface_storage::{JsonLimits, PrivateDirectory, ensure_current_revision, read_json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const BYTES: usize = 16 * 1024 * 1024;
const MAX_TIME: u64 = 8_640_000_000_000_000;
const MAX_RUNS: usize = 500;
const MAX_FOLDERS: usize = 16;
const MAX_PAST_THREADS: usize = 20;
const LEGACY_VERSION: u8 = 1;

/// The folders an agent-bound session may work in, resolved from the store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentBinding {
    pub agent_id: String,
    pub roots: Vec<PathBuf>,
    pub cwd: PathBuf,
    pub thread: bool,
    /// The agent's launch access now; `None` is the approval gate.
    pub access: Option<interface_contracts::engine::AgentAccess>,
}
fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.to_owned())
}
fn text(value: &str, max: usize, empty: bool) -> Result<(), SettingsError> {
    if (!empty && value.trim().is_empty()) || value.chars().count() > max || value.contains('\0') {
        return Err(invalid("A field is empty, too long, or contains NUL."));
    }
    Ok(())
}
fn absolute(path: &str) -> Result<(), SettingsError> {
    text(path, 4096, false)?;
    if !Path::new(path).is_absolute() {
        return Err(invalid("Use absolute folder paths."));
    }
    Ok(())
}
fn folders_valid(folders: &[String]) -> Result<(), SettingsError> {
    if folders.len() > MAX_FOLDERS {
        return Err(invalid("An agent can use at most 16 folders."));
    }
    let mut seen = BTreeSet::new();
    for folder in folders {
        absolute(folder)?;
        if !seen.insert(folder) {
            return Err(invalid("Each folder can be listed only once."));
        }
    }
    Ok(())
}
fn lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
fn profile_valid(p: &AgentProfile) -> Result<(), SettingsError> {
    text(&p.id, 64, false)?;
    text(&p.name, 60, false)?;
    text(&p.engine, 128, false)?;
    text(&p.model, 512, true)?;
    text(&p.effort, 128, true)?;
    text(&p.instructions, 8000, true)?;
    if let Some(home) = &p.home {
        absolute(home)?;
    }
    folders_valid(&p.folders)?;
    if p.instructions_file_sha256
        .as_deref()
        .is_some_and(|sha| !lowercase_sha256(sha))
    {
        return Err(invalid(
            "The instruction file hash must be 64 lowercase hex characters.",
        ));
    }
    Ok(())
}
fn threads_valid(s: &AgentWorkspace, ids: &BTreeSet<&String>) -> Result<(), SettingsError> {
    for (agent_id, threads) in &s.threads {
        if !ids.contains(agent_id)
            || threads.past.len() > MAX_PAST_THREADS
            || threads.last_read_at_ms > MAX_TIME
            || threads.last_activity_at_ms > MAX_TIME
            || threads
                .current
                .as_ref()
                .is_some_and(|c| c.started_at_ms > MAX_TIME)
            || threads
                .past
                .iter()
                .any(|p| p.started_at_ms > MAX_TIME || p.ended_at_ms > MAX_TIME)
        {
            return Err(invalid("Invalid agent thread record."));
        }
        for key in threads
            .current
            .iter()
            .map(|c| &c.session_key)
            .chain(threads.past.iter().map(|p| &p.session_key))
        {
            text(key, 256, false)?;
        }
    }
    Ok(())
}
fn routine_valid(r: &AgentRoutine) -> Result<(), SettingsError> {
    text(&r.id, 64, false)?;
    text(&r.name, 80, false)?;
    text(&r.prompt, 8000, false)?;
    text(&r.cwd, 4096, false)?;
    if !Path::new(&r.cwd).is_absolute()
        || r.start_at_ms == 0
        || r.start_at_ms > MAX_TIME
        || r.next_run_at_ms.is_some_and(|v| v > MAX_TIME)
        || r.interval_minutes
            .is_some_and(|v| !(5..=525_600).contains(&v))
    {
        return Err(invalid(
            "Use an absolute folder, a valid start time, and an interval of 5 minutes to one year.",
        ));
    }
    Ok(())
}
fn validate(s: &AgentWorkspace) -> Result<(), SettingsError> {
    if s.version != AGENT_WORKSPACE_VERSION
        || s.revision > 9_007_199_254_740_990
        || s.agents.len() > 50
        || s.assignments.len() > 500
        || s.routines.len() > 200
        || s.runs.len() > MAX_RUNS
    {
        return Err(invalid("Unsupported or over-budget agent workspace."));
    }
    let mut ids = BTreeSet::new();
    for p in &s.agents {
        profile_valid(p)?;
        if !ids.insert(&p.id) {
            return Err(invalid("Duplicate agent id."));
        }
    }
    for (key, id) in &s.assignments {
        text(key, 256, false)?;
        if !ids.contains(id) {
            return Err(invalid("Unknown assignment agent."));
        }
    }
    let mut routines = BTreeSet::new();
    for r in &s.routines {
        routine_valid(r)?;
        if !ids.contains(&r.agent_id) || !routines.insert(&r.id) {
            return Err(invalid("Unknown agent or duplicate routine."));
        }
    }
    let mut runs = BTreeSet::new();
    for run in &s.runs {
        text(&run.id, 160, false)?;
        profile_valid(&run.profile)?;
        text(&run.prompt, 8000, false)?;
        text(&run.cwd, 4096, false)?;
        text(&run.detail, 2000, true)?;
        if !runs.insert(&run.id)
            || run.agent_id != run.profile.id
            || !Path::new(&run.cwd).is_absolute()
            || run.scheduled_at_ms > MAX_TIME
            || run.updated_at_ms > MAX_TIME
        {
            return Err(invalid("Invalid run record."));
        }
    }
    threads_valid(s, &ids)
}

/// Run ids of one-off teammate tasks start with this; routine runs never do.
pub const AGENT_TASK_RUN_PREFIX: &str = "task:";
/// A task's run keeps at most this much of its title (a routine name's limit)...
const AGENT_TASK_NAME_CHARS: usize = 80;
/// ...and of its prompt (a routine prompt's limit).
const AGENT_TASK_PROMPT_CHARS: usize = 8000;

/// The owner's one-off task for a teammate, as [`AgentStore::admit_task`] takes it.
#[derive(Clone, Copy, Debug)]
pub struct AgentTaskRequest<'a> {
    pub request_id: &'a str,
    pub agent_id: &'a str,
    /// The absolute folder the task works in.
    pub cwd: &'a str,
    /// The owner's own words, which name the task.
    pub title: &'a str,
    /// The whole prompt, context included.
    pub prompt: &'a str,
    /// A model or effort chosen at launch; it applies to this task only.
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
}

/// What [`AgentStore::admit_task`] decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentTaskAdmission {
    /// A new `Starting` run the caller must launch now.
    Admitted(AgentRun),
    /// This request already admitted a run: launch nothing, report what it became.
    Existing(AgentRun),
}

pub struct AgentStore {
    pub work: crate::AgentWorkStore,
    starts: crate::agent_starts::AgentStarts,
    permissions: crate::agent_permissions::AgentPermissions,
    document: LeasedDocument,
    state: AgentWorkspace,
    pending_events: BTreeMap<String, (SessionStatus, bool)>,
    pending_overflow: bool,
}
impl AgentStore {
    /// Opens `file` (v2). When it is absent, `legacy` (v1) is read once, never modified, and
    /// migrated into `file`.
    /// # Errors
    /// Refuses unsafe, corrupt, foreign-version, or already leased storage. Never resets it.
    pub fn open(file: &Path, legacy: &Path) -> Result<Self, SettingsError> {
        let limits = JsonLimits::new(BYTES, 24, 500_000)?;
        let (document, loaded) = LeasedDocument::open::<AgentWorkspace>(
            file,
            ".agents.lock",
            limits,
            "Agent storage has an uncertain write; restart before changing it.",
        )?;
        let (state, migrated) = match loaded {
            Some(value) => (value, false),
            None => match read_legacy(legacy, limits)? {
                Some(mut value) => {
                    if value.version != LEGACY_VERSION {
                        return Err(invalid("Unsupported legacy agent workspace version."));
                    }
                    value.version = AGENT_WORKSPACE_VERSION;
                    (value, true)
                }
                None => (AgentWorkspace::default(), false),
            },
        };
        validate(&state)?;
        let mut store = Self {
            work: crate::AgentWorkStore::open(&file.with_file_name("agent-work.v1.json"))?,
            starts: crate::agent_starts::AgentStarts::open(
                &file.with_file_name("agent-starts.v1.json"),
            )?,
            permissions: crate::agent_permissions::AgentPermissions::open(
                &file.with_file_name("agent-permissions.v1.json"),
            )?,
            document,
            state,
            pending_events: BTreeMap::new(),
            pending_overflow: false,
        };
        let recovered = recovered(&store.state);
        if migrated || recovered != store.state {
            store.commit(recovered)?;
        }
        Ok(store)
    }
    #[must_use]
    pub fn snapshot(&self) -> AgentWorkspace {
        self.state.clone()
    }
    /// The workspace, borrowed, for readers that need a few fields rather
    /// than the owned copy [`Self::snapshot`] makes.
    #[must_use]
    pub fn workspace(&self) -> &AgentWorkspace {
        &self.state
    }
    /// One roster profile, borrowed.
    #[must_use]
    pub fn profile(&self, agent_id: &str) -> Option<&AgentProfile> {
        self.state
            .agents
            .iter()
            .find(|profile| profile.id == agent_id)
    }
    /// Flushes an intent before a first-message provider effect.
    /// # Errors
    /// Refuses changed input, unsafe or uncertain storage and exhausted capacity.
    pub fn reserve_start(
        &mut self,
        request: &str,
        agent: &str,
        text: &str,
    ) -> Result<crate::AgentStartAdmission, SettingsError> {
        self.starts.reserve(request, agent, text)
    }
    /// Records the outcome without ever making an ambiguous effect replayable.
    /// # Errors
    /// Refuses unsafe storage or an already settled intent.
    pub fn settle_start(
        &mut self,
        request: &str,
        key: Option<String>,
        unsent: bool,
    ) -> Result<(), SettingsError> {
        self.starts.settle(request, key, unsent)
    }
    /// The committed workspace revision; it moves on every durable change.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.state.revision
    }
    /// The earliest time `claim` could admit or start a run: now while a run
    /// is queued, else the earliest enabled routine's due time. A due routine
    /// whose agent is busy stays due, so callers must bound their retry rate.
    #[must_use]
    pub fn next_claim_at(&self) -> Option<u64> {
        if self
            .state
            .runs
            .iter()
            .any(|run| run.status == AgentRunStatus::Queued)
        {
            return Some(0);
        }
        self.state
            .routines
            .iter()
            .filter(|routine| routine.enabled)
            .filter_map(|routine| routine.next_run_at_ms)
            .min()
    }
    fn commit(&mut self, mut next: AgentWorkspace) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        next.revision = self
            .state
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("Revision exhausted."))?;
        validate(&next)?;
        self.document.commit(&next)?;
        self.state = next;
        Ok(())
    }
    /// Applies one store command and returns the resulting state, borrowed.
    /// Reads and no-op commands neither clone nor write.
    /// # Errors
    /// Refuses stale revisions and invalid commands before any disk or provider effect.
    #[allow(clippy::too_many_lines)]
    pub fn command(
        &mut self,
        command: AgentCommand,
        now: u64,
    ) -> Result<&AgentWorkspace, SettingsError> {
        use AgentCommand as C;
        if self.document.is_uncertain() {
            return Err(invalid("Agent storage needs restart after a failed write."));
        }
        if matches!(command, C::Read {}) {
            return Ok(&self.state);
        }
        let mut cleanup_agent = None;
        let mut cleanup_routine = None;
        let mut next = self.state.clone();
        match command {
            C::Read {} => return Ok(&self.state),
            C::ImportLegacy {
                agents,
                assignments,
            } => {
                // Revision zero is the only import authority, even if the user later deletes every agent.
                if next.revision != 0 {
                    return Ok(&self.state);
                }
                // Home, folders and the instruction hash are Rust-owned grants; a renderer
                // import never supplies them.
                next.agents = agents
                    .into_iter()
                    .map(|mut profile| {
                        profile.home = None;
                        profile.folders = Vec::new();
                        profile.instructions_file_sha256 = None;
                        profile
                    })
                    .collect();
                next.assignments = assignments;
            }
            C::SaveAgent {
                expected_revision,
                profile,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                save_agent(&mut next, profile, now)?;
            }
            C::RemoveAgent {
                expected_revision,
                id,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                if next
                    .runs
                    .iter()
                    .any(|r| r.agent_id == id && r.status.blocks_agent())
                {
                    return Err(invalid(
                        "Finish or acknowledge this agent's run before deleting it.",
                    ));
                }
                if self
                    .work
                    .view(&id)
                    .tasks
                    .iter()
                    .any(|t| t.execution.holds_resources())
                {
                    return Err(invalid(
                        "Inspect and acknowledge unfinished tasks before removing this agent.",
                    ));
                }
                let routines = next
                    .routines
                    .iter()
                    .filter(|r| r.agent_id == id)
                    .map(|r| r.id.clone())
                    .collect::<Vec<_>>();
                cleanup_agent = Some((id.clone(), routines));
                next.agents.retain(|p| p.id != id);
                next.assignments.retain(|_, v| *v != id);
                next.routines.retain(|r| r.agent_id != id);
                next.threads.remove(&id);
            }
            C::Assign {
                expected_revision,
                session_key,
                agent_id,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                make_assignment_room(&mut next, &session_key)?;
                next.assignments.insert(session_key, agent_id);
            }
            C::SaveRoutine {
                expected_revision,
                routine,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                if routine.enabled
                    && self.work.calendar(&routine.id).is_some()
                    && !self.routine_tested(&routine)
                {
                    return Err(invalid(
                        "Test this calendar routine with its current task contract before enabling it. A test run performs real work.",
                    ));
                }
                save_routine(&mut next, routine)?;
            }
            C::RemoveRoutine {
                expected_revision,
                id,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                cleanup_routine = Some(id.clone());
                next.routines.retain(|r| r.id != id);
                next.runs
                    .retain(|r| r.routine_id != id || r.status != AgentRunStatus::Queued);
            }
            C::RunNow {
                expected_revision,
                routine_id,
                request_id,
            } => {
                if !run_now(&mut next, expected_revision, &routine_id, &request_id, now)? {
                    return Ok(&self.state);
                }
            }
            C::AcknowledgeRun {
                expected_revision,
                id,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                let r = next
                    .runs
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or_else(|| invalid("Run no longer exists."))?;
                if r.status != AgentRunStatus::Unknown {
                    return Err(invalid("Only an uncertain run needs acknowledgment."));
                }
                r.status = AgentRunStatus::Failed;
                r.updated_at_ms = now;
                r.detail = "Uncertain result acknowledged by user. No retry requested.".into();
            }
            C::NewThread {
                expected_revision,
                agent_id,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                profile_mut(&mut next, &agent_id)?;
                end_thread(&mut next, &agent_id, now);
            }
            C::MarkRead { agent_id } => {
                profile_mut(&mut next, &agent_id)?;
                if !mark_read(&mut next, &agent_id, now) {
                    return Ok(&self.state);
                }
            }
            C::SaveAgentFolders {
                expected_revision,
                agent_id,
                folders,
            } => {
                ensure_current_revision(expected_revision, next.revision)?;
                folders_valid(&folders)?;
                profile_mut(&mut next, &agent_id)?.folders = folders;
            }
            // Filesystem, decision-log and provider effects are not store commands.
            C::Calendar { .. }
            | C::Work { .. }
            | C::StartAgentThread { .. }
            | C::StartAgentTask { .. }
            | C::ReadAgentFiles { .. }
            | C::SaveAgentMemory { .. }
            | C::OverwriteAgentInstructionsFile { .. }
            | C::ReadAgentDecisions { .. }
            | C::RecreateAgentHome { .. }
            | C::RevealAgentHome { .. }
            | C::SavePermissionMode { .. } => {
                return Err(invalid("Handled by the desktop service."));
            }
        }
        self.commit(next)?;
        if let Some((agent, routines)) = cleanup_agent {
            // Work cleanup first, as before permission modes existed; a failed
            // sidecar write must not skip it.
            self.work.forget_agent(&agent, &routines)?;
            self.permissions.forget(&agent)?;
        }
        if let Some(routine) = cleanup_routine {
            self.work.save_calendar(&routine, None)?;
        }
        Ok(&self.state)
    }
    /// Calendar edits pause before changing the sidecar, so a crash cannot enable a
    /// partially applied schedule. Existing interval-only routines keep their behavior.
    /// # Errors
    /// Refuses stale revisions, invalid timezones and missing routines.
    pub fn configure_calendar(
        &mut self,
        agent: &str,
        id: &str,
        revision: u64,
        schedule: Option<interface_contracts::agent_work::AgentCalendarSchedule>,
        now: u64,
    ) -> Result<(), SettingsError> {
        ensure_current_revision(revision, self.state.revision)?;
        let next_due = schedule
            .as_ref()
            .map(|s| crate::calendar_occurrences(s, now, 1))
            .transpose()?
            .and_then(|v| v.first().copied());
        let mut next = self.state.clone();
        let routine = next
            .routines
            .iter_mut()
            .find(|r| r.id == id && r.agent_id == agent)
            .ok_or_else(|| invalid("Unknown routine."))?;
        routine.enabled = false;
        routine.next_run_at_ms = next_due.or(Some(now.saturating_add(60_000)));
        self.commit(next)?;
        self.work.save_calendar(id, schedule)
    }
    /// # Errors
    /// A failed deterministic output check pauses calendars configured to stop on failure.
    pub fn pause_failed_calendar(&mut self, task: &str) -> Result<(), SettingsError> {
        let Some(run) = self.state.runs.iter().find(|run| run.id == task) else {
            return Ok(());
        };
        if !self
            .work
            .calendar(&run.routine_id)
            .is_some_and(|s| s.pause_on_failure)
        {
            return Ok(());
        }
        let mut next = self.state.clone();
        let Some(routine) = next
            .routines
            .iter_mut()
            .find(|r| r.id == run.routine_id && r.enabled)
        else {
            return Ok(());
        };
        routine.enabled = false;
        self.commit(next)
    }
    #[must_use]
    pub fn routine_tested(&self, routine: &AgentRoutine) -> bool {
        let spec = self.work.spec(&routine.agent_id);
        self.work.view(&routine.agent_id).tasks.iter().any(|task| {
            task.execution == interface_contracts::agent_work::AgentExecutionState::Completed
                && task.goal == routine.prompt
                && (task.cwd == routine.cwd
                    || std::fs::canonicalize(&routine.cwd)
                        .is_ok_and(|path| path == Path::new(&task.cwd)))
                && task.spec == spec
                && (spec.artifacts.is_empty()
                    || task.verification
                        == interface_contracts::agent_work::AgentVerificationState::Verified)
                && self.state.runs.iter().any(|run| {
                    run.id == task.id
                        && run.status == AgentRunStatus::Succeeded
                        && run.routine_id == routine.id
                        && self
                            .state
                            .agents
                            .iter()
                            .any(|profile| profile == &run.profile)
                })
        })
    }
    /// Coalesce missed fixed-interval occurrences to one current run; never overlap an agent.
    /// # Errors
    /// Returns validation or durable claim errors. No caller may dispatch on error.
    pub fn claim(&mut self, now: u64) -> Result<Option<AgentRun>, SettingsError> {
        if self.document.is_uncertain() {
            return Err(invalid("Agent storage needs restart."));
        }
        if !self
            .state
            .runs
            .iter()
            .any(|r| r.status == AgentRunStatus::Starting)
        {
            self.pending_events.clear();
            self.pending_overflow = false;
        }
        // The idle tick: nothing due and nothing queued leaves state untouched.
        let due = self
            .state
            .routines
            .iter()
            .any(|r| r.enabled && r.next_run_at_ms.is_some_and(|due| due <= now));
        if !due
            && !self
                .state
                .runs
                .iter()
                .any(|r| r.status == AgentRunStatus::Queued)
        {
            return Ok(None);
        }
        let mut next = self.state.clone();
        self.admit_scheduled(&mut next, now)?;
        // Keep the number of provider attachments bounded, including approval waits.
        let active = next
            .runs
            .iter()
            .filter(|r| {
                matches!(
                    r.status,
                    AgentRunStatus::Starting | AgentRunStatus::Running | AgentRunStatus::Waiting
                )
            })
            .count();
        let claim = if active < 4 {
            next.runs
                .iter_mut()
                .find(|r| {
                    r.status == AgentRunStatus::Queued
                        && !self
                            .work
                            .view(&r.agent_id)
                            .tasks
                            .iter()
                            .any(|task| task.execution.holds_resources())
                })
                .map(|r| {
                    r.status = AgentRunStatus::Starting;
                    r.updated_at_ms = now;
                    r.clone()
                })
        } else {
            None
        };
        if next != self.state {
            self.commit(next)?;
        }
        Ok(claim)
    }
    fn admit_scheduled(&self, next: &mut AgentWorkspace, now: u64) -> Result<(), SettingsError> {
        for r in next.routines.clone() {
            let Some(due) = r.next_run_at_ms.filter(|due| r.enabled && *due <= now) else {
                continue;
            };
            if next
                .runs
                .iter()
                .any(|run| run.agent_id == r.agent_id && run.status.blocks_agent())
            {
                continue;
            }
            if self
                .work
                .view(&r.agent_id)
                .tasks
                .iter()
                .any(|task| task.execution.holds_resources())
            {
                continue;
            }
            let calendar = self.work.calendar(&r.id);
            if calendar.is_some() && !self.routine_tested(&r) {
                if let Some(routine) = next.routines.iter_mut().find(|row| row.id == r.id) {
                    routine.enabled = false;
                }
                continue;
            }
            if let Some(calendar) = &calendar
                && calendar.missed == interface_contracts::agent_work::AgentMissedRunPolicy::Skip
                && now.saturating_sub(due) > 60_000
            {
                let upcoming = crate::calendar_occurrences(calendar, now, 1)?
                    .first()
                    .copied();
                if let Some(routine) = next.routines.iter_mut().find(|row| row.id == r.id) {
                    routine.next_run_at_ms = upcoming;
                }
                continue;
            }
            let interval = r.interval_minutes.map(|v| u64::from(v) * 60_000);
            let occurrence = if calendar.is_some() {
                due
            } else {
                interval.map_or(due, |step| due + ((now - due) / step) * step)
            };
            admit(
                next,
                &r,
                format!("scheduled:{}:{occurrence}", r.id),
                occurrence,
                now,
            )?;
            if let Some(r) = next.routines.iter_mut().find(|v| v.id == r.id) {
                r.next_run_at_ms = match &calendar {
                    Some(calendar) => crate::calendar_occurrences(calendar, now, 1)?
                        .first()
                        .copied(),
                    None => interval
                        .and_then(|v| occurrence.checked_add(v))
                        .filter(|v| *v <= MAX_TIME),
                };
            }
        }
        Ok(())
    }
    /// Admits the owner's one-off task for a teammate (protocol 38) as a run with no routine,
    /// so it takes a routine run's gated launch, binding, approval handling and recovery. The
    /// run is `Starting` at once: the host launches it in the same request, never the
    /// scheduler. Its id is `task:<request id>`, so one request never admits twice; the same
    /// request again returns the run it admitted. Unlike a routine, a busy agent does not
    /// refuse it here: the work ledger refuses overlapping work before any provider launch.
    /// # Errors
    /// Refuses invalid input, an unknown agent, a request id another agent's task holds, a full
    /// run history, and persistence failures.
    pub fn admit_task(
        &mut self,
        request: &AgentTaskRequest<'_>,
        now: u64,
    ) -> Result<AgentTaskAdmission, SettingsError> {
        text(request.request_id, 64, false)?;
        absolute(request.cwd)?;
        text(request.prompt, usize::MAX, false)?;
        let id = format!("{AGENT_TASK_RUN_PREFIX}{}", request.request_id);
        if let Some(existing) = self.state.runs.iter().find(|run| run.id == id) {
            if existing.agent_id != request.agent_id {
                return Err(invalid(
                    "This request id already belongs to another agent's task.",
                ));
            }
            return Ok(AgentTaskAdmission::Existing(existing.clone()));
        }
        let mut profile = self
            .profile(request.agent_id)
            .cloned()
            .ok_or_else(|| invalid("Agent no longer exists."))?;
        // The task composer opens on the agent's model and effort; what the owner picks there
        // applies to this task only, as it did before tasks were gated.
        if let Some(model) = request.model.filter(|model| !model.trim().is_empty()) {
            model.trim().clone_into(&mut profile.model);
        }
        if let Some(effort) = request.effort.filter(|effort| !effort.trim().is_empty()) {
            effort.trim().clone_into(&mut profile.effort);
        }
        profile_valid(&profile)?;
        let title = request.title.trim();
        let name: String = if title.is_empty() { "Task" } else { title }
            .chars()
            .take(AGENT_TASK_NAME_CHARS)
            .collect();
        let mut next = self.state.clone();
        if next.runs.len() >= MAX_RUNS {
            let i = next
                .runs
                .iter()
                .position(|run| !run.status.blocks_agent())
                .ok_or_else(|| invalid("Run history is full."))?;
            next.runs.remove(i);
        }
        let run = AgentRun {
            id,
            routine_id: String::new(),
            agent_id: request.agent_id.to_owned(),
            name,
            scheduled_at_ms: now,
            updated_at_ms: now,
            status: AgentRunStatus::Starting,
            session_key: None,
            detail: String::new(),
            profile,
            // A bounded record of what was asked; the launch sends the whole prompt and an
            // admitted run is never replayed from this copy.
            prompt: request
                .prompt
                .chars()
                .take(AGENT_TASK_PROMPT_CHARS)
                .collect(),
            cwd: request.cwd.to_owned(),
            began: false,
        };
        next.runs.push(run.clone());
        self.commit(next)?;
        Ok(AgentTaskAdmission::Admitted(run))
    }
    /// # Errors
    /// Record the provider identity after its initial prompt has been admitted.
    pub fn bind(&mut self, id: &str, key: &str, now: u64) -> Result<(), SettingsError> {
        text(key, 256, false)?;
        let mut next = self.state.clone();
        let run = next
            .runs
            .iter_mut()
            .find(|r| r.id == id && r.status == AgentRunStatus::Starting)
            .ok_or_else(|| invalid("Run claim is no longer starting."))?;
        run.session_key = Some(key.into());
        run.status = AgentRunStatus::Running;
        run.began = self
            .pending_events
            .get(key)
            .is_some_and(|(_, began)| *began);
        run.updated_at_ms = now;
        let agent_id = run.agent_id.clone();
        make_assignment_room(&mut next, key)?;
        next.assignments.insert(key.into(), agent_id);
        self.commit(next)?;
        // Fast providers can finish before createSession returns its key. Replay only the final
        // observed state for that exact key, never the provider command.
        if self.pending_overflow {
            self.fail(id,"Session observation overflowed during launch. Inspect the task before acknowledging.",false,now)?;
        } else if let Some((status, _)) = self.pending_events.remove(key) {
            self.observe(
                key,
                &EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged { status },
                },
                now,
            )?;
        }
        Ok(())
    }
    /// # Errors
    /// Refuses persistence failures; an ambiguous effect is never replayed.
    pub fn fail(
        &mut self,
        id: &str,
        detail: &str,
        definitely_unsent: bool,
        now: u64,
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let Some(run) = next
            .runs
            .iter_mut()
            .find(|r| r.id == id && r.status.blocks_agent())
        else {
            return Ok(());
        };
        run.status = if definitely_unsent {
            AgentRunStatus::Failed
        } else {
            AgentRunStatus::Unknown
        };
        run.detail = detail.chars().take(2000).collect();
        run.updated_at_ms = now;
        let routine_id = run.routine_id.clone();
        if self
            .work
            .calendar(&routine_id)
            .is_some_and(|s| s.pause_on_failure)
            && let Some(routine) = next.routines.iter_mut().find(|r| r.id == routine_id)
        {
            routine.enabled = false;
        }
        self.commit(next)
    }
    /// Whether [`Self::observe`] can act on this event at all. Callers check
    /// it before taking the store lock so text deltas never contend with a
    /// durable agent write.
    #[must_use]
    pub fn observes(event: &EngineEvent) -> bool {
        observed_status(event).is_some()
    }
    /// # Errors
    /// Fails closed if the event cannot be durably projected.
    #[allow(clippy::too_many_lines)]
    pub fn observe(
        &mut self,
        key: &str,
        event: &EngineEvent,
        now: u64,
    ) -> Result<(), SettingsError> {
        if observed_status(event).is_none() {
            return Ok(());
        }
        // A thread key is never a run key; its events must not fill the launch projection.
        if let Some(agent_id) = self.thread_agent(key) {
            return self.observe_thread(&agent_id, event, now);
        }
        if !self
            .state
            .runs
            .iter()
            .any(|r| r.session_key.as_deref() == Some(key))
        {
            if self
                .state
                .runs
                .iter()
                .any(|r| r.status == AgentRunStatus::Starting)
            {
                if self.pending_events.len() >= 64 && !self.pending_events.contains_key(key) {
                    // Bounded projection overflow cannot become an invented successful run.
                    self.pending_overflow = true;
                    return Err(invalid(
                        "Too many concurrent session observations while an agent was starting.",
                    ));
                }
                let Some(status) = observed_status(event) else {
                    return Ok(());
                };
                let began = status == SessionStatus::Working
                    || self
                        .pending_events
                        .get(key)
                        .is_some_and(|(_, began)| *began);
                self.pending_events.insert(key.into(), (status, began));
            } else {
                self.pending_events.clear();
                self.pending_overflow = false;
            }
            return Ok(());
        }
        let mut next = self.state.clone();
        let Some(run) = next.runs.iter_mut().find(|r| {
            r.session_key.as_deref() == Some(key)
                && matches!(r.status, AgentRunStatus::Running | AgentRunStatus::Waiting)
        }) else {
            return Ok(());
        };
        match &event.event {
            EngineEventKind::StatusChanged {
                status: SessionStatus::Working,
            }
            | EngineEventKind::UserMessage { .. } => {
                run.began = true;
                run.status = AgentRunStatus::Running;
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } if run.began => {
                run.status = AgentRunStatus::Succeeded;
                run.detail = "Provider finished the turn. Review the task for results.".into();
            }
            EngineEventKind::ApprovalRequested { .. }
            | EngineEventKind::StatusChanged {
                status: SessionStatus::Waiting,
            } => {
                run.status = AgentRunStatus::Waiting;
                run.detail = "Open the task to respond to the provider's approval request.".into();
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Failed,
            }
            | EngineEventKind::Error {
                retryable: false, ..
            }
            | EngineEventKind::SessionClosed => {
                run.status = AgentRunStatus::Unknown;
                run.detail="Provider stopped without a confirmed completed turn. Inspect the task before acknowledging.".into();
            }
            _ => return Ok(()),
        }
        let routine_id = run.routine_id.clone();
        let failed = matches!(run.status, AgentRunStatus::Failed | AgentRunStatus::Unknown);
        if failed
            && self
                .work
                .calendar(&routine_id)
                .is_some_and(|s| s.pause_on_failure)
            && let Some(routine) = next.routines.iter_mut().find(|r| r.id == routine_id)
        {
            routine.enabled = false;
        }
        if next == self.state {
            return Ok(());
        }
        if let Some(run) = next
            .runs
            .iter_mut()
            .find(|r| r.session_key.as_deref() == Some(key))
        {
            run.updated_at_ms = now;
        }
        self.commit(next)
    }
}
impl AgentStore {
    /// Record the provider session that is now this agent's one thread.
    /// # Errors
    /// Refuses an unknown agent, a session owned by another agent, or a failed write.
    pub fn bind_thread(
        &mut self,
        agent_id: &str,
        session_key: &str,
        now: u64,
    ) -> Result<(), SettingsError> {
        text(session_key, 256, false)?;
        let mut next = self.state.clone();
        profile_mut(&mut next, agent_id)?;
        if self
            .thread_agent(session_key)
            .is_some_and(|owner| owner != agent_id)
        {
            return Err(invalid("That session already belongs to another agent."));
        }
        let previous = next.threads.get(agent_id).and_then(|t| t.current.clone());
        let started_at_ms = match previous {
            Some(current) if current.session_key == session_key => current.started_at_ms,
            Some(_) => {
                // Never lose a thread reference: a replaced thread stays reachable as past.
                end_thread(&mut next, agent_id, now);
                now
            }
            None => now,
        };
        let threads = next.threads.entry(agent_id.to_owned()).or_default();
        threads.current = Some(AgentThread {
            session_key: session_key.to_owned(),
            started_at_ms,
            status: AgentThreadStatus::Working,
            interrupted: false,
        });
        threads.last_activity_at_ms = now;
        make_assignment_room(&mut next, session_key)?;
        next.assignments
            .insert(session_key.to_owned(), agent_id.to_owned());
        self.commit(next)
    }
    /// # Errors
    /// Refuses an unknown agent, an invalid path or hash, or a failed write.
    pub fn set_home(
        &mut self,
        agent_id: &str,
        home: &str,
        instructions_sha: &str,
    ) -> Result<(), SettingsError> {
        self.update_profile(agent_id, |p| {
            p.home = Some(home.to_owned());
            p.instructions_file_sha256 = Some(instructions_sha.to_owned());
        })
    }
    /// # Errors
    /// Refuses an unknown agent, an invalid hash, or a failed write.
    pub fn set_instructions_sha(&mut self, agent_id: &str, sha: &str) -> Result<(), SettingsError> {
        self.update_profile(agent_id, |p| {
            p.instructions_file_sha256 = Some(sha.to_owned());
        })
    }
    fn update_profile(
        &mut self,
        agent_id: &str,
        change: impl FnOnce(&mut AgentProfile),
    ) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        change(profile_mut(&mut next, agent_id)?);
        if next == self.state {
            return Ok(());
        }
        self.commit(next)
    }
    /// One status per agent: needsAttention > working > unread > idle.
    #[must_use]
    pub fn statuses(&self, pending: &[AgentPendingApproval]) -> BTreeMap<String, AgentStatus> {
        self.state
            .agents
            .iter()
            .map(|profile| {
                let threads = self.state.threads.get(&profile.id);
                let current = threads.and_then(|t| t.current.as_ref());
                let mut runs = self.state.runs.iter().filter(|r| r.agent_id == profile.id);
                let status = if pending.iter().any(|p| p.agent_id == profile.id)
                    || current.is_some_and(|c| c.interrupted)
                    || runs.clone().any(|r| {
                        matches!(r.status, AgentRunStatus::Waiting | AgentRunStatus::Unknown)
                    }) {
                    AgentStatus::NeedsAttention
                } else if current.is_some_and(|c| c.status != AgentThreadStatus::Idle)
                    || runs.any(|r| {
                        matches!(r.status, AgentRunStatus::Running | AgentRunStatus::Starting)
                    })
                {
                    AgentStatus::Working
                } else if threads.is_some_and(|t| t.last_activity_at_ms > t.last_read_at_ms) {
                    AgentStatus::Unread
                } else {
                    AgentStatus::Idle
                };
                (profile.id.clone(), status)
            })
            .collect()
    }
    #[must_use]
    pub fn response(&self, pending: &[AgentPendingApproval]) -> AgentControlResponse {
        AgentControlResponse {
            workspace: self.snapshot(),
            work: None,
            calendar: None,
            statuses: self.statuses(pending),
            pending_approvals: pending.to_vec(),
            files: None,
            decisions: None,
            session_key: None,
            permission_modes: self.permissions.modes(&self.agent_ids()),
        }
    }
    fn agent_ids(&self) -> Vec<String> {
        self.state.agents.iter().map(|p| p.id.clone()).collect()
    }
    /// The agent's permission mode; an unknown agent is gated.
    #[must_use]
    pub fn permission_mode(&self, agent_id: &str) -> AgentPermissionMode {
        self.permissions.mode(agent_id)
    }
    /// Sets an existing agent's permission mode. It applies from the next launch or resume.
    /// # Errors
    /// Refuses an unknown agent and failed or uncertain storage.
    pub fn save_permission_mode(
        &mut self,
        agent_id: &str,
        mode: AgentPermissionMode,
    ) -> Result<(), SettingsError> {
        let known = self.agent_ids();
        self.permissions.save(agent_id, mode, &known)
    }
    /// Scope for an agent-bound session: its thread (home and folders) or a routine run
    /// (home, folders and the run's folder). The current profile wins over a run snapshot so
    /// removing a folder narrows running work too. A run folder that fails `is_grantable`
    /// (the desktop service's grant rule) is left out, so the policy asks instead.
    #[must_use]
    pub fn binding(
        &self,
        session_key: &str,
        is_grantable: impl Fn(&Path) -> bool,
    ) -> Option<AgentBinding> {
        if let Some(agent_id) = self.thread_agent(session_key) {
            let profile = self.state.agents.iter().find(|p| p.id == agent_id)?;
            let cwd = PathBuf::from(profile.home.as_ref()?);
            return Some(AgentBinding {
                roots: scope_roots(profile),
                access: self.permissions.mode(&agent_id).access(),
                agent_id,
                cwd,
                thread: true,
            });
        }
        let run = self
            .state
            .runs
            .iter()
            .rev()
            .find(|r| r.session_key.as_deref() == Some(session_key))?;
        let profile = self
            .state
            .agents
            .iter()
            .find(|p| p.id == run.agent_id)
            .unwrap_or(&run.profile);
        let cwd = PathBuf::from(&run.cwd);
        let mut roots = scope_roots(profile);
        if !roots.contains(&cwd) && is_grantable(&cwd) {
            roots.push(cwd.clone());
        }
        Some(AgentBinding {
            agent_id: run.agent_id.clone(),
            roots,
            cwd,
            thread: false,
            access: self.permissions.mode(&run.agent_id).access(),
        })
    }
    fn thread_agent(&self, session_key: &str) -> Option<String> {
        self.state
            .threads
            .iter()
            .find(|(_, t)| {
                t.current
                    .as_ref()
                    .is_some_and(|c| c.session_key == session_key)
            })
            .map(|(id, _)| id.clone())
    }
    /// Whether `session_key` is only a past thread of some agent. Past threads are read-only.
    #[must_use]
    pub fn is_past_thread(&self, session_key: &str) -> bool {
        self.thread_agent(session_key).is_none()
            && self
                .state
                .threads
                .values()
                .any(|t| t.past.iter().any(|p| p.session_key == session_key))
    }
    /// Whether `session_key` is an agent's current thread or the session of a routine run that
    /// is still starting, running or waiting. Every other agent session is read-only.
    #[must_use]
    pub fn is_live_agent_session(&self, session_key: &str) -> bool {
        self.thread_agent(session_key).is_some()
            || self.state.runs.iter().any(|r| {
                r.session_key.as_deref() == Some(session_key)
                    && matches!(
                        r.status,
                        AgentRunStatus::Starting
                            | AgentRunStatus::Running
                            | AgentRunStatus::Waiting
                    )
            })
    }
    fn observe_thread(
        &mut self,
        agent_id: &str,
        event: &EngineEvent,
        now: u64,
    ) -> Result<(), SettingsError> {
        use AgentThreadStatus as T;
        let mut next = self.state.clone();
        let Some(AgentThreadState {
            current: Some(current),
            last_activity_at_ms,
            ..
        }) = next.threads.get_mut(agent_id)
        else {
            return Ok(());
        };
        match &event.event {
            EngineEventKind::StatusChanged {
                status: SessionStatus::Working,
            }
            | EngineEventKind::UserMessage { .. } => current.status = T::Working,
            EngineEventKind::ApprovalRequested { .. }
            | EngineEventKind::StatusChanged {
                status: SessionStatus::Waiting,
            } => {
                current.status = T::Waiting;
                *last_activity_at_ms = now;
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } => {
                if current.status != T::Idle {
                    current.status = T::Idle;
                    *last_activity_at_ms = now;
                }
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Failed,
            }
            | EngineEventKind::Error {
                retryable: false, ..
            }
            | EngineEventKind::SessionClosed => {
                // Only a turn that was running ends here; closing an idle pane or stopping
                // the host is not new activity.
                if current.status != T::Idle {
                    current.status = T::Idle;
                    *last_activity_at_ms = now;
                }
            }
            _ => return Ok(()),
        }
        if next == self.state {
            return Ok(());
        }
        self.commit(next)
    }
}

fn read_legacy(file: &Path, limits: JsonLimits) -> Result<Option<AgentWorkspace>, SettingsError> {
    match std::fs::symlink_metadata(file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }
    let (Some(parent), Some(name)) = (file.parent(), file.file_name().and_then(|n| n.to_str()))
    else {
        return Err(SettingsError::InvalidPath);
    };
    // Read-only: the legacy file and its directory are never hardened, rewritten or moved.
    let directory = PrivateDirectory::open(parent)?;
    Ok(read_json::<AgentWorkspace>(&directory, name, limits)?.map(|v| v.into_parts().0))
}

fn recovered(state: &AgentWorkspace) -> AgentWorkspace {
    let mut next = state.clone();
    for run in &mut next.runs {
        if matches!(
            run.status,
            AgentRunStatus::Starting | AgentRunStatus::Running | AgentRunStatus::Waiting
        ) {
            run.status = AgentRunStatus::Unknown;
            run.detail="Host stopped during execution. Inspect the task before acknowledging; it will not be retried automatically.".into();
        }
    }
    for current in next.threads.values_mut().filter_map(|t| t.current.as_mut()) {
        if current.status != AgentThreadStatus::Idle {
            current.status = AgentThreadStatus::Idle;
            current.interrupted = true;
        }
    }
    next
}

fn profile_mut<'a>(
    state: &'a mut AgentWorkspace,
    agent_id: &str,
) -> Result<&'a mut AgentProfile, SettingsError> {
    state
        .agents
        .iter_mut()
        .find(|p| p.id == agent_id)
        .ok_or_else(|| invalid("Agent no longer exists."))
}

/// Inserts or replaces one routine. An edit keeps the pending occurrence
/// unless its schedule changed; pausing drops its queued scheduled runs.
fn save_routine(next: &mut AgentWorkspace, mut routine: AgentRoutine) -> Result<(), SettingsError> {
    routine_valid(&routine)?;
    let Some(r) = next.routines.iter_mut().find(|r| r.id == routine.id) else {
        routine.next_run_at_ms = Some(routine.start_at_ms);
        next.routines.push(routine);
        return Ok(());
    };
    if r.agent_id != routine.agent_id {
        return Err(invalid("A routine cannot change its owning agent."));
    }
    routine.next_run_at_ms =
        if r.start_at_ms == routine.start_at_ms && r.interval_minutes == routine.interval_minutes {
            r.next_run_at_ms
        } else {
            Some(routine.start_at_ms)
        };
    let paused = !routine.enabled;
    let routine_id = routine.id.clone();
    *r = routine;
    if paused {
        next.runs.retain(|run| {
            run.routine_id != routine_id
                || run.status != AgentRunStatus::Queued
                || !run.id.starts_with("scheduled:")
        });
    }
    Ok(())
}

/// Queues one manual run. Returns `false` when this request id already
/// queued the same routine: idempotency is checked before the revision, so a
/// retried request is a no-op rather than a stale-revision error.
fn run_now(
    next: &mut AgentWorkspace,
    expected_revision: u64,
    routine_id: &str,
    request_id: &str,
    now: u64,
) -> Result<bool, SettingsError> {
    text(request_id, 64, false)?;
    let id = format!("manual:{request_id}");
    if let Some(existing) = next.runs.iter().find(|r| r.id == id) {
        if existing.routine_id != routine_id {
            return Err(invalid(
                "This request id already belongs to another routine.",
            ));
        }
        return Ok(false);
    }
    ensure_current_revision(expected_revision, next.revision)?;
    let r = next
        .routines
        .iter()
        .find(|r| r.id == routine_id)
        .cloned()
        .ok_or_else(|| invalid("Routine no longer exists."))?;
    admit(next, &r, id, now, now)?;
    Ok(true)
}

fn save_agent(
    state: &mut AgentWorkspace,
    mut profile: AgentProfile,
    now: u64,
) -> Result<(), SettingsError> {
    // Home, folders and the instruction hash are Rust-owned; renderer values are ignored.
    let stored = state.agents.iter().find(|p| p.id == profile.id);
    profile.home = stored.and_then(|p| p.home.clone());
    profile.folders = stored.map(|p| p.folders.clone()).unwrap_or_default();
    profile.instructions_file_sha256 = stored.and_then(|p| p.instructions_file_sha256.clone());
    let engine_changed = stored.is_some_and(|p| p.engine != profile.engine);
    profile_valid(&profile)?;
    unique_appearance(state, &mut profile)?;
    if engine_changed {
        end_thread(state, &profile.id, now);
    }
    if let Some(p) = state.agents.iter_mut().find(|p| p.id == profile.id) {
        *p = profile;
    } else {
        state.agents.push(profile);
    }
    Ok(())
}

fn end_thread(state: &mut AgentWorkspace, agent_id: &str, now: u64) {
    let Some(threads) = state.threads.get_mut(agent_id) else {
        return;
    };
    let Some(current) = threads.current.take() else {
        return;
    };
    threads.past.push(AgentPastThread {
        session_key: current.session_key,
        started_at_ms: current.started_at_ms,
        ended_at_ms: now,
    });
    let excess = threads.past.len().saturating_sub(MAX_PAST_THREADS);
    threads.past.drain(..excess);
}

fn mark_read(state: &mut AgentWorkspace, agent_id: &str, now: u64) -> bool {
    let Some(threads) = state.threads.get_mut(agent_id) else {
        return false;
    };
    let interrupted = threads.current.as_ref().is_some_and(|c| c.interrupted);
    if threads.last_read_at_ms >= threads.last_activity_at_ms && !interrupted {
        return false;
    }
    threads.last_read_at_ms = now.max(threads.last_activity_at_ms);
    if let Some(current) = threads.current.as_mut() {
        current.interrupted = false;
    }
    true
}

fn scope_roots(profile: &AgentProfile) -> Vec<PathBuf> {
    profile
        .home
        .iter()
        .chain(&profile.folders)
        .map(PathBuf::from)
        .collect()
}

fn admit(
    s: &mut AgentWorkspace,
    r: &AgentRoutine,
    id: String,
    due: u64,
    now: u64,
) -> Result<(), SettingsError> {
    if s.runs
        .iter()
        .any(|v| v.agent_id == r.agent_id && v.status.blocks_agent())
    {
        return Err(invalid(
            "This agent already has an active or uncertain run.",
        ));
    }
    let profile = s
        .agents
        .iter()
        .find(|p| p.id == r.agent_id)
        .cloned()
        .ok_or_else(|| invalid("Agent no longer exists."))?;
    if s.runs.len() >= MAX_RUNS {
        let i = s
            .runs
            .iter()
            .position(|v| !v.status.blocks_agent())
            .ok_or_else(|| invalid("Run history is full."))?;
        s.runs.remove(i);
    }
    s.runs.push(AgentRun {
        id,
        routine_id: r.id.clone(),
        agent_id: r.agent_id.clone(),
        name: r.name.clone(),
        scheduled_at_ms: due,
        updated_at_ms: now,
        status: AgentRunStatus::Queued,
        session_key: None,
        detail: String::new(),
        profile,
        prompt: r.prompt.clone(),
        cwd: r.cwd.clone(),
        began: false,
    });
    Ok(())
}

fn unique_appearance(
    state: &AgentWorkspace,
    profile: &mut AgentProfile,
) -> Result<(), SettingsError> {
    use interface_contracts::agents::{AgentColor as C, AgentShape as S};
    let colors = [C::Teal, C::Coral, C::Sky, C::Sand, C::Violet, C::Moss];
    let shapes = [
        S::Droplet,
        S::Blossom,
        S::Pebble,
        S::Triangle,
        S::Diamond,
        S::Arch,
        S::Star,
        S::Hexagon,
        S::Square,
    ];
    let legacy = |color| match color {
        C::Teal => S::Droplet,
        C::Coral => S::Blossom,
        C::Sky => S::Pebble,
        C::Sand => S::Triangle,
        C::Violet => S::Diamond,
        C::Moss => S::Arch,
    };
    if state.agents.iter().any(|p| {
        p.id == profile.id
            && p.color == profile.color
            && p.shape.unwrap_or_else(|| legacy(p.color))
                == profile.shape.unwrap_or_else(|| legacy(profile.color))
    }) {
        return Ok(());
    }
    let desired = profile.shape.unwrap_or_else(|| legacy(profile.color));
    let used = |color, shape| {
        state.agents.iter().any(|p| {
            p.id != profile.id
                && p.color == color
                && p.shape.unwrap_or_else(|| legacy(p.color)) == shape
        })
    };
    if !used(profile.color, desired) {
        profile.shape = Some(desired);
        return Ok(());
    }
    if state.agents.iter().any(|p| p.id == profile.id) {
        return Err(invalid(
            "That shape and color belong to another agent. Choose a different combination.",
        ));
    }
    for shape in shapes {
        for color in colors {
            if !used(color, shape) {
                profile.color = color;
                profile.shape = Some(shape);
                return Ok(());
            }
        }
    }
    Err(invalid("No unused character appearance remains."))
}

#[cfg(test)]
mod tests;

fn make_assignment_room(state: &mut AgentWorkspace, key: &str) -> Result<(), SettingsError> {
    if state.assignments.len() < 500 || state.assignments.contains_key(key) {
        return Ok(());
    }
    let removable = state
        .assignments
        .keys()
        .find(|key| {
            !state
                .runs
                .iter()
                .any(|r| r.session_key.as_ref() == Some(key) && r.status.blocks_agent())
        })
        .cloned();
    if let Some(key) = removable {
        state.assignments.remove(&key);
        Ok(())
    } else {
        Err(invalid(
            "Every retained task is active; cannot discard an assignment.",
        ))
    }
}

fn observed_status(event: &EngineEvent) -> Option<SessionStatus> {
    match &event.event {
        EngineEventKind::StatusChanged { status } => Some(*status),
        EngineEventKind::UserMessage { .. } => Some(SessionStatus::Working),
        EngineEventKind::ApprovalRequested { .. } => Some(SessionStatus::Waiting),
        EngineEventKind::Error {
            retryable: false, ..
        }
        | EngineEventKind::SessionClosed => Some(SessionStatus::Failed),
        _ => None,
    }
}
