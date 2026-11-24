//! Agent teammates: the agent store, home folders, the decision log and pending owner approvals.
//!
//! Lock order: `agents` before `pending_agent_approvals`, `agent_decisions` and `agents_root`;
//! none of the latter is ever held while taking `agents`.
use super::{DesktopService, DesktopServiceError, agent_now_ms};
use interface_contracts::{
    agents::{
        AgentCommand, AgentDecision, AgentFiles, AgentPendingApproval, AgentProfile, AgentRun,
    },
    engine::EngineEvent,
};
use interface_settings::{
    AGENT_TASK_RUN_PREFIX, AgentBinding, AgentDecisionLog, AgentStore, AgentTaskAdmission,
    AgentTaskRequest, SettingsError,
    agent_home::{self, HomeError},
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{MutexGuard, PoisonError, TryLockError},
};

/// What the host needs to launch an agent's first thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentThreadLaunch {
    pub engine: String,
    pub model: String,
    pub effort: String,
    pub name: String,
    pub home: String,
    pub folders: Vec<String>,
    /// The agent's permission mode as launch access; `None` launches behind the approval gate.
    pub access: Option<interface_contracts::engine::AgentAccess>,
}

/// What a command adds to the workspace response, and a home to show in Finder once the
/// store lock is released.
#[derive(Default)]
struct CommandExtras {
    files: Option<AgentFiles>,
    decisions: Option<Vec<AgentDecision>>,
    reveal: Option<String>,
}

pub(super) fn open_agents(settings: &Path) -> (Option<AgentStore>, Option<AgentDecisionLog>) {
    let Some(root) = settings.parent() else {
        return (None, None);
    };
    let store = AgentStore::open(&root.join("agents.v2.json"), &root.join("agents.v1.json"))
        .inspect_err(|error| eprintln!("[desktop] agent workspace unavailable: {error}"))
        .ok();
    let decisions = AgentDecisionLog::open(&root.join("agents").join("decisions"))
        .inspect_err(|error| eprintln!("[desktop] agent decision log unavailable: {error}"))
        .ok();
    (store, decisions)
}

fn unavailable(message: &str) -> DesktopServiceError {
    DesktopServiceError::new("agent-unavailable", message, false)
}

pub(super) fn refused(error: &SettingsError) -> DesktopServiceError {
    DesktopServiceError::new("agent-state-refused", error.to_string(), false)
}

pub(super) fn home_error(error: &HomeError) -> DesktopServiceError {
    let code = if *error == HomeError::Conflict {
        "agent-file-conflict"
    } else {
        "agent-state-refused"
    };
    DesktopServiceError::new(code, error.to_string(), false)
}

pub(super) fn profile_of(
    store: &AgentStore,
    agent_id: &str,
) -> Result<AgentProfile, DesktopServiceError> {
    store.profile(agent_id).cloned().ok_or_else(|| {
        DesktopServiceError::new("agent-state-refused", "Agent no longer exists.", false)
    })
}

/// Creates the home on first use (or verifies it) and records it when the store lacks it.
fn ensure_agent_home(
    store: &mut AgentStore,
    root: &Path,
    profile: &AgentProfile,
) -> Result<String, DesktopServiceError> {
    let (home, sha) = agent_home::ensure_home(root, profile, agent_now_ms())
        .map_err(|error| home_error(&error))?;
    if profile.home.as_deref() != Some(home.as_str())
        || profile.instructions_file_sha256.as_deref() != Some(sha.as_str())
    {
        store
            .set_home(&profile.id, &home, &sha)
            .map_err(|error| refused(&error))?;
    }
    Ok(home)
}

/// Regenerates the instruction files after a profile or folder change. A conflict, missing or
/// foreign folder leaves every file untouched; `ReadAgentFiles` reports it.
fn regenerate_instructions(
    store: &mut AgentStore,
    agent_id: &str,
) -> Result<(), DesktopServiceError> {
    let profile = profile_of(store, agent_id)?;
    let (Some(_), Some(sha)) = (&profile.home, &profile.instructions_file_sha256) else {
        return Ok(());
    };
    match agent_home::write_instruction_files(&profile, sha) {
        Ok(written) => store
            .set_instructions_sha(agent_id, &written)
            .map_err(|error| refused(&error)),
        Err(HomeError::Io(error)) => {
            eprintln!("[desktop] agent instruction files were not regenerated: {error}");
            Ok(())
        }
        Err(_) => Ok(()),
    }
}

/// Read, memory save or instruction overwrite; every one answers with the refreshed files.
fn agent_files(
    store: &mut AgentStore,
    command: AgentCommand,
) -> Result<AgentFiles, DesktopServiceError> {
    let agent_id = match command {
        AgentCommand::ReadAgentFiles { agent_id } => agent_id,
        AgentCommand::SaveAgentMemory {
            agent_id,
            expected_sha256,
            text,
        } => {
            let profile = profile_of(store, &agent_id)?;
            if store
                .work
                .tasks_of(&agent_id)
                .any(|task| task.execution.holds_resources())
            {
                return Err(unavailable("Stop active work before editing memory."));
            }
            if let Some(memory) = agent_home::read_files(&profile)
                .map_err(|e| home_error(&e))?
                .memory
            {
                store
                    .work
                    .remember_memory(
                        &agent_id,
                        &memory.text,
                        "observed before owner save",
                        agent_now_ms(),
                    )
                    .map_err(|e| refused(&e))?;
            }
            agent_home::save_memory(&profile, &expected_sha256, &text)
                .map_err(|error| home_error(&error))?;
            store
                .work
                .remember_memory(&agent_id, &text, "owner save", agent_now_ms())
                .map_err(|e| refused(&e))?;
            agent_id
        }
        AgentCommand::OverwriteAgentInstructionsFile {
            agent_id,
            expected_sha256,
        } => {
            let profile = profile_of(store, &agent_id)?;
            let written = agent_home::write_instruction_files(&profile, &expected_sha256)
                .map_err(|error| home_error(&error))?;
            store
                .set_instructions_sha(&agent_id, &written)
                .map_err(|error| refused(&error))?;
            agent_id
        }
        _ => {
            return Err(DesktopServiceError::new(
                "invalid-payload",
                "Invalid agent command.",
                false,
            ));
        }
    };
    agent_home::read_files(&profile_of(store, &agent_id)?).map_err(|error| home_error(&error))
}

/// Saves the profile, then regenerates the instruction files when their text would change.
fn save_agent(
    store: &mut AgentStore,
    command: AgentCommand,
    now: u64,
) -> Result<(), DesktopServiceError> {
    let AgentCommand::SaveAgent { profile, .. } = &command else {
        return Ok(());
    };
    let agent_id = profile.id.clone();
    let before = profile_of(store, &agent_id).ok();
    store
        .command(command, now)
        .map_err(|error| refused(&error))?;
    let after = profile_of(store, &agent_id)?;
    if let (Some(before), Some(home)) = (before, after.home.as_deref())
        && agent_home::render_instructions(&before, home)
            != agent_home::render_instructions(&after, home)
    {
        regenerate_instructions(store, &agent_id)?;
    }
    Ok(())
}

/// Canonical form of `path`; a missing tail is appended to its longest existing ancestor.
fn resolved(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(current) {
            return missing
                .iter()
                .rev()
                .fold(canonical, |resolved, name| resolved.join(name));
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn reveal(home: &Path) -> Result<(), HomeError> {
    // Tests never open Finder unless explicitly asked to.
    if cfg!(test) && std::env::var_os("INTERFACE_TEST_REVEAL").is_none_or(|value| value != "1") {
        return Ok(());
    }
    agent_home::reveal(home)
}

#[cfg(test)]
thread_local! {
    /// Whether the agent store was unlocked where a test's reveal was called.
    pub(super) static REVEALED_UNLOCKED: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

impl DesktopService {
    /// Validates a folder update before the host closes sessions holding old grants.
    /// # Errors
    /// Refuses stale revisions and ungrantable folders without disrupting live work.
    pub fn check_agent_folder_change(
        &self,
        payload: Value,
    ) -> Result<Vec<String>, DesktopServiceError> {
        let AgentCommand::SaveAgentFolders {
            expected_revision,
            agent_id,
            folders,
        } = serde_json::from_value(payload).map_err(|_| unavailable("Invalid folder command."))?
        else {
            return Err(unavailable("Invalid folder command."));
        };
        self.with_agents(|store| {
            if store.revision() != expected_revision {
                return Err(unavailable(
                    "Agent workspace changed. Reload before saving folders.",
                ));
            }
            let profile = profile_of(store, &agent_id)?;
            if folders.len() > 16 {
                return Err(unavailable("An agent can use at most 16 folders."));
            }
            let canonical = folders
                .iter()
                .map(|folder| self.grantable_folder(folder, &profile))
                .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
            if canonical.len() != folders.len() {
                return Err(unavailable("Each folder can be listed only once."));
            }
            let snapshot = store.workspace();
            let mut keys = snapshot
                .threads
                .get(&agent_id)
                .and_then(|thread| thread.current.as_ref())
                .map(|current| vec![current.session_key.clone()])
                .unwrap_or_default();
            keys.extend(
                snapshot
                    .runs
                    .iter()
                    .filter(|run| run.agent_id == agent_id && run.status.blocks_agent())
                    .filter_map(|run| run.session_key.clone()),
            );
            keys.sort();
            keys.dedup();
            Ok(keys)
        })
    }
    /// # Errors
    /// Refuses invalid input or unavailable durable intent storage before launch.
    pub fn reserve_agent_start(
        &self,
        request: &str,
        agent: &str,
        text: &str,
    ) -> Result<interface_settings::AgentStartAdmission, DesktopServiceError> {
        self.with_agents(|store| {
            store
                .reserve_start(request, agent, text)
                .map_err(|error| refused(&error))
        })
    }
    /// # Errors
    /// A failed outcome write must be treated as unknown by the caller.
    pub fn settle_agent_start(
        &self,
        request: &str,
        key: Option<String>,
        unsent: bool,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            store
                .settle_start(request, key, unsent)
                .map_err(|error| refused(&error))
        })
    }
    pub(super) fn with_agents<T>(
        &self,
        operation: impl FnOnce(&mut AgentStore) -> Result<T, DesktopServiceError>,
    ) -> Result<T, DesktopServiceError> {
        self.with_open_agents(operation)?.ok_or_else(|| {
            unavailable("Agent storage could not be opened. Existing data has been preserved.")
        })
    }
    /// Like [`Self::with_agents`], but `Ok(None)` when the store never opened: then no agent
    /// work could have been admitted in this process.
    pub(super) fn with_open_agents<T>(
        &self,
        operation: impl FnOnce(&mut AgentStore) -> Result<T, DesktopServiceError>,
    ) -> Result<Option<T>, DesktopServiceError> {
        let mut guard = self
            .agents
            .lock()
            .map_err(|_| unavailable("Agent storage lock is unavailable."))?;
        guard.as_mut().map(operation).transpose()
    }

    pub(super) fn pending_approvals(&self) -> MutexGuard<'_, Vec<AgentPendingApproval>> {
        // The list holds no invariant a panic could break; keep serving it.
        self.pending_agent_approvals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn agents_root(&self) -> PathBuf {
        self.agents_root
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_else(|| agent_home::agents_root(&self.home_directory, false))
    }

    /// # Errors
    /// Returns storage errors before any provider can be dispatched.
    pub fn claim_agent_run(&self) -> Result<Option<AgentRun>, DesktopServiceError> {
        self.with_agents(|store| store.claim(agent_now_ms()).map_err(|error| refused(&error)))
    }
    /// Milliseconds until a claim could next admit or start a run (0 when one
    /// is already due), or `None` when nothing is scheduled.
    ///
    /// # Errors
    /// Returns storage errors, like `claim_agent_run`.
    pub fn agent_claim_wait_ms(&self) -> Result<Option<u64>, DesktopServiceError> {
        self.with_agents(|store| {
            Ok(store
                .next_claim_at()
                .map(|due| due.saturating_sub(agent_now_ms())))
        })
    }
    /// The agent store's committed revision, or `None` while it is unavailable.
    #[must_use]
    pub fn agent_revision(&self) -> Option<u64> {
        self.with_agents(|store| Ok(store.revision())).ok()
    }
    /// # Errors
    /// Returns a failed durable binding before the prompt is sent.
    pub fn bind_agent_run(&self, id: &str, key: &str) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            store
                .bind(id, key, agent_now_ms())
                .map_err(|error| refused(&error))
        })
    }
    /// # Errors
    /// Returns any durable failure; never retries the provider.
    pub fn fail_agent_run(
        &self,
        id: &str,
        detail: &str,
        unsent: bool,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            store
                .fail(id, detail, unsent, agent_now_ms())
                .map_err(|error| refused(&error))
        })
    }
    /// Projects a session event onto the agent runs and threads. Returns whether it committed
    /// a new store revision (a run or thread changed), which is what wakes the scheduler.
    /// # Errors
    /// Returns any durable event projection failure.
    pub fn observe_agent_event(
        &self,
        key: &str,
        event: &EngineEvent,
    ) -> Result<bool, DesktopServiceError> {
        // Most events are text deltas the projection ignores; skip the lock.
        if !AgentStore::observes(event) {
            return Ok(false);
        }
        self.with_agents(|store| {
            let before = store.revision();
            store
                .observe(key, event, agent_now_ms())
                .map_err(|error| refused(&error))?;
            Ok(store.revision() != before)
        })
    }

    /// The host derives the profile root (`Interface Agents` or `Interface Dev Agents`).
    pub fn set_agents_root(&self, root: PathBuf) {
        *self
            .agents_root
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(root);
    }

    #[must_use]
    pub fn home_directory(&self) -> &Path {
        &self.home_directory
    }

    /// The grant rule. This host's home directory and agents root are resolved once, on first
    /// use: most callers (every non-agent session's binding) never consult the rule.
    fn grant_check(&self) -> impl Fn(&Path) -> bool + use<> {
        let home_directory = self.home_directory.clone();
        let agents_root = self.agents_root();
        let bases = std::cell::OnceCell::new();
        move |path| {
            std::fs::canonicalize(path).is_ok_and(|canonical| {
                let (home_directory, agents_root) =
                    bases.get_or_init(|| (resolved(&home_directory), resolved(&agents_root)));
                agent_home::is_grantable_root(&canonical, home_directory, agents_root)
            })
        }
    }

    /// Whether an existing `path` may be an agent's working root: a granted folder or a gated
    /// routine run's cwd. Refuses `/`, system locations, the home directory, the agents root,
    /// their ancestors and anything inside the agents root.
    #[must_use]
    pub fn is_grantable_root(&self, path: &Path) -> bool {
        self.grant_check()(path)
    }

    #[must_use]
    pub fn agent_binding(&self, session_key: &str) -> Option<AgentBinding> {
        // Build the grant rule before taking the store lock; it reads `agents_root`. Only a
        // routine run outside its roots consults it, resolving the bases under the lock.
        let grantable = self.grant_check();
        self.agents
            .lock()
            .ok()?
            .as_ref()?
            .binding(session_key, grantable)
    }

    /// [`Self::agent_binding`] without waiting: `None` while another caller holds the agent
    /// store (a commit holds it across fsync), so an async caller can wait off its worker.
    #[must_use]
    pub fn try_agent_binding(&self, session_key: &str) -> Option<Option<AgentBinding>> {
        let grantable = self.grant_check();
        match self.agents.try_lock() {
            Ok(guard) => Some(
                guard
                    .as_ref()
                    .and_then(|store| store.binding(session_key, grantable)),
            ),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => Some(None),
        }
    }

    /// Holds the agent store until the returned guard drops, as a commit does across its
    /// fsync. Only for tests that prove a caller waits for the store off its async workers.
    #[doc(hidden)]
    #[must_use]
    pub fn hold_agent_store(&self) -> impl Drop + '_ {
        self.agents.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a session may not be resumed, sent to or reconfigured.
    ///
    /// A session whose registry working folder (`registry_cwd`) lies inside the agents root is
    /// an agent-home session: it is writable only while it is an agent's current thread or an
    /// unsettled routine run's session. It fails closed: with the store unavailable, and for a
    /// fork or a thread older than the past-thread list, it is read-only. Outside the agents
    /// root, a recorded past thread is read-only too.
    #[must_use]
    pub fn is_read_only_agent_session(
        &self,
        session_key: &str,
        registry_cwd: Option<&Path>,
    ) -> bool {
        if registry_cwd.is_some_and(|cwd| self.is_inside_agents_root(cwd)) {
            return !self.with_agent_store(|store| store.is_live_agent_session(session_key));
        }
        self.with_agent_store(|store| store.is_past_thread(session_key))
    }

    /// Whether an absolute `path` resolves inside the agents root (the root itself included).
    #[must_use]
    pub fn is_inside_agents_root(&self, path: &Path) -> bool {
        path.is_absolute() && resolved(path).starts_with(resolved(&self.agents_root()))
    }

    /// A read of the agent store; `false` when it is unavailable.
    fn with_agent_store(&self, read: impl FnOnce(&AgentStore) -> bool) -> bool {
        self.agents
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(read))
            .unwrap_or(false)
    }

    /// The agent's current launch access, read at each launch and resume; `None` when it is
    /// gated, unknown, or the store is unavailable.
    #[must_use]
    pub fn agent_access(&self, agent_id: &str) -> Option<interface_contracts::engine::AgentAccess> {
        self.agents
            .lock()
            .ok()?
            .as_ref()?
            .permission_mode(agent_id)
            .access()
    }

    /// # Errors
    /// Refuses an unknown agent, an existing thread, or a missing or foreign home folder.
    pub fn prepare_agent_thread(
        &self,
        agent_id: &str,
    ) -> Result<AgentThreadLaunch, DesktopServiceError> {
        let root = self.agents_root();
        self.with_agents(|store| {
            let profile = profile_of(store, agent_id)?;
            if store
                .workspace()
                .threads
                .get(agent_id)
                .is_some_and(|t| t.current.is_some())
            {
                return Err(DesktopServiceError::new(
                    "agent-state-refused",
                    "This agent already has a conversation; send to it instead.",
                    false,
                ));
            }
            let home = ensure_agent_home(store, &root, &profile)?;
            Ok(AgentThreadLaunch {
                access: store.permission_mode(agent_id).access(),
                engine: profile.engine,
                model: profile.model,
                effort: profile.effort,
                name: profile.name,
                home,
                folders: profile.folders,
            })
        })
    }

    /// Prepares the agent's home (folder, instruction files, `MEMORY.md`) and admits the
    /// owner's one-off task as a gated run (protocol 38). The same request returns what it
    /// admitted before. A new task is refused while the agent has active or uncertain work,
    /// before any run is recorded.
    /// # Errors
    /// Refuses an unknown agent, a missing or foreign home, a busy agent, invalid input, or a
    /// failed write. Nothing has reached a provider.
    pub fn admit_agent_task(
        &self,
        request: &AgentTaskRequest<'_>,
    ) -> Result<AgentTaskAdmission, DesktopServiceError> {
        let root = self.agents_root();
        self.with_agents(|store| {
            let id = format!("{AGENT_TASK_RUN_PREFIX}{}", request.request_id);
            if !store.workspace().runs.iter().any(|run| run.id == id) {
                let profile = profile_of(store, request.agent_id)?;
                if store
                    .work
                    .view(request.agent_id)
                    .tasks
                    .iter()
                    .any(|task| task.execution.holds_resources())
                {
                    return Err(DesktopServiceError::new(
                        "agent-state-refused",
                        "This agent is still working on something. Wait for it to finish, or stop it, before giving it a new task.",
                        false,
                    ));
                }
                ensure_agent_home(store, &root, &profile)?;
            }
            store
                .admit_task(request, agent_now_ms())
                .map_err(|error| refused(&error))
        })
    }

    /// One roster profile, or `None` for an unknown agent or an unavailable store.
    #[must_use]
    pub fn agent_profile(&self, agent_id: &str) -> Option<AgentProfile> {
        self.agents
            .lock()
            .ok()?
            .as_ref()?
            .profile(agent_id)
            .cloned()
    }

    /// # Errors
    /// Refuses an unknown agent, a session owned by another agent, or a failed write.
    pub fn bind_agent_thread(
        &self,
        agent_id: &str,
        session_key: &str,
    ) -> Result<(), DesktopServiceError> {
        self.with_agents(|store| {
            store
                .bind_thread(agent_id, session_key, agent_now_ms())
                .map_err(|error| refused(&error))
        })
    }

    /// # Errors
    /// Returns an unavailable log or a failed append; the caller then asks the owner.
    pub fn record_agent_decision(
        &self,
        agent_id: &str,
        entry: &AgentDecision,
    ) -> Result<(), DesktopServiceError> {
        if matches!(
            entry.outcome,
            interface_contracts::agents::AgentDeliveryOutcome::Sent
                | interface_contracts::agents::AgentDeliveryOutcome::Unknown
        ) {
            self.with_agents(|store| {
                store
                    .work
                    .finish_approvals(&entry.session_key, Some(&entry.approval_id), false)
                    .map_err(|e| refused(&e))
            })?;
        }
        let mut guard = self
            .agent_decisions
            .lock()
            .map_err(|_| unavailable("Agent decision log lock is unavailable."))?;
        let log = guard
            .as_mut()
            .ok_or_else(|| unavailable("The agent decision log is unavailable."))?;
        log.append(agent_id, entry)
            .map_err(|error| unavailable(&format!("Could not record the decision: {error}")))
    }

    /// Held until answered or the provider session ends; unresolved requests are never evicted.
    pub fn add_pending_agent_approval(&self, pending: AgentPendingApproval) -> bool {
        if self
            .with_agents(|store| {
                store
                    .work
                    .pending_approval(pending.clone())
                    .map_err(|e| refused(&e))
            })
            .is_err()
        {
            return false;
        }
        let mut list = self.pending_approvals();
        list.retain(|p| {
            p.session_key != pending.session_key || p.approval_id != pending.approval_id
        });
        list.push(pending);
        true
    }

    #[must_use]
    pub fn take_pending_agent_approval(
        &self,
        session_key: &str,
        approval_id: &str,
    ) -> Option<AgentPendingApproval> {
        let mut list = self.pending_approvals();
        let index = list
            .iter()
            .position(|p| p.session_key == session_key && p.approval_id == approval_id)?;
        Some(list.remove(index))
    }

    pub fn clear_pending_agent_approvals(&self, session_key: &str) {
        let _ = self.with_agents(|store| {
            store
                .work
                .finish_approvals(session_key, None, true)
                .map_err(|e| refused(&e))
        });
        self.pending_approvals()
            .retain(|p| p.session_key != session_key);
    }

    /// # Errors
    /// Returns an unavailable store or an encoding failure.
    pub fn agent_control_response(&self) -> Result<Value, DesktopServiceError> {
        self.agent_control(serde_json::json!({"kind": "read"}))
    }

    pub(super) fn agent_control(&self, payload: Value) -> Result<Value, DesktopServiceError> {
        let command: AgentCommand = serde_json::from_value(payload).map_err(|_| {
            DesktopServiceError::new("invalid-payload", "Invalid agent command.", false)
        })?;
        let command = match command {
            AgentCommand::Calendar { agent_id, command } => {
                return self.agent_calendar_command(&agent_id, command);
            }
            AgentCommand::Work { agent_id, command } => {
                return self.agent_work_command(&agent_id, command);
            }
            command => command,
        };
        let root = self.agents_root();
        let (response, home) = self.with_agents(|store| {
            let extras = self.run_agent_command(store, command, &root)?;
            let mut response = store.response(&self.pending_approvals());
            response.files = extras.files;
            response.decisions = extras.decisions;
            Ok((response, extras.reveal))
        })?;
        // Every session's event tap takes the store lock; Finder must not hold it up.
        if let Some(home) = home {
            #[cfg(test)]
            REVEALED_UNLOCKED.set(Some(self.agents.try_lock().is_ok()));
            reveal(Path::new(&home)).map_err(|error| home_error(&error))?;
        }
        serde_json::to_value(response).map_err(|_| {
            DesktopServiceError::new(
                "agent-encode-failed",
                "Could not encode the agent workspace.",
                false,
            )
        })
    }

    #[allow(clippy::too_many_lines)]
    fn run_agent_command(
        &self,
        store: &mut AgentStore,
        command: AgentCommand,
        root: &Path,
    ) -> Result<CommandExtras, DesktopServiceError> {
        use AgentCommand as C;
        let now = agent_now_ms();
        match command {
            C::Calendar { .. } | C::Work { .. } | C::StartAgentThread { .. } => {
                Err(DesktopServiceError::new(
                    "invalid-payload",
                    "Starting a conversation is handled by the host.",
                    false,
                ))
            }
            C::StartAgentTask { .. } => Err(DesktopServiceError::new(
                "invalid-payload",
                "Starting a task is handled by the host.",
                false,
            )),
            C::ReadAgentFiles { .. }
            | C::SaveAgentMemory { .. }
            | C::OverwriteAgentInstructionsFile { .. } => Ok(CommandExtras {
                files: Some(agent_files(store, command)?),
                ..CommandExtras::default()
            }),
            C::ReadAgentDecisions { agent_id, limit } => {
                let decisions = self
                    .agent_decisions
                    .lock()
                    .map_err(|_| unavailable("Agent decision log lock is unavailable."))?
                    .as_ref()
                    .map(|log| log.read(&agent_id, usize::from(limit)))
                    .unwrap_or_default();
                Ok(CommandExtras {
                    decisions: Some(decisions),
                    ..CommandExtras::default()
                })
            }
            C::RecreateAgentHome { agent_id } => {
                let profile = profile_of(store, &agent_id)?;
                let (home, sha) = agent_home::recreate_home(root, &profile, now)
                    .map_err(|error| home_error(&error))?;
                store
                    .set_home(&agent_id, &home, &sha)
                    .map_err(|error| refused(&error))?;
                Ok(CommandExtras::default())
            }
            C::RevealAgentHome { agent_id } => {
                let profile = profile_of(store, &agent_id)?;
                // Revealed by the caller once the store lock is released.
                Ok(CommandExtras {
                    reveal: Some(ensure_agent_home(store, root, &profile)?),
                    ..CommandExtras::default()
                })
            }
            C::SaveAgentFolders {
                expected_revision,
                agent_id,
                folders,
            } => {
                let profile = profile_of(store, &agent_id)?;
                let folders = folders
                    .iter()
                    .map(|folder| self.grantable_folder(folder, &profile))
                    .collect::<Result<Vec<_>, _>>()?;
                let command = C::SaveAgentFolders {
                    expected_revision,
                    agent_id,
                    folders,
                };
                store
                    .command(command, now)
                    .map_err(|error| refused(&error))?;
                regenerate_instructions(store, &profile.id)?;
                Ok(CommandExtras::default())
            }
            C::SaveAgent { .. } => {
                save_agent(store, command, now)?;
                Ok(CommandExtras::default())
            }
            C::SaveRoutine {
                expected_revision,
                mut routine,
            } => {
                if store.revision() != expected_revision {
                    return Err(unavailable(
                        "Agent workspace changed. Reload before saving the routine.",
                    ));
                }
                let profile = profile_of(store, &routine.agent_id)?;
                if routine.enabled && store.work.calendar(&routine.id).is_some() {
                    let ids: Vec<_> = store
                        .work
                        .tasks_of(&routine.agent_id)
                        .filter(|t| {
                            t.verification
                                == interface_contracts::agent_work::AgentVerificationState::Verified
                        })
                        .map(|t| t.id.clone())
                        .collect();
                    for id in ids {
                        super::agent_work::verify(store, &routine.agent_id, &id)?;
                    }
                }
                let home = ensure_agent_home(store, root, &profile)?;
                if routine.cwd.trim().is_empty() {
                    routine.cwd = home;
                }
                store
                    .command(
                        C::SaveRoutine {
                            expected_revision: store.revision(),
                            routine,
                        },
                        now,
                    )
                    .map_err(|error| refused(&error))?;
                Ok(CommandExtras::default())
            }
            C::SavePermissionMode { agent_id, mode } => {
                store
                    .save_permission_mode(&agent_id, mode)
                    .map_err(|error| refused(&error))?;
                Ok(CommandExtras::default())
            }
            C::RemoveAgent { ref id, .. } => {
                let id = id.clone();
                store
                    .command(command, now)
                    .map_err(|error| refused(&error))?;
                self.pending_approvals().retain(|p| p.agent_id != id);
                Ok(CommandExtras::default())
            }
            other => {
                store.command(other, now).map_err(|error| refused(&error))?;
                Ok(CommandExtras::default())
            }
        }
    }
    /// Canonical form of a folder the owner grants an agent: it must pass
    /// [`Self::is_grantable_root`] and not be the agent's own home.
    fn grantable_folder(
        &self,
        folder: &str,
        profile: &AgentProfile,
    ) -> Result<String, DesktopServiceError> {
        let refuse = |detail: &str| {
            DesktopServiceError::new(
                "agent-state-refused",
                format!("{folder} can't be granted to an agent: {detail}"),
                false,
            )
        };
        if !Path::new(folder).is_absolute() {
            return Err(refuse("use an absolute folder."));
        }
        let canonical =
            std::fs::canonicalize(folder).map_err(|_| refuse("it is missing or unreadable."))?;
        if !canonical.is_dir() {
            return Err(refuse("it is not a folder."));
        }
        let canonical_str = canonical
            .clone()
            .into_os_string()
            .into_string()
            .map_err(|_| refuse("its path is not valid UTF-8."))?;
        if interface_workspace::is_credential_path(&canonical_str) {
            return Err(refuse("it holds credentials."));
        }
        if !self.is_grantable_root(&canonical)
            || profile
                .home
                .as_deref()
                .is_some_and(|home| canonical == resolved(Path::new(home)))
        {
            return Err(refuse(
                "choose a project folder, not a system folder, your home folder or its parents, or an agents folder.",
            ));
        }
        Ok(canonical_str)
    }
}
