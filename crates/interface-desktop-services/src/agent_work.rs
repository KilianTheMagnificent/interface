//! Filesystem evidence and host-facing task operations; the ledger owns lifecycle rules.
use super::agent_control::{home_error, profile_of, refused};
use super::{DesktopService, DesktopServiceError, agent_now_ms};
use interface_contracts::{
    agent_work::{
        AgentCalendarCommand, AgentCalendarView, AgentEvidence, AgentExecutionState,
        AgentTaskRecord, AgentVerificationState, AgentWorkCommand, ArtifactFormat,
    },
    engine::{EngineEvent, EngineEventKind},
};
use interface_settings::{AgentStore, agent_home};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::UNIX_EPOCH,
};

/// The error code of a failed event projection for the agent task bound to the session. The
/// host retires that session; no other `observe_agent_work` error names an agent's session.
pub const AGENT_WORK_PROJECTION_FAILED: &str = "agent-work-projection-failed";

/// Records one event of the task `before` (active on `key`), verifying it once it completes.
fn observe_task_event(
    store: &mut AgentStore,
    key: &str,
    event: &EngineEvent,
    before: &AgentTaskRecord,
) -> Result<(), DesktopServiceError> {
    store
        .work
        .observe(key, event, agent_now_ms())
        .map_err(|e| refused(&e))?;
    if store
        .work
        .task(&before.agent_id, &before.id)
        .is_some_and(|task| task.execution == AgentExecutionState::Completed)
    {
        verify(store, &before.agent_id, &before.id)?;
        // Local revision history only. This does not add memory to provider input.
        if let Ok(profile) = profile_of(store, &before.agent_id)
            && let Ok(files) = agent_home::read_files(&profile)
            && let Some(memory) = files.memory
            && !memory.too_large
        {
            store
                .work
                .remember_memory(
                    &before.agent_id,
                    &memory.text,
                    "observed after provider turn; not owner authorization",
                    agent_now_ms(),
                )
                .map_err(|e| refused(&e))?;
        }
    }
    Ok(())
}

pub(super) fn error(message: &str) -> DesktopServiceError {
    DesktopServiceError::new("agent-work-refused", message, false)
}

pub(super) fn evidence(
    path: &str,
    cwd: &str,
    roots: &[String],
    now: u64,
) -> Result<(AgentEvidence, Vec<u8>, u64), DesktopServiceError> {
    let original = Path::new(cwd).join(path);
    if interface_workspace::is_credential_path(&original.to_string_lossy()) {
        return Err(error("Credential-shaped files cannot be task evidence."));
    }
    let canonical = fs::canonicalize(&original)
        .map_err(|_| error("A required input or artifact is missing."))?;
    if interface_workspace::is_credential_path(&canonical.to_string_lossy())
        || !roots.iter().any(|root| canonical.starts_with(root))
    {
        return Err(error(
            "Evidence must remain inside the task's current granted folders.",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&canonical)
        .map_err(|_| error("Evidence could not be opened safely."))?;
    let metadata = file
        .metadata()
        .map_err(|_| error("Evidence metadata unavailable."))?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(error("Evidence must be a regular file of at most 4 MiB."));
    }
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    if fs::symlink_metadata(&canonical).ok().as_ref().map(identity) != Some(identity(&metadata)) {
        return Err(error("Evidence path changed during opening."));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| error("Evidence could not be read."))?;
    if bytes.len() > 4 * 1024 * 1024
        || fs::canonicalize(&original).ok().as_ref() != Some(&canonical)
        || fs::symlink_metadata(&canonical).ok().as_ref().map(identity) != Some(identity(&metadata))
        || file.metadata().ok().as_ref().map(identity) != Some(identity(&metadata))
        || !roots.iter().any(|root| {
            canonical.starts_with(root)
                && fs::canonicalize(root).ok().as_deref() == Some(Path::new(root))
        })
    {
        return Err(error("Evidence changed while being read."));
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or_else(|| error("Evidence modification time unavailable."))?;
    Ok((
        AgentEvidence {
            path: canonical.to_string_lossy().into_owned(),
            sha256: agent_home::sha256_hex(&bytes),
            bytes: bytes.len() as u64,
            checked_at_ms: now,
        },
        bytes,
        modified,
    ))
}

impl DesktopService {
    /// # Errors
    /// Returns a task only for its exact owner.
    pub fn agent_work_record(
        &self,
        agent: &str,
        id: &str,
    ) -> Result<AgentTaskRecord, DesktopServiceError> {
        self.with_agents(|store| {
            store
                .work
                .task(agent, id)
                .ok_or_else(|| error("Unknown task."))
        })
    }
    /// # Errors
    /// Includes live interactive, routine and uncertain task bindings.
    pub fn agent_live_sessions(&self, agent: &str) -> Result<Vec<String>, DesktopServiceError> {
        self.with_agents(|store| {
            let state = store.workspace();
            let mut keys = std::collections::BTreeSet::new();
            if let Some(key) = state
                .threads
                .get(agent)
                .and_then(|t| t.current.as_ref())
                .map(|t| &t.session_key)
            {
                keys.insert(key.clone());
            }
            keys.extend(
                state
                    .runs
                    .iter()
                    .filter(|r| r.agent_id == agent && r.status.blocks_agent())
                    .filter_map(|r| r.session_key.clone()),
            );
            keys.extend(
                store
                    .work
                    .tasks_of(agent)
                    .filter(|t| t.execution.holds_resources())
                    .filter_map(|t| t.session_key.clone()),
            );
            Ok(keys.into_iter().collect())
        })
    }
    /// Appends only the owner's explicit task settings. Does not read or include memory files.
    /// # Errors
    /// Refuses unavailable state rather than omitting configured acceptance requirements.
    pub fn agent_task_prompt(
        &self,
        agent: &str,
        text: &str,
    ) -> Result<String, DesktopServiceError> {
        self.with_agents(|store| {
            let spec = store.work.spec(agent);
            if spec == interface_contracts::agent_work::AgentWorkSpec::default() { return Ok(text.into()); }
            let encoded = serde_json::to_string(&spec).map_err(|_| error("Could not encode task contract."))?;
            Ok(format!("{text}\n\nInterface task contract (owner settings; output checks do not grant extra authority):\n{encoded}\nKeep the goal and failed checks visible. A worker result or finished turn does not establish verification. Use iface_agent.task_status to recover current state."))
        })
    }
    #[must_use]
    pub fn agent_tool_endpoint_exists(&self, agent: &str, id: &str) -> bool {
        self.with_agents(|store| {
            Ok(store.work.task(agent, id).is_some_and(|task| {
                !task.stop_requested
                    && matches!(
                        task.execution,
                        AgentExecutionState::Starting
                            | AgentExecutionState::Running
                            | AgentExecutionState::Completed
                    )
            }))
        })
        .unwrap_or(false)
    }
    /// # Errors
    /// Checks fresh inputs and reserves all writable resources before provider dispatch.
    pub fn begin_agent_work(
        &self,
        agent: &str,
        id: &str,
        goal: &str,
        cwd: &str,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            let profile = profile_of(store, agent)?;
            let spec = store.work.spec(agent);
            // Local recovery history is not appended to the provider's prompt.
            if let Ok(files) = agent_home::read_files(&profile) && let Some(memory) = files.memory && !memory.too_large {
                store.work.remember_memory(agent, &memory.text, "before provider turn; local history", agent_now_ms()).map_err(|e| refused(&e))?;
            }
            if spec.read_only { return Err(error("Read-only shared work requires a verified read-only provider sandbox; use a scoped consultation.")); }
            let mut roots = profile.folders.clone();
            if let Some(home) = &profile.home { roots.push(home.clone()); }
            if !roots.iter().any(|root| root == cwd) { roots.push(cwd.into()); }
            let roots = roots.iter().map(|root| fs::canonicalize(root).map(|root| root.to_string_lossy().into_owned()).map_err(|_| error("A granted folder is missing."))).collect::<Result<Vec<_>, _>>()?;
            let cwd = fs::canonicalize(cwd).map_err(|_| error("The task folder is missing."))?.to_string_lossy().into_owned();
            let now = agent_now_ms();
            let inputs = spec.inputs.iter().map(|input| {
                let (proof, _, modified) = evidence(&input.path, &cwd, &roots, now)?;
                if modified > now.saturating_add(1000) || now.saturating_sub(modified) > u64::from(input.max_age_seconds) * 1000 { return Err(error("A task input is stale or has a future timestamp.")); }
                Ok(proof)
            }).collect::<Result<Vec<_>, _>>()?;
            store.work.admit(AgentTaskRecord {
                id: id.into(), agent_id: agent.into(), session_key: None, goal: goal.chars().take(8000).collect(), spec, roots, cwd,
                started_at_ms: now, deadline_at_ms: now, progress_at_ms: now, progress_sha256: String::new(), began: false, actions: 0, reported_tokens: None,
                execution: AgentExecutionState::Starting, verification: AgentVerificationState::Unverified, input_evidence: inputs, artifacts: vec![],
                detail: "First provider dispatch has been reserved.".into(), next_action: "Wait for the provider, or stop the task.".into(), stop_requested: false, notes: vec![],
            }).map_err(|e| refused(&e))
        })
    }
    /// # Errors
    /// A bind failure after provider dispatch is outcome-unknown.
    pub fn bind_agent_work(&self, id: &str, key: &str) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| store.work.bind(id, key).map_err(|e| refused(&e)))
    }
    /// The task settings the owner configured for `agent`: what its next
    /// task copies, and what its ordinary conversation and routines follow
    /// while no task is active.
    ///
    /// # Errors
    /// Fails closed on unavailable storage.
    pub fn agent_work_spec(
        &self,
        agent: &str,
    ) -> Result<interface_contracts::agent_work::AgentWorkSpec, DesktopServiceError> {
        self.with_agents(|store| Ok(store.work.spec(agent)))
    }
    /// # Errors
    /// Fails closed on unavailable storage.
    pub fn active_agent_work(
        &self,
        key: &str,
    ) -> Result<Option<AgentTaskRecord>, DesktopServiceError> {
        self.with_agents(|store| Ok(store.work.active(key)))
    }
    /// # Errors
    /// A failed durable update must not allow more provider effects.
    pub fn settle_agent_work(
        &self,
        agent: &str,
        id: &str,
        state: AgentExecutionState,
        detail: &str,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            store
                .work
                .settle(agent, id, state, detail)
                .map_err(|e| refused(&e))?;
            if state == AgentExecutionState::Stopped
                && store
                    .workspace()
                    .runs
                    .iter()
                    .any(|r| r.id == id && r.status.blocks_agent())
            {
                store
                    .fail(id, detail, true, agent_now_ms())
                    .map_err(|e| refused(&e))?;
            }
            Ok(())
        })
    }
    /// # Errors
    /// Stop is durable before interruption.
    pub fn stop_agent_work(
        &self,
        agent: &str,
        id: &str,
        detail: &str,
    ) -> Result<Option<String>, DesktopServiceError> {
        self.with_agents(|store| store.work.stop(agent, id, detail).map_err(|e| refused(&e)))
    }
    /// The tasks whose budgets have expired, or `None` when no task is under a
    /// budget at all: then nothing can expire before the next admission.
    ///
    /// # Errors
    /// Returns only exact task identities whose budgets have expired.
    pub fn expired_agent_work(&self) -> Result<Option<Vec<AgentTaskRecord>>, DesktopServiceError> {
        self.with_agents(|store| {
            Ok(store
                .work
                .has_budgeted()
                .then(|| store.work.expired(agent_now_ms())))
        })
    }
    /// # Errors
    /// Only significant task events are written, never each streaming text delta. An
    /// [`AGENT_WORK_PROJECTION_FAILED`] error belongs to the task bound to `key`. Any other
    /// error means the store could not be read; it never says `key` is an agent's session.
    pub fn observe_agent_work(
        &self,
        key: &str,
        event: &EngineEvent,
    ) -> Result<(), DesktopServiceError> {
        if !matches!(
            event.event,
            EngineEventKind::ToolCall { .. }
                | EngineEventKind::ToolResult { .. }
                | EngineEventKind::UsageUpdated { .. }
                | EngineEventKind::StatusChanged { .. }
                | EngineEventKind::SessionClosed
        ) {
            return Ok(());
        }
        // A store that never opened admitted no task, so it has nothing to record.
        self.with_open_agents(|store| {
            let Some(before) = store.work.active(key) else {
                // Not a task's session. A starting task may buffer it, which never fails.
                return store
                    .work
                    .observe(key, event, agent_now_ms())
                    .map_err(|e| refused(&e));
            };
            observe_task_event(store, key, event, &before).map_err(|error| {
                DesktopServiceError::new(AGENT_WORK_PROJECTION_FAILED, error.message, false)
            })
        })
        .map(|_| ())
    }

    /// # Errors
    /// Strictly scoped owner operations. Stop is intercepted by the host for process closure.
    #[allow(clippy::too_many_lines)]
    pub fn agent_work_command(
        &self,
        agent: &str,
        command: AgentWorkCommand,
    ) -> Result<Value, DesktopServiceError> {
        self.with_agents(|store| {
            let profile = profile_of(store, agent)?;
            match command {
                AgentWorkCommand::Read {} => {}
                AgentWorkCommand::Configure {
                    expected_revision,
                    spec,
                } => store
                    .work
                    .configure(agent, expected_revision, spec)
                    .map_err(|e| refused(&e))?,
                AgentWorkCommand::Verify { task_id } => verify(store, agent, &task_id)?,
                AgentWorkCommand::Stop { .. } => {
                    return Err(error("Stop must close the provider through the host."));
                }
                AgentWorkCommand::Acknowledge { .. } => {
                    return Err(error(
                        "Acknowledgement requires provider closure through the host.",
                    ));
                }
                AgentWorkCommand::RestoreMemory {
                    sha256,
                    expected_sha256,
                } => {
                    if store
                        .work
                        .tasks_of(agent)
                        .any(|t| t.execution.holds_resources())
                    {
                        return Err(error("Stop active work before restoring memory."));
                    }
                    let revision = store
                        .work
                        .memory_history_of(agent)
                        .iter()
                        .find(|r| r.sha256 == sha256)
                        .cloned()
                        .ok_or_else(|| error("Memory revision unavailable."))?;
                    let before = agent_home::read_files(&profile).map_err(|e| home_error(&e))?;
                    if let Some(memory) = before.memory {
                        store
                            .work
                            .remember_memory(
                                agent,
                                &memory.text,
                                "observed before owner restore",
                                agent_now_ms(),
                            )
                            .map_err(|e| refused(&e))?;
                    }
                    agent_home::save_memory(&profile, &expected_sha256, &revision.text)
                        .map_err(|e| home_error(&e))?;
                    store
                        .work
                        .remember_memory(agent, &revision.text, "owner restore", agent_now_ms())
                        .map_err(|e| refused(&e))?;
                }
                AgentWorkCommand::ProposeFact { fact } => store
                    .work
                    .propose_fact(agent, fact, agent_now_ms())
                    .map_err(|e| refused(&e))?,
                AgentWorkCommand::ConfirmFact { id } => store
                    .work
                    .confirm_fact(agent, &id)
                    .map_err(|e| refused(&e))?,
            }
            // A previously verified receipt can become stale; an observation never upgrades it.
            let verified: Vec<_> = store
                .work
                .tasks_of(agent)
                .filter(|t| t.verification == AgentVerificationState::Verified)
                .map(|t| t.id.clone())
                .collect();
            for id in verified {
                verify(store, agent, &id)?;
            }
            if profile.home.is_some()
                && let Ok(files) = agent_home::read_files(&profile)
                && let Some(memory) = files.memory
                && !memory.too_large
            {
                store
                    .work
                    .remember_memory(
                        agent,
                        &memory.text,
                        "observed file; not owner authorization",
                        agent_now_ms(),
                    )
                    .map_err(|e| refused(&e))?;
            }
            let mut response = store.response(&self.pending_approvals());
            response.work = Some(store.work.view(agent));
            serde_json::to_value(response).map_err(|_| error("Could not encode agent task state."))
        })
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn verify(
    store: &mut interface_settings::AgentStore,
    agent: &str,
    id: &str,
) -> Result<(), DesktopServiceError> {
    let task = store
        .work
        .task(agent, id)
        .ok_or_else(|| error("Unknown task."))?;
    if task.execution != AgentExecutionState::Completed {
        return Err(error(
            "Wait for execution to complete before verifying artifacts.",
        ));
    }
    let profile = profile_of(store, agent)?;
    let current_roots: Vec<_> = profile
        .home
        .into_iter()
        .chain(profile.folders)
        .filter_map(|root| fs::canonicalize(root).ok())
        .collect();
    let routine_root = store
        .workspace()
        .runs
        .iter()
        .any(|run| run.id == task.id && run.cwd == task.cwd);
    let roots: Vec<_> = task
        .roots
        .iter()
        .filter(|root| {
            current_roots
                .iter()
                .any(|current| Path::new(root).starts_with(current))
                || routine_root && **root == task.cwd
        })
        .cloned()
        .collect();
    let now = agent_now_ms();
    let checked = (|| {
        if task.spec.artifacts.is_empty() {
            return Err(error(
                "No acceptance checks were specified; result remains unverified.",
            ));
        }
        for input in &task.input_evidence {
            let (current, _, modified) = evidence(&input.path, &task.cwd, &roots, now)?;
            if task
                .spec
                .inputs
                .iter()
                .zip(&task.input_evidence)
                .any(|(required, original)| {
                    original.path == input.path
                        && now.saturating_sub(modified) > u64::from(required.max_age_seconds) * 1000
                })
            {
                return Err(error("A required input is no longer fresh."));
            }
            if current.sha256 != input.sha256 {
                return Err(error("An input changed since admission."));
            }
        }
        task.spec
            .artifacts
            .iter()
            .map(|check| {
                let (proof, bytes, modified) = evidence(&check.path, &task.cwd, &roots, now)?;
                if modified < task.started_at_ms { return Err(error("An artifact predates this task. Produce a fresh output before verification.")); }
                if bytes.is_empty() {
                    return Err(error("An expected artifact is empty."));
                }
                match check.format {
                    ArtifactFormat::Json => {
                        serde_json::from_slice::<Value>(&bytes)
                            .map_err(|_| error("An expected JSON artifact is invalid."))?;
                    }
                    ArtifactFormat::Text => {
                        std::str::from_utf8(&bytes)
                            .map_err(|_| error("An expected text artifact is not UTF-8."))?;
                    }
                    ArtifactFormat::Nonempty => {}
                }
                if let Some(contains) = &check.contains
                    && !std::str::from_utf8(&bytes).is_ok_and(|text| text.contains(contains))
                {
                    return Err(error("An artifact failed its required text check."));
                }
                Ok(proof)
            })
            .collect::<Result<Vec<_>, _>>()
    })();
    let (state, proofs, detail) = match checked {
        Ok(proofs)
            if task.verification == AgentVerificationState::Verified
                && task
                    .artifacts
                    .iter()
                    .zip(&proofs)
                    .any(|(a, b)| a.sha256 != b.sha256 || a.path != b.path) =>
        {
            (
                AgentVerificationState::Stale,
                proofs,
                "Artifact bytes changed after verification; inspect and verify the new result."
                    .into(),
            )
        }
        Ok(proofs) => (
            AgentVerificationState::Verified,
            proofs,
            "All declared deterministic artifact checks passed for these exact bytes.".into(),
        ),
        Err(error) => (
            if task.verification == AgentVerificationState::Verified {
                AgentVerificationState::Stale
            } else if task.spec.artifacts.is_empty() {
                AgentVerificationState::Unverified
            } else {
                AgentVerificationState::Failed
            },
            vec![],
            error.message,
        ),
    };
    if state == task.verification
        && proofs
            .iter()
            .map(|p| (&p.path, &p.sha256))
            .eq(task.artifacts.iter().map(|p| (&p.path, &p.sha256)))
        && detail == task.detail
    {
        return Ok(());
    }
    store
        .work
        .verification(agent, id, state, proofs, detail)
        .map_err(|e| refused(&e))?;
    if matches!(
        state,
        AgentVerificationState::Failed | AgentVerificationState::Stale
    ) {
        store.pause_failed_calendar(id).map_err(|e| refused(&e))?;
    }
    Ok(())
}

impl DesktopService {
    /// # Errors
    /// Calendar edits pause the routine; enabling requires a successful test of current inputs.
    pub fn agent_calendar_command(
        &self,
        agent: &str,
        command: AgentCalendarCommand,
    ) -> Result<Value, DesktopServiceError> {
        self.with_agents(|store| {
            profile_of(store, agent)?;
            let (routine_id, schedule) = match command {
                AgentCalendarCommand::Preview { schedule, after_ms } => {
                    let occurrences =
                        interface_settings::calendar_occurrences(&schedule, after_ms, 3)
                            .map_err(|e| refused(&e))?;
                    let mut response = store.response(&self.pending_approvals());
                    response.calendar = Some(AgentCalendarView {
                        routine_id: String::new(),
                        schedule: Some(schedule),
                        next_occurrences_ms: occurrences,
                        tested: false,
                    });
                    return serde_json::to_value(response)
                        .map_err(|_| error("Could not encode calendar preview."));
                }
                AgentCalendarCommand::Read { routine_id } => {
                    let schedule = store.work.calendar(&routine_id);
                    (routine_id, schedule)
                }
                AgentCalendarCommand::Save {
                    expected_revision,
                    routine_id,
                    schedule,
                } => {
                    store
                        .configure_calendar(
                            agent,
                            &routine_id,
                            expected_revision,
                            schedule.clone(),
                            agent_now_ms(),
                        )
                        .map_err(|e| refused(&e))?;
                    (routine_id, schedule)
                }
            };
            let routine = store
                .workspace()
                .routines
                .iter()
                .find(|r| r.id == routine_id && r.agent_id == agent)
                .cloned()
                .ok_or_else(|| error("Unknown routine."))?;
            let next = match &schedule {
                Some(schedule) => {
                    interface_settings::calendar_occurrences(schedule, agent_now_ms(), 3)
                        .map_err(|e| refused(&e))?
                }
                None => routine.next_run_at_ms.into_iter().collect(),
            };
            let mut response = store.response(&self.pending_approvals());
            response.calendar = Some(AgentCalendarView {
                routine_id,
                schedule,
                next_occurrences_ms: next,
                tested: store.routine_tested(&routine),
            });
            serde_json::to_value(response).map_err(|_| error("Could not encode calendar state."))
        })
    }
}

impl DesktopService {
    /// # Errors
    /// Returns exactly one active task in this canonical folder; never widens an ambiguous match.
    pub fn agent_tool_task_for_cwd(
        &self,
        cwd: &Path,
    ) -> Result<Option<AgentTaskRecord>, DesktopServiceError> {
        let cwd = fs::canonicalize(cwd).map_err(|_| error("Task folder is unavailable."))?;
        self.with_agents(|store| {
            let mut tasks = store
                .workspace()
                .agents
                .iter()
                .flat_map(|agent| store.work.tasks_of(&agent.id))
                .filter(|task| {
                    matches!(
                        task.execution,
                        AgentExecutionState::Starting | AgentExecutionState::Running
                    ) && !task.stop_requested
                        && Path::new(&task.cwd) == cwd
                });
            // Exactly one match, or none: a second one makes it ambiguous.
            Ok(match (tasks.next(), tasks.next()) {
                (Some(task), None) => Some(task.clone()),
                _ => None,
            })
        })
    }
    /// # Errors
    /// Tool authority comes only from the bound task, never from tool arguments.
    pub fn agent_tool_task(
        &self,
        agent: &str,
        id: &str,
    ) -> Result<AgentTaskRecord, DesktopServiceError> {
        self.with_agents(|store| {
            let origin = store
                .work
                .task(agent, id)
                .ok_or_else(|| error("Unknown task."))?;
            // A connection may survive later turns of the same live thread, but cannot
            // transfer authority to another session, expanded roots or recovered work.
            let task =
                if origin.execution == AgentExecutionState::Completed && !origin.stop_requested {
                    origin
                        .session_key
                        .as_deref()
                        .and_then(|key| store.work.active(key))
                        .filter(|task| {
                            task.agent_id == agent
                                && task.roots == origin.roots
                                && task.cwd == origin.cwd
                        })
                        .ok_or_else(|| error("Task admission is closed."))?
                } else {
                    origin
                };
            if !store.work.admits(&task, agent_now_ms()) {
                return Err(error("Task admission is closed."));
            }
            Ok(task)
        })
    }
    /// # Errors
    /// Bounded recovery metadata; memory text and unrelated task context are not inserted.
    pub fn agent_task_context(
        &self,
        agent: &str,
        task: &AgentTaskRecord,
    ) -> Result<Value, DesktopServiceError> {
        let pending = self
            .pending_approvals()
            .iter()
            .filter(|p| p.agent_id == agent && Some(&p.session_key) == task.session_key.as_ref())
            .cloned()
            .collect::<Vec<_>>();
        self.with_agents(|store| {
            let view = store.work.view(agent);
            let children = view.consultations.iter().filter(|c| c.task_id == task.id).map(|c| serde_json::json!({"id":c.id,"model":c.model,"inputs":c.inputs,"status":c.status,"review":c.review,"resultSha256":c.result_sha256,"elapsedMs":c.elapsed_ms,"reportedTokens":c.reported_tokens})).collect::<Vec<_>>();
            Ok(serde_json::json!({"version":1,"task":task,"cost":null,"costStatus":"unavailable","pendingApprovalCount":pending.len(),"pendingApprovals":pending.into_iter().take(32).collect::<Vec<_>>(),"consultations":children,"memorySha256":view.memory_history.last().map(|r| &r.sha256),"recovery":"Never replay uncertain work. Mutation request IDs return their recorded receipts only for identical inputs. Owner approvals remain owner decisions."}))
        })
    }
    /// # Errors
    /// Changes are restricted to task records or this agent's compare-and-save memory.
    pub fn agent_tool_mutation(
        &self,
        agent: &str,
        id: &str,
        request: &str,
        name: &str,
        input: &Value,
    ) -> Result<String, DesktopServiceError> {
        let task = self.agent_tool_task(agent, id)?;
        let id = task.id.as_str();
        let encoded =
            serde_json::to_vec(&(name, input)).map_err(|_| error("Invalid tool arguments."))?;
        if let Some(cached) = self.with_agents(|store| {
            store
                .work
                .reserve_tool(id, request, &encoded)
                .map_err(|e| refused(&e))
        })? {
            return Ok(cached);
        }
        let result = self.with_agents(|store| {
            let current = store.work.task(agent, id).ok_or_else(|| error("Task disappeared."))?;
            if !store.work.admits(&current, agent_now_ms()) { return Err(error("Task admission is closed.")); }
            let result = match name {
                "consult_review" | "consult_apply" => super::agent_consultation::review_or_apply(store, agent, id, name, input)?,
                "artifact_register" => {
                    let path = input.get("path").and_then(Value::as_str).ok_or_else(|| error("Missing artifact path."))?;
                    let (proof, _, _) = evidence(path, &task.cwd, &task.roots, agent_now_ms())?;
                    store.work.register_artifact(agent, id, proof.clone()).map_err(|e| refused(&e))?;
                    serde_json::json!({"artifact":proof,"verification":"unverified"})
                }
                "memory_propose" => {
                    let hash = input.get("expectedSha256").and_then(Value::as_str).ok_or_else(|| error("Missing expected memory hash."))?;
                    let text = input.get("text").and_then(Value::as_str).ok_or_else(|| error("Missing memory text."))?;
                    let profile = profile_of(store, agent)?;
                    let files = agent_home::read_files(&profile).map_err(|e| home_error(&e))?;
                    if let Some(memory) = files.memory { store.work.remember_memory(agent, &memory.text, "before agent compare-and-save", agent_now_ms()).map_err(|e| refused(&e))?; }
                    let sha = agent_home::save_memory(&profile, hash, text).map_err(|e| home_error(&e))?;
                    store.work.remember_memory(agent, text, "agent proposal; not owner authorization", agent_now_ms()).map_err(|e| refused(&e))?;
                    serde_json::json!({"sha256":sha,"source":"agent"})
                }
                "notification_request" => {
                    let text = input.get("text").and_then(Value::as_str).ok_or_else(|| error("Missing notification text."))?;
                    store.work.note(agent, id, &format!("Agent request: {text}")).map_err(|e| refused(&e))?;
                    serde_json::json!({"queuedForOwner":true,"destination":"task results"})
                }
                "routine_draft" => {
                    let name = input.get("name").and_then(Value::as_str).ok_or_else(|| error("Missing routine name."))?;
                    let prompt = input.get("prompt").and_then(Value::as_str).ok_or_else(|| error("Missing routine prompt."))?;
                    let routine_id = uuid::Uuid::new_v4().to_string();
                    let routine = interface_contracts::agents::AgentRoutine { id: routine_id.clone(), agent_id: agent.into(), name: name.into(), prompt: prompt.into(), cwd: task.cwd.clone(), enabled: false, start_at_ms: agent_now_ms().saturating_add(3_600_000), interval_minutes: Some(1440), next_run_at_ms: None };
                    store.command(interface_contracts::agents::AgentCommand::SaveRoutine { expected_revision: store.revision(), routine }, agent_now_ms()).map_err(|e| refused(&e))?;
                    serde_json::json!({"routineId":routine_id,"enabled":false,"requiresOwnerReview":true})
                }
                _ => return Err(error("Unknown or untrusted host tool.")),
            };
            Ok(result.to_string())
        });
        // Even a refused local mutation gets a final receipt; an uncertain save cannot replay.
        let encoded = match &result {
            Ok(result) => result.clone(),
            Err(e) => serde_json::json!({"error":e.message}).to_string(),
        };
        self.with_agents(|store| {
            store
                .work
                .settle_tool(id, request, encoded.clone())
                .map_err(|e| refused(&e))
        })?;
        Ok(encoded)
    }
}
