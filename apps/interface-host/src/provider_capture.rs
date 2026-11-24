pub(crate) mod accounts;
mod claude_registry;
mod generation;
pub(crate) mod native;
mod process_probe;
mod receipts;

use std::{
    fs,
    path::{Component, Path, PathBuf},
    thread,
    time::Duration,
};

use engine_claude::session_id_from_transcript_path;
use interface_common::uuid::canonical_uuid_v4;
use interface_contracts::terminal::{
    TERMINAL_LAUNCH_SESSION_VERSION, TerminalGeneration, TerminalId, TerminalLaunchProfileId,
    TerminalLaunchSessionV1,
};
use interface_storage::{
    FileSnapshot, JsonLimits, PrivateDirectory, WriterLock, atomic_write_json, read_bounded,
    read_json, snapshot_target,
};
use interface_terminal::{
    TerminalBackendError, TerminalProcessIdentity, TerminalProviderSession,
    TerminalProviderSessionCaptureOwner, TerminalProviderSessionCaptureProcess,
    TerminalProviderSessionLookup, TerminalProviderSessionTransition,
    captured_process_group_is_live, settle_stopped_orphan_process_group,
};
use serde::{Deserialize, Serialize};

use crate::bounded_process::{Capture, CapturedOutput, run_bounded};
use crate::capture_generation::{
    CAPTURE_CLAIMS_SEALED, CAPTURE_OWNER_FILE, GenerationEntry, MAX_CAPTURE_GENERATION_ENTRIES,
    MAX_CAPTURE_GENERATIONS, MAX_CAPTURE_OWNER_BYTES, MAX_RECEIPT_BYTES, Marker, RECEIPT_ROOT_LOCK,
    RECEIPT_SEALED, RECEIPT_WRITER_LOCK, acquire_root_lock, acquire_root_lock_for_finalization,
    acquire_writer_lock, acquire_writer_lock_for_finalization, classify, create_private_empty_file,
    read_marker, receipt_file_id,
};
#[cfg(test)]
use crate::capture_generation::{RECEIPT_TEMP_PREFIX, RECEIPT_TEMP_SUFFIX, pending_marker_name};
use crate::capture_hook::transition_for;
use crate::claude_terminal_hook::{
    ClaudeTerminalSessionReceipt, RECEIPT_VERSION, hook_process_identity, transcript_session_id,
};
use crate::codex_terminal_hook::read_codex_receipts;
use crate::opencode_terminal_session::read_opencode_receipt;
use claude_registry::{
    assert_owned_directory, read_current_session_with, snapshot_owned_directory,
};
use generation::{
    generation_has_empty_marker, generation_receipts_are_checkpointed,
    lock_claims_sealed_generation, lock_reusable_generation, prune_completed_receipts_except,
    remove_generation_receipts,
};
#[cfg(target_os = "macos")]
use process_probe::valid_terminal_device;
#[cfg(test)]
use process_probe::{bounded_process_observation, validate_live_process_output};
use process_probe::{
    normalize_whitespace, validate_live_process, validate_reaped_process_identity,
};
#[cfg(test)]
use receipts::claude_project_dir_name;
use receipts::{read_final_verified_receipt, read_verified_receipt, scan_receipts};

const STARTUP_ACTIVE_SETTLE_CHECKS: usize = 40;
const STARTUP_ACTIVE_SETTLE_DELAY: Duration = Duration::from_millis(50);
const LEGACY_CAPTURE_OWNER_VERSION: u32 = 1;
const CAPTURE_OWNER_VERSION: u32 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCaptureOwner {
    version: u32,
    capture_token: String,
    terminal_id: TerminalId,
    terminal_generation: TerminalGeneration,
    root: String,
    cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior_launch_session: Option<TerminalLaunchSessionV1>,
    #[serde(default)]
    nested_shell: bool,
    /// A trusted wrapper replaced the previous nested process before the
    /// service could file which public predecessor applies. Startup uses this
    /// bit to distinguish that handoff from a completed shell demotion.
    #[serde(default)]
    nested_replacement: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process: Option<StoredProcessIdentity>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredProcessIdentity {
    process_id: u32,
    process_group_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_device: Option<String>,
    process_start: String,
}

impl From<&TerminalProcessIdentity> for StoredProcessIdentity {
    fn from(process: &TerminalProcessIdentity) -> Self {
        Self {
            process_id: process.process_id,
            process_group_id: process.process_group_id,
            terminal_device: process.terminal_device.clone(),
            process_start: process.process_start.clone(),
        }
    }
}

impl From<&StoredProcessIdentity> for TerminalProcessIdentity {
    fn from(process: &StoredProcessIdentity) -> Self {
        Self {
            process_id: process.process_id,
            process_group_id: process.process_group_id,
            terminal_device: process.terminal_device.clone(),
            process_start: process.process_start.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupReceiptDisposition {
    Durable,
    DefinitelyUnsaved,
    Retryable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecoveredClaudeTerminalSession {
    pub(crate) terminal_id: TerminalId,
    pub(crate) terminal_generation: TerminalGeneration,
    pub(crate) root: String,
    pub(crate) cwd: String,
    pub(crate) prior_launch_session: Option<TerminalLaunchSessionV1>,
    pub(crate) launch_session: TerminalLaunchSessionV1,
    /// The provider whose receipt was recovered; the owner record does not
    /// name it, so the receipt that verified decides.
    pub(crate) profile_id: TerminalLaunchProfileId,
    pub(crate) nested_shell: bool,
    pub(crate) nested_replacement: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct StartupReceiptSweep {
    pub(crate) examined: usize,
    pub(crate) retired: usize,
    pub(crate) active: usize,
    pub(crate) retryable: usize,
    pub(crate) recovered_orphans: usize,
}

/// Corroborates the provider session a terminal pane captured (Claude, Codex
/// or `OpenCode`) against the exact spawned process and the provider's durable
/// store.
///
/// Claude's PID sidecar proves the live process generation but never selects
/// the durable UUID. That identity is derived from the exact transcript path
/// captured by Interface's private hook. The renderer cannot select the
/// receipt root, provider store, or process identity.
pub(crate) struct ProviderCaptureLookup {
    config: PathBuf,
    sessions: PathBuf,
    projects: PathBuf,
    receipts: PathBuf,
}

impl ProviderCaptureLookup {
    /// Derives Claude's sibling `sessions` directory from its configured
    /// `projects` root.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured root is not an absolute path with
    /// a parent directory.
    pub(crate) fn new(
        projects_root: &Path,
        receipts_root: &Path,
    ) -> Result<Self, TerminalBackendError> {
        if !projects_root.is_absolute() || !receipts_root.is_absolute() {
            return Err(invalid_registry());
        }
        let config_root = projects_root
            .parent()
            .filter(|parent| parent.is_absolute())
            .ok_or_else(invalid_registry)?
            .to_path_buf();
        let sessions_root = config_root.join("sessions");
        PrivateDirectory::open(receipts_root).map_err(|_| invalid_registry())?;
        // Claude creates and removes provider-owned registry directories
        // independently of Interface. Missing paths are therefore a
        // normal id-less lookup state, not an application-startup error;
        // every actual read snapshots and revalidates them instead.
        Ok(Self {
            config: config_root,
            sessions: sessions_root,
            projects: projects_root.to_path_buf(),
            receipts: receipts_root.to_path_buf(),
        })
    }

    fn current_session_with<F>(
        &self,
        process: &TerminalProcessIdentity,
        capture_token: &str,
        validate_process: F,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
    where
        F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    {
        if !canonical_uuid_v4(capture_token) {
            return Err(invalid_registry());
        }
        let Some(config_identity) = snapshot_owned_directory(&self.config)? else {
            return Ok(None);
        };
        let Some(sessions_identity) = snapshot_owned_directory(&self.sessions)? else {
            return Ok(None);
        };
        let Some(process_record) =
            read_current_session_with(&self.sessions, process, &validate_process)?
        else {
            return Ok(None);
        };
        let owner = verified_owner(&self.receipts, capture_token, process, false)?;
        assert_owned_directory(&self.config, config_identity)?;
        assert_owned_directory(&self.sessions, sessions_identity)?;
        let session = read_verified_receipt(
            &self.projects,
            &self.receipts,
            capture_token,
            process,
            Some(&process_record),
            owner.as_ref().map(|owner| Path::new(&owner.cwd)),
        )?;
        validate_process(process)?;
        Ok(session)
    }

    /// Codex's receipt is appended by its hook under the generation writer
    /// lock, so reading under that same lock sees whole files only. The
    /// reporting process must be this exact spawned pane process.
    fn codex_session_with<F>(
        &self,
        process: &TerminalProcessIdentity,
        capture_token: &str,
        validate_process: F,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
    where
        F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    {
        if !canonical_uuid_v4(capture_token) {
            return Err(invalid_registry());
        }
        verified_owner(&self.receipts, capture_token, process, false)?;
        let root = PrivateDirectory::open(&self.receipts).map_err(|_| invalid_registry())?;
        let Ok(generation) = root.open_child(capture_token) else {
            return Ok(None);
        };
        let writer = acquire_writer_lock(&generation).map_err(|_| invalid_registry())?;
        let session = read_codex_receipts(&generation, capture_token, process);
        let release = writer.release(&generation).map_err(|_| invalid_registry());
        let session = session?;
        release?;
        validate_process(process)?;
        Ok(session)
    }

    /// An armed login shell is polled as Claude, but its wrapper may have run
    /// Codex instead; the generation then holds a Codex receipt from that
    /// exact process. Direct Claude panes never take this path.
    fn manual_codex_session_with<F>(
        &self,
        process: &TerminalProcessIdentity,
        capture_token: &str,
        validate_process: F,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
    where
        F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    {
        if !capture_owner(&self.receipts, capture_token)?.is_some_and(|owner| owner.nested_shell) {
            return Ok(None);
        }
        self.codex_session_with(process, capture_token, validate_process)
    }

    /// Read one dead nested process's stable receipt without sealing the
    /// retained shell's reusable capture generation.
    fn settled_session_with<P>(
        &self,
        process: &TerminalProcessIdentity,
        capture_token: &str,
        process_is_gone: P,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
    where
        P: Fn(&StoredProcessIdentity) -> Result<bool, ()>,
    {
        validate_reaped_process_identity(process)?;
        let stored_process = StoredProcessIdentity::from(process);
        if process_is_gone(&stored_process) != Ok(true) {
            return Err(invalid_registry());
        }
        let Some(locked) = lock_reusable_generation(&self.receipts, capture_token)? else {
            return Ok(None);
        };
        locked.run(|generation, _| {
            let owner =
                read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
            if !owner.nested_shell || owner.process.as_ref() != Some(&stored_process) {
                return Err(invalid_registry());
            }
            let session = read_verified_receipt(
                &self.projects,
                &self.receipts,
                capture_token,
                process,
                None,
                Some(Path::new(&owner.cwd)),
            )?;
            if process_is_gone(&stored_process) != Ok(true) {
                return Err(invalid_registry());
            }
            Ok(session)
        })
    }

    /// Reconcile crash-surviving capture generations before any replacement
    /// terminal can spawn.
    ///
    /// The callback owns the final authority/roster decision. Only a durable
    /// acknowledgement or a definitive unsaved classification retires the
    /// receipt; every ambiguous outcome remains sealed and retryable.
    pub(crate) fn reconcile_startup_receipts<F>(
        &self,
        reconcile: F,
    ) -> Result<StartupReceiptSweep, TerminalBackendError>
    where
        F: FnMut(&RecoveredClaudeTerminalSession) -> StartupReceiptDisposition,
    {
        self.reconcile_startup_receipts_with_recovery(
            reconcile,
            capture_process_group_is_gone,
            |process| {
                settle_stopped_orphan_process_group(&TerminalProcessIdentity::from(process))
                    .map_err(|_| ())
            },
            STARTUP_ACTIVE_SETTLE_DELAY,
        )
    }

    #[cfg(test)]
    fn reconcile_startup_receipts_with<F, P>(
        &self,
        reconcile: F,
        process_is_gone: P,
        active_settle_delay: Duration,
    ) -> Result<StartupReceiptSweep, TerminalBackendError>
    where
        F: FnMut(&RecoveredClaudeTerminalSession) -> StartupReceiptDisposition,
        P: FnMut(&StoredProcessIdentity) -> Result<bool, ()>,
    {
        self.reconcile_startup_receipts_with_recovery(
            reconcile,
            process_is_gone,
            |_| Ok(false),
            active_settle_delay,
        )
    }

    #[allow(clippy::too_many_lines)] // One bounded sweep keeps classification and retirement decisions together.
    fn reconcile_startup_receipts_with_recovery<F, P, R>(
        &self,
        mut reconcile: F,
        mut process_is_gone: P,
        mut recover_orphan: R,
        active_settle_delay: Duration,
    ) -> Result<StartupReceiptSweep, TerminalBackendError>
    where
        F: FnMut(&RecoveredClaudeTerminalSession) -> StartupReceiptDisposition,
        P: FnMut(&StoredProcessIdentity) -> Result<bool, ()>,
        R: FnMut(&StoredProcessIdentity) -> Result<bool, ()>,
    {
        let root = PrivateDirectory::open(&self.receipts).map_err(|_| invalid_registry())?;
        let registration = acquire_root_lock(&root).map_err(|_| invalid_registry())?;
        let generations = bounded_generation_names(&root);
        let release = registration.release(&root).map_err(|_| invalid_registry());
        let generations = generations?;
        release?;

        let mut sweep = StartupReceiptSweep::default();
        let mut recoveries = Vec::<(
            String,
            StoredCaptureOwner,
            TerminalProviderSession,
            TerminalLaunchProfileId,
        )>::new();
        let mut process_candidates = Vec::<(String, StoredCaptureOwner)>::new();
        for capture_token in generations {
            sweep.examined = sweep.examined.saturating_add(1);
            let owner = match capture_owner(&self.receipts, &capture_token) {
                Ok(Some(owner)) => owner,
                Ok(None) => {
                    match remove_generation_receipts(&self.receipts, &capture_token) {
                        Ok(()) => sweep.retired = sweep.retired.saturating_add(1),
                        Err(_) => sweep.retryable = sweep.retryable.saturating_add(1),
                    }
                    continue;
                }
                Err(_) => {
                    sweep.retryable = sweep.retryable.saturating_add(1);
                    continue;
                }
            };
            if owner.process.is_none() {
                // A crash between spawn and `record_process` leaves no
                // identity with which any receipt can be authenticated. The
                // generation is therefore unusable evidence regardless of
                // whether a hook managed to commit bytes. Retire it through
                // the normal locked finalization path so it cannot block a
                // fresh capture for this stable terminal ID forever.
                match remove_generation_receipts(&self.receipts, &capture_token) {
                    Ok(()) => sweep.retired = sweep.retired.saturating_add(1),
                    Err(_) => sweep.retryable = sweep.retryable.saturating_add(1),
                }
                continue;
            }
            process_candidates.push((capture_token, owner));
        }

        // Probe all active captures once per round. The delay/round budget is
        // global, so startup remains bounded, but no long-lived generation can
        // consume it before a later just-exiting process gets rechecked.
        let mut dead_processes = Vec::<(String, StoredCaptureOwner)>::new();
        for round in 0..=STARTUP_ACTIVE_SETTLE_CHECKS {
            let mut still_active = Vec::new();
            for (capture_token, owner) in process_candidates {
                let stored_process = owner.process.as_ref().expect("candidate has a process");
                match process_is_gone(stored_process) {
                    Ok(true) => dead_processes.push((capture_token, owner)),
                    Ok(false) if round == 0 => {
                        // A stopped orphan cannot finish on its own. Recheck
                        // its private owner and settle it before spending the
                        // startup grace window on genuinely supervised groups.
                        if capture_owner(&self.receipts, &capture_token)
                            .ok()
                            .flatten()
                            .as_ref()
                            != Some(&owner)
                        {
                            sweep.retryable = sweep.retryable.saturating_add(1);
                            continue;
                        }
                        match recover_orphan(stored_process) {
                            Ok(true) => {
                                sweep.recovered_orphans = sweep.recovered_orphans.saturating_add(1);
                                dead_processes.push((capture_token, owner));
                            }
                            Ok(false) => still_active.push((capture_token, owner)),
                            Err(()) => sweep.retryable = sweep.retryable.saturating_add(1),
                        }
                    }
                    Ok(false) => still_active.push((capture_token, owner)),
                    Err(()) => sweep.retryable = sweep.retryable.saturating_add(1),
                }
            }
            process_candidates = still_active;
            if process_candidates.is_empty() || round == STARTUP_ACTIVE_SETTLE_CHECKS {
                break;
            }
            thread::sleep(active_settle_delay);
        }
        sweep.active = sweep.active.saturating_add(process_candidates.len());

        for (capture_token, owner) in dead_processes {
            let stored_process = owner.process.as_ref().expect("candidate has a process");
            let process = TerminalProcessIdentity::from(stored_process);
            let session = read_final_verified_receipt(
                &self.projects,
                &self.receipts,
                &capture_token,
                &process,
                Some(Path::new(&owner.cwd)),
            )
            .and_then(|session| match session {
                Some(session) => Ok(Some((session, TerminalLaunchProfileId::Claude))),
                // The owner record does not name its provider; a generation
                // without a Claude receipt may still hold a Codex one.
                None => match self.codex_session_with(&process, &capture_token, |_| Ok(()))? {
                    Some(session) => Ok(Some((session, TerminalLaunchProfileId::Codex))),
                    None => {
                        match self.opencode_session_with(&process, &capture_token, |_| Ok(()))? {
                            Some(session) => Ok(Some((session, TerminalLaunchProfileId::Opencode))),
                            None => self.native_session_with(&process, &capture_token, |_| Ok(())),
                        }
                    }
                },
            });
            match session {
                Ok(Some((session, profile_id))) => {
                    recoveries.push((capture_token, owner, session, profile_id));
                }
                Ok(None) => match remove_generation_receipts(&self.receipts, &capture_token) {
                    Ok(()) => sweep.retired = sweep.retired.saturating_add(1),
                    Err(_) => sweep.retryable = sweep.retryable.saturating_add(1),
                },
                Err(_) => sweep.retryable = sweep.retryable.saturating_add(1),
            }
        }

        recoveries.sort_by(|left, right| {
            (
                left.1.terminal_id,
                left.1.terminal_generation,
                left.0.as_str(),
            )
                .cmp(&(
                    right.1.terminal_id,
                    right.1.terminal_generation,
                    right.0.as_str(),
                ))
        });
        for (capture_token, owner, session, profile_id) in &recoveries {
            let ambiguous =
                recoveries
                    .iter()
                    .any(|(other_token, other_owner, other_session, _)| {
                        other_token != capture_token
                            && other_owner.terminal_id == owner.terminal_id
                            && other_owner.prior_launch_session == owner.prior_launch_session
                            && other_session.id != session.id
                    });
            if ambiguous {
                sweep.retryable = sweep.retryable.saturating_add(1);
                continue;
            }
            let recovered = RecoveredClaudeTerminalSession {
                terminal_id: owner.terminal_id,
                terminal_generation: owner.terminal_generation,
                root: owner.root.clone(),
                cwd: owner.cwd.clone(),
                prior_launch_session: owner.prior_launch_session.clone(),
                launch_session: TerminalLaunchSessionV1 {
                    version: TERMINAL_LAUNCH_SESSION_VERSION,
                    id: Some(session.id.clone()),
                    resume: true,
                },
                profile_id: *profile_id,
                nested_shell: owner.nested_shell,
                nested_replacement: owner.nested_replacement,
            };
            match reconcile(&recovered) {
                StartupReceiptDisposition::Durable
                | StartupReceiptDisposition::DefinitelyUnsaved => {
                    match remove_generation_receipts(&self.receipts, capture_token) {
                        Ok(()) => sweep.retired = sweep.retired.saturating_add(1),
                        Err(_) => sweep.retryable = sweep.retryable.saturating_add(1),
                    }
                }
                StartupReceiptDisposition::Retryable => {
                    sweep.retryable = sweep.retryable.saturating_add(1);
                }
            }
        }
        Ok(sweep)
    }
}

impl TerminalProviderSessionLookup for ProviderCaptureLookup {
    fn begin_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        if !captured_profile(profile_id) {
            return Ok(());
        }
        register_capture_owner(&self.receipts, owner)
    }

    fn record_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        if !captured_profile(profile_id) {
            return Ok(());
        }
        record_capture_process(&self.receipts, capture_token, process)
    }

    fn capture_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        Ok(capture_owner(&self.receipts, capture_token)?
            .filter(|owner| owner.nested_shell)
            .and_then(|owner| {
                owner
                    .process
                    .map(|process| TerminalProviderSessionCaptureProcess {
                        process: TerminalProcessIdentity::from(&process),
                        cwd: PathBuf::from(owner.cwd),
                    })
            }))
    }

    fn seal_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        let locked = lock_claims_sealed_generation(&self.receipts, capture_token)?
            .ok_or_else(invalid_registry)?;
        locked.run(|generation, _| {
            let owner =
                read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
            if !owner.nested_shell {
                return Err(invalid_registry());
            }
            Ok(owner
                .process
                .map(|process| TerminalProviderSessionCaptureProcess {
                    process: TerminalProcessIdentity::from(&process),
                    cwd: PathBuf::from(owner.cwd),
                }))
        })
    }

    fn replace_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        previous: &TerminalProcessIdentity,
        replacement: &TerminalProcessIdentity,
        cwd: &Path,
        prior_launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        replace_nested_capture_process(
            &self.config,
            &self.sessions,
            &self.projects,
            &self.receipts,
            capture_token,
            previous,
            replacement,
            cwd,
            prior_launch_session,
        )
    }

    fn rearm_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
        cwd: &Path,
    ) -> Result<(), TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(());
        }
        rearm_nested_capture_process(&self.receipts, capture_token, process, cwd)
    }

    fn checkpoint_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        launch_session: &TerminalLaunchSessionV1,
    ) -> Result<(), TerminalBackendError> {
        if !captured_profile(profile_id) {
            return Ok(());
        }
        checkpoint_capture_session(&self.receipts, capture_token, launch_session)
    }

    fn current_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if interface_terminal::native_checkpoint::supports(profile_id) {
            return self
                .native_session_with(process, capture_token, validate_live_process)
                .map(|value| {
                    value
                        .filter(|(_, profile)| *profile == profile_id)
                        .map(|(session, _)| session)
                });
        }
        if profile_id == TerminalLaunchProfileId::Codex {
            return self.codex_session_with(process, capture_token, validate_live_process);
        }
        if profile_id == TerminalLaunchProfileId::Opencode {
            return self.opencode_session_with(process, capture_token, validate_live_process);
        }
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        match self.current_session_with(process, capture_token, validate_live_process)? {
            Some(session) => Ok(Some(session)),
            None => self.manual_codex_session_with(process, capture_token, validate_live_process),
        }
    }

    fn settled_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        match self.settled_session_with(process, capture_token, capture_process_group_is_gone)? {
            Some(session) => Ok(Some(session)),
            None => self.manual_codex_session_with(process, capture_token, |process| {
                if capture_process_group_is_gone(&StoredProcessIdentity::from(process)) == Ok(true)
                {
                    Ok(())
                } else {
                    Err(invalid_registry())
                }
            }),
        }
    }

    fn final_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        if interface_terminal::native_checkpoint::supports(profile_id) {
            validate_reaped_process_identity(process)?;
            return self
                .native_session_with(process, capture_token, |_| Ok(()))
                .map(|value| {
                    value
                        .filter(|(_, profile)| *profile == profile_id)
                        .map(|(session, _)| session)
                });
        }
        if profile_id == TerminalLaunchProfileId::Codex {
            validate_reaped_process_identity(process)?;
            return self.codex_session_with(process, capture_token, |_| Ok(()));
        }
        if profile_id == TerminalLaunchProfileId::Opencode {
            // The pane-local observation outlives its process; shutdown still
            // validates the exact reaped process before saving its final ID.
            validate_reaped_process_identity(process)?;
            return self.opencode_session_with(process, capture_token, |_| Ok(()));
        }
        if profile_id != TerminalLaunchProfileId::Claude {
            return Ok(None);
        }
        validate_reaped_process_identity(process)?;
        if capture_process_group_is_gone(&StoredProcessIdentity::from(process)) != Ok(true) {
            return Err(invalid_registry());
        }
        let owner = verified_owner(&self.receipts, capture_token, process, false)?;
        match read_final_verified_receipt(
            &self.projects,
            &self.receipts,
            capture_token,
            process,
            owner.as_ref().map(|owner| Path::new(&owner.cwd)),
        )? {
            Some(session) => Ok(Some(session)),
            None => self.manual_codex_session_with(process, capture_token, |_| Ok(())),
        }
    }

    fn end_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        if !captured_profile(profile_id) {
            return Ok(());
        }
        remove_generation_receipts(&self.receipts, capture_token)
    }
}

impl ProviderCaptureLookup {
    /// Read only this pane generation's provider observation. Shared log
    /// timestamps and other panes' session creations never select its chat.
    fn opencode_session_with<F>(
        &self,
        process: &TerminalProcessIdentity,
        capture_token: &str,
        validate_process: F,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
    where
        F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    {
        if !canonical_uuid_v4(capture_token) {
            return Err(invalid_registry());
        }
        let owner = verified_owner(&self.receipts, capture_token, process, true)?
            .ok_or_else(invalid_registry)?;
        validate_process(process)?;
        let root = PrivateDirectory::open(&self.receipts).map_err(|_| invalid_registry())?;
        let generation = root
            .open_child(capture_token)
            .map_err(|_| invalid_registry())?;
        let session =
            read_opencode_receipt(&generation, capture_token, process, Path::new(&owner.cwd))?;
        validate_process(process)?;
        Ok(session)
    }
}

fn register_capture_owner(
    receipts_root: &Path,
    owner: &TerminalProviderSessionCaptureOwner,
) -> Result<(), TerminalBackendError> {
    let stored = StoredCaptureOwner {
        version: CAPTURE_OWNER_VERSION,
        capture_token: owner.capture_token.clone(),
        terminal_id: owner.terminal_id,
        terminal_generation: owner.terminal_generation,
        root: owner.root.clone(),
        cwd: owner.cwd.clone(),
        prior_launch_session: owner.prior_launch_session.clone(),
        nested_shell: owner.nested_shell,
        nested_replacement: false,
        process: None,
    };
    validate_capture_owner(&stored, &owner.capture_token)?;

    let root = PrivateDirectory::open(receipts_root).map_err(|_| invalid_registry())?;
    let registration = acquire_root_lock(&root).map_err(|_| invalid_registry())?;
    let result = (|| {
        let generations = bounded_generation_names(&root)?;
        if generations
            .iter()
            .any(|generation| generation == &owner.capture_token)
            || generations.len() >= MAX_CAPTURE_GENERATIONS
        {
            return Err(invalid_registry());
        }
        for capture_token in &generations {
            let generation = root
                .open_child(capture_token)
                .map_err(|_| invalid_registry())?;
            if read_capture_owner(&generation, capture_token)?
                .is_some_and(|current| current.terminal_id == stored.terminal_id)
            {
                // One unresolved capture owner per stable terminal ID. A
                // still-active or retryable predecessor must be reconciled,
                // not hidden behind a second Claude process for the same tab.
                return Err(invalid_registry());
            }
        }
        let generation = root
            .open_or_create_child(&owner.capture_token)
            .map_err(|_| invalid_registry())?;
        // Keep the root registration gate until the owner record is durable.
        // Otherwise a concurrent begin could observe this ownerless directory
        // and create a second generation for the same terminal ID.
        let writer = acquire_writer_lock(&generation).map_err(|_| invalid_registry())?;
        let write = write_new_capture_owner(&generation, &writer, &stored);
        let release = writer.release(&generation).map_err(|_| invalid_registry());
        write?;
        release
    })();
    let release = registration.release(&root).map_err(|_| invalid_registry());
    result?;
    release?;
    Ok(())
}

fn record_capture_process(
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
) -> Result<(), TerminalBackendError> {
    if !canonical_uuid_v4(capture_token) {
        return Err(invalid_registry());
    }
    let root = PrivateDirectory::open(receipts_root).map_err(|_| invalid_registry())?;
    let generation = root
        .open_child(capture_token)
        .map_err(|_| invalid_registry())?;
    let writer = acquire_writer_lock(&generation).map_err(|_| invalid_registry())?;
    let result = (|| {
        if generation_has_empty_marker(&generation, RECEIPT_SEALED)? {
            return Err(invalid_registry());
        }
        let claims_sealed = generation_has_empty_marker(&generation, CAPTURE_CLAIMS_SEALED)?;
        let mut owner =
            read_capture_owner(&generation, capture_token)?.ok_or_else(invalid_registry)?;
        let stored_process = StoredProcessIdentity::from(process);
        validate_process_owner(&stored_process)?;
        match owner.process.as_ref() {
            Some(current) if current == &stored_process => return Ok(()),
            Some(_) => return Err(invalid_registry()),
            None => {
                if claims_sealed {
                    return Err(invalid_registry());
                }
                owner.process = Some(stored_process);
                owner.nested_replacement = false;
            }
        }
        write_capture_owner(&generation, &writer, &owner)
    })();
    let release = writer.release(&generation).map_err(|_| invalid_registry());
    result?;
    release
}

#[allow(clippy::too_many_arguments)] // Every path and identity is an independent trust boundary.
fn replace_nested_capture_process(
    config_root: &Path,
    sessions_root: &Path,
    projects_root: &Path,
    receipts_root: &Path,
    capture_token: &str,
    previous: &TerminalProcessIdentity,
    replacement: &TerminalProcessIdentity,
    cwd: &Path,
    prior_launch_session: Option<&TerminalLaunchSessionV1>,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
    replace_nested_capture_process_with(
        config_root,
        sessions_root,
        projects_root,
        receipts_root,
        capture_token,
        previous,
        replacement,
        cwd,
        prior_launch_session,
        validate_live_process,
        capture_process_group_is_gone,
    )
}

#[allow(clippy::too_many_arguments)]
fn replace_nested_capture_process_with<F, P>(
    config_root: &Path,
    sessions_root: &Path,
    projects_root: &Path,
    receipts_root: &Path,
    capture_token: &str,
    previous: &TerminalProcessIdentity,
    replacement: &TerminalProcessIdentity,
    cwd: &Path,
    prior_launch_session: Option<&TerminalLaunchSessionV1>,
    validate_process: F,
    process_is_gone: P,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
where
    F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    P: Fn(&StoredProcessIdentity) -> Result<bool, ()>,
{
    if previous == replacement {
        return Err(invalid_registry());
    }
    validate_process_owner(&StoredProcessIdentity::from(replacement))?;
    validate_process(replacement)?;
    let Some(config_identity) = snapshot_owned_directory(config_root)? else {
        return Ok(None);
    };
    let Some(sessions_identity) = snapshot_owned_directory(sessions_root)? else {
        return Ok(None);
    };
    let Some(reported_session_id) =
        read_current_session_with(sessions_root, replacement, &validate_process)?
    else {
        return replace_nested_codex_process(
            receipts_root,
            capture_token,
            previous,
            replacement,
            cwd,
            prior_launch_session,
            &process_is_gone,
        );
    };
    let Some(locked) = lock_reusable_generation(receipts_root, capture_token)? else {
        return Ok(None);
    };
    locked.run(|generation, writer| {
        let mut owner =
            read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
        let stored_previous = StoredProcessIdentity::from(previous);
        let stored_replacement = StoredProcessIdentity::from(replacement);
        if !owner.nested_shell
            || (owner.process.as_ref() != Some(&stored_previous)
                && owner.process.as_ref() != Some(&stored_replacement))
        {
            return Err(invalid_registry());
        }
        if process_is_gone(&stored_previous) != Ok(true) {
            return Err(invalid_registry());
        }
        let canonical_cwd = canonical_nested_cwd(&owner, cwd)?;
        let scan = scan_receipts(generation, capture_token, projects_root, replacement)?;
        if scan.pending_write {
            return Ok(None);
        }
        let Some(candidate) = scan.candidate else {
            return Ok(None);
        };
        if candidate.reported_session_id != reported_session_id {
            return Err(invalid_registry());
        }
        let transcript_id =
            session_id_from_transcript_path(projects_root, &candidate.transcript_path)
                .map_err(|_| invalid_registry())?;
        if transcript_id != candidate.transcript_id {
            return Err(invalid_registry());
        }
        let stable_receipt = read_bounded(generation, &candidate.name, MAX_RECEIPT_BYTES)
            .map_err(|_| invalid_registry())?
            .ok_or_else(invalid_registry)?;
        if stable_receipt.snapshot() != &candidate.snapshot {
            return Err(invalid_registry());
        }
        assert_owned_directory(config_root, config_identity)?;
        assert_owned_directory(sessions_root, sessions_identity)?;
        prune_completed_receipts_except(generation, capture_token, Some(&candidate.name))?;
        owner.cwd = owner_path(&canonical_cwd)?;
        owner.prior_launch_session = prior_launch_session.cloned();
        owner.process = Some(stored_replacement);
        owner.nested_replacement = false;
        write_capture_owner(generation, writer, &owner)?;
        Ok(Some(TerminalProviderSession {
            id: transcript_id,
            transition: candidate.transition,
        }))
    })
}

/// The Codex arm of [`replace_nested_capture_process_with`]: Codex has no
/// per-process session record, so the replacement is authenticated by the
/// receipt its own hook filed against the exact replacement process.
fn replace_nested_codex_process<P>(
    receipts_root: &Path,
    capture_token: &str,
    previous: &TerminalProcessIdentity,
    replacement: &TerminalProcessIdentity,
    cwd: &Path,
    prior_launch_session: Option<&TerminalLaunchSessionV1>,
    process_is_gone: P,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError>
where
    P: Fn(&StoredProcessIdentity) -> Result<bool, ()>,
{
    let Some(locked) = lock_reusable_generation(receipts_root, capture_token)? else {
        return Ok(None);
    };
    locked.run(|generation, writer| {
        let mut owner =
            read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
        let stored_previous = StoredProcessIdentity::from(previous);
        let stored_replacement = StoredProcessIdentity::from(replacement);
        if !owner.nested_shell
            || (owner.process.as_ref() != Some(&stored_previous)
                && owner.process.as_ref() != Some(&stored_replacement))
        {
            return Err(invalid_registry());
        }
        if process_is_gone(&stored_previous) != Ok(true) {
            return Err(invalid_registry());
        }
        let canonical_cwd = canonical_nested_cwd(&owner, cwd)?;
        let Some(session) = read_codex_receipts(generation, capture_token, replacement)? else {
            return Ok(None);
        };
        prune_completed_receipts_except(generation, capture_token, None)?;
        owner.cwd = owner_path(&canonical_cwd)?;
        owner.prior_launch_session = prior_launch_session.cloned();
        owner.process = Some(stored_replacement);
        owner.nested_replacement = false;
        write_capture_owner(generation, writer, &owner)?;
        Ok(Some(session))
    })
}

fn rearm_nested_capture_process(
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    cwd: &Path,
) -> Result<(), TerminalBackendError> {
    rearm_nested_capture_process_with(
        receipts_root,
        capture_token,
        process,
        cwd,
        capture_process_group_is_gone,
    )
}

fn rearm_nested_capture_process_with<P>(
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    cwd: &Path,
    process_is_gone: P,
) -> Result<(), TerminalBackendError>
where
    P: Fn(&StoredProcessIdentity) -> Result<bool, ()>,
{
    let Some(locked) = lock_reusable_generation(receipts_root, capture_token)? else {
        return Err(invalid_registry());
    };
    locked.run(|generation, writer| {
        let mut owner =
            read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
        let stored_process = StoredProcessIdentity::from(process);
        if !owner.nested_shell
            || owner.process.as_ref() != Some(&stored_process)
            || process_is_gone(&stored_process) != Ok(true)
            || !generation_receipts_are_checkpointed(
                generation,
                capture_token,
                &stored_process,
                Path::new(&owner.cwd),
                owner.prior_launch_session.as_ref(),
            )?
        {
            return Err(invalid_registry());
        }
        let canonical_cwd = canonical_nested_cwd(&owner, cwd)?;
        prune_completed_receipts_except(generation, capture_token, None)?;
        owner.cwd = owner_path(&canonical_cwd)?;
        owner.prior_launch_session = None;
        owner.process = None;
        owner.nested_replacement = false;
        write_capture_owner(generation, writer, &owner)
    })
}

/// Claim a retained-shell capture for the trusted wrapper's own exact process
/// before it execs Claude. This closes the otherwise unavoidable window in
/// which a new hook receipt could exist while the durable owner was still
/// processless or pointed at the prior invocation.
pub(crate) fn claim_manual_nested_capture_process(
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    cwd: &Path,
) -> Result<(), TerminalBackendError> {
    claim_manual_nested_capture_process_with(
        receipts_root,
        capture_token,
        process,
        cwd,
        validate_live_process,
        capture_process_group_is_gone,
    )
}

fn claim_manual_nested_capture_process_with<F, P>(
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    cwd: &Path,
    validate_process: F,
    process_is_gone: P,
) -> Result<(), TerminalBackendError>
where
    F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
    P: Fn(&StoredProcessIdentity) -> Result<bool, ()>,
{
    validate_process_owner(&StoredProcessIdentity::from(process))?;
    validate_process(process)?;
    let Some(locked) = lock_reusable_generation(receipts_root, capture_token)? else {
        return Err(invalid_registry());
    };
    locked.run(|generation, writer| {
        let mut owner =
            read_capture_owner(generation, capture_token)?.ok_or_else(invalid_registry)?;
        if !owner.nested_shell {
            return Err(invalid_registry());
        }
        let canonical_cwd = canonical_nested_cwd(&owner, cwd)?;
        let stored_process = StoredProcessIdentity::from(process);
        let replaced = match owner.process.as_ref() {
            Some(current) if current == &stored_process => {
                if Path::new(&owner.cwd) != canonical_cwd {
                    return Err(invalid_registry());
                }
                return Ok(());
            }
            Some(current) => {
                if process_is_gone(current) != Ok(true) {
                    return Err(invalid_registry());
                }
                true
            }
            None => false,
        };
        if replaced
            && !generation_receipts_are_checkpointed(
                generation,
                capture_token,
                owner.process.as_ref().expect("a replacement has an owner"),
                Path::new(&owner.cwd),
                owner.prior_launch_session.as_ref(),
            )?
        {
            // The prior wrapper may have exited before the terminal poll
            // durably filed its hook receipt. Refuse the new exec while that
            // receipt is still the only evidence of the prior session; a
            // later checkpointed retry can safely prune it.
            return Err(invalid_registry());
        }
        // The exclusive writer+registration gates prove that no old hook
        // can publish after this prune and before the new exact owner is
        // durable. Provider transcripts are outside this private tree and
        // are never touched.
        prune_completed_receipts_except(generation, capture_token, None)?;
        owner.cwd = owner_path(&canonical_cwd)?;
        owner.process = Some(stored_process);
        owner.nested_replacement = replaced;
        write_capture_owner(generation, writer, &owner)
    })
}

/// A canonical cwd as the capture owner file stores it; refused unless it is
/// a valid owner path.
fn owner_path(path: &Path) -> Result<String, TerminalBackendError> {
    path.to_str()
        .filter(|cwd| valid_owner_path(cwd))
        .map(str::to_owned)
        .ok_or_else(invalid_registry)
}

fn canonical_nested_cwd(
    owner: &StoredCaptureOwner,
    cwd: &Path,
) -> Result<PathBuf, TerminalBackendError> {
    let canonical = fs::canonicalize(cwd).map_err(|_| invalid_registry())?;
    if canonical != cwd || !canonical.is_dir() || !canonical.starts_with(Path::new(&owner.root)) {
        return Err(invalid_registry());
    }
    Ok(canonical)
}

fn checkpoint_capture_session(
    receipts_root: &Path,
    capture_token: &str,
    launch_session: &TerminalLaunchSessionV1,
) -> Result<(), TerminalBackendError> {
    if !canonical_uuid_v4(capture_token) || !valid_launch_session(launch_session) {
        return Err(invalid_registry());
    }
    let root = PrivateDirectory::open(receipts_root).map_err(|_| invalid_registry())?;
    let generation = root
        .open_child(capture_token)
        .map_err(|_| invalid_registry())?;
    let writer = acquire_writer_lock(&generation).map_err(|_| invalid_registry())?;
    let result = (|| {
        let mut owner =
            read_capture_owner(&generation, capture_token)?.ok_or_else(invalid_registry)?;
        if owner.process.is_none() {
            return Err(invalid_registry());
        }
        if owner.prior_launch_session.as_ref() == Some(launch_session) {
            return Ok(());
        }
        owner.prior_launch_session = Some(launch_session.clone());
        owner.nested_replacement = false;
        write_capture_owner(&generation, &writer, &owner)
    })();
    let release = writer.release(&generation).map_err(|_| invalid_registry());
    result?;
    release
}

fn bounded_generation_names(root: &PrivateDirectory) -> Result<Vec<String>, TerminalBackendError> {
    root.assert_unchanged().map_err(|_| invalid_registry())?;
    let mut generations = Vec::new();
    for entry in fs::read_dir(root.path()).map_err(|_| invalid_registry())? {
        let name = entry
            .map_err(|_| invalid_registry())?
            .file_name()
            .into_string()
            .map_err(|_| invalid_registry())?;
        if name == RECEIPT_ROOT_LOCK {
            let lock = read_bounded(root, &name, 1)
                .map_err(|_| invalid_registry())?
                .ok_or_else(invalid_registry)?;
            if !lock.bytes().is_empty() {
                return Err(invalid_registry());
            }
            continue;
        }
        if !canonical_uuid_v4(&name) {
            return Err(invalid_registry());
        }
        root.open_child(&name).map_err(|_| invalid_registry())?;
        generations.push(name);
        if generations.len() > MAX_CAPTURE_GENERATIONS {
            return Err(invalid_registry());
        }
    }
    root.assert_unchanged().map_err(|_| invalid_registry())?;
    generations.sort_unstable();
    Ok(generations)
}

fn owner_limits() -> Result<JsonLimits, TerminalBackendError> {
    JsonLimits::new(MAX_CAPTURE_OWNER_BYTES, 12, 128).map_err(|_| invalid_registry())
}

fn read_capture_owner(
    generation: &PrivateDirectory,
    capture_token: &str,
) -> Result<Option<StoredCaptureOwner>, TerminalBackendError> {
    let owner = read_json::<StoredCaptureOwner>(generation, CAPTURE_OWNER_FILE, owner_limits()?)
        .map_err(|_| invalid_registry())?
        .map(|loaded| loaded.into_parts().0);
    if let Some(owner) = owner.as_ref() {
        validate_capture_owner(owner, capture_token)?;
    }
    Ok(owner)
}

/// The generation's capture owner, refused unless it names exactly
/// `process`. A missing owner is `None`, or refused when `required` (only
/// the `OpenCode` observation needs one; Claude and Codex captures may predate
/// it).
fn verified_owner(
    receipts_root: &Path,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    required: bool,
) -> Result<Option<StoredCaptureOwner>, TerminalBackendError> {
    let owner = capture_owner(receipts_root, capture_token)?;
    match &owner {
        Some(owner) if owner.process.as_ref() != Some(&StoredProcessIdentity::from(process)) => {
            Err(invalid_registry())
        }
        None if required => Err(invalid_registry()),
        _ => Ok(owner),
    }
}

fn capture_owner(
    receipts_root: &Path,
    capture_token: &str,
) -> Result<Option<StoredCaptureOwner>, TerminalBackendError> {
    if !canonical_uuid_v4(capture_token) {
        return Err(invalid_registry());
    }
    let root = PrivateDirectory::open(receipts_root).map_err(|_| invalid_registry())?;
    let path = root.path().join(capture_token);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid_registry()),
    }
    let generation = root
        .open_child(capture_token)
        .map_err(|_| invalid_registry())?;
    read_capture_owner(&generation, capture_token)
}

fn capture_process_group_is_gone(process: &StoredProcessIdentity) -> Result<bool, ()> {
    validate_process_owner(process).map_err(|_| ())?;
    captured_process_group_is_live(&TerminalProcessIdentity::from(process))
        .map(|live| !live)
        .map_err(|_| ())
}

fn write_new_capture_owner(
    generation: &PrivateDirectory,
    writer: &WriterLock,
    owner: &StoredCaptureOwner,
) -> Result<(), TerminalBackendError> {
    if read_capture_owner(generation, &owner.capture_token)?.is_some() {
        return Err(invalid_registry());
    }
    write_capture_owner(generation, writer, owner)
}

fn write_capture_owner(
    generation: &PrivateDirectory,
    writer: &WriterLock,
    owner: &StoredCaptureOwner,
) -> Result<(), TerminalBackendError> {
    validate_capture_owner(owner, &owner.capture_token)?;
    let expected = snapshot_target(generation, CAPTURE_OWNER_FILE, MAX_CAPTURE_OWNER_BYTES)
        .map_err(|_| invalid_registry())?;
    atomic_write_json(
        generation,
        writer,
        CAPTURE_OWNER_FILE,
        owner,
        &expected,
        owner_limits()?,
    )
    .map_err(|_| invalid_registry())?;
    Ok(())
}

fn validate_capture_owner(
    owner: &StoredCaptureOwner,
    capture_token: &str,
) -> Result<(), TerminalBackendError> {
    if !matches!(
        (owner.version, owner.nested_shell),
        (LEGACY_CAPTURE_OWNER_VERSION, false) | (CAPTURE_OWNER_VERSION, _)
    ) || owner.capture_token != capture_token
        || !canonical_uuid_v4(capture_token)
        || owner.terminal_generation.0 == 0
        || !valid_owner_path(&owner.root)
        || !valid_owner_path(&owner.cwd)
        || !Path::new(&owner.cwd).starts_with(Path::new(&owner.root))
        || owner
            .prior_launch_session
            .as_ref()
            .is_some_and(|session| !valid_launch_session(session))
        || owner
            .process
            .as_ref()
            .is_some_and(|process| validate_process_owner(process).is_err())
    {
        return Err(invalid_registry());
    }
    Ok(())
}

pub(super) fn valid_owner_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4_096
        && !value.chars().any(char::is_control)
        && Path::new(value).is_absolute()
        && !Path::new(value)
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

fn valid_launch_session(session: &TerminalLaunchSessionV1) -> bool {
    session.version == TERMINAL_LAUNCH_SESSION_VERSION
        && session.id.as_ref().is_none_or(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        })
}

fn validate_process_owner(process: &StoredProcessIdentity) -> Result<(), TerminalBackendError> {
    if process.process_id == 0
        || process.process_group_id != process.process_id
        || process.process_start.len() > 128
        || normalize_whitespace(&process.process_start) != process.process_start
        || process.process_start.is_empty()
        || !process.process_start.is_ascii()
        || process.process_start.chars().any(char::is_control)
        || process.terminal_device.as_ref().is_some_and(|terminal| {
            terminal.is_empty()
                || terminal.len() > 128
                || !terminal.is_ascii()
                || terminal.chars().any(char::is_control)
        })
    {
        return Err(invalid_registry());
    }
    #[cfg(target_os = "macos")]
    if process
        .terminal_device
        .as_deref()
        .is_none_or(|terminal| !valid_terminal_device(terminal))
    {
        return Err(invalid_registry());
    }
    Ok(())
}

fn invalid_registry() -> TerminalBackendError {
    TerminalBackendError::new("Claude terminal session metadata is unsafe or malformed")
}

/// Providers with a host-private `SessionStart` receipt path.
pub(crate) fn captured_profile(profile_id: TerminalLaunchProfileId) -> bool {
    matches!(
        profile_id,
        TerminalLaunchProfileId::Claude
            | TerminalLaunchProfileId::Codex
            | TerminalLaunchProfileId::Opencode
            | TerminalLaunchProfileId::Gemini
            | TerminalLaunchProfileId::Muse
            | TerminalLaunchProfileId::Kimi
    )
}

/// The lookup's name before it served every captured provider; the tests,
/// pinned by landings, still use it.
#[cfg(test)]
pub(crate) type ClaudeTerminalSessionLookup = ProviderCaptureLookup;

#[cfg(test)]
#[path = "claude_terminal_session_tests.rs"]
mod tests;
