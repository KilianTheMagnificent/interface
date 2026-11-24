//! Provider capture, recovery polling and final receipt projection.
use super::{
    Arc, Instant, ManualProviderExit, PROVIDER_SESSION_STARTUP_POLL_DELAY,
    PROVIDER_SESSION_STARTUP_POLLS, Path, PathBuf, ProviderSessionCaptureKind, ProviderSessionWake,
    Shared, TERMINAL_LAUNCH_REQUEST_VERSION, TerminalBackendError, TerminalEntry,
    TerminalGeneration, TerminalId, TerminalLaunchCatalog, TerminalLaunchProfileId,
    TerminalLaunchRequestV1, TerminalLaunchSessionV1, TerminalLifecycleEvent,
    TerminalProcessControl, TerminalProcessIdentity, TerminalProviderSession,
    TerminalProviderSessionCaptureProcess, TerminalProviderSessionLookup,
    TerminalProviderSessionPersistence, TerminalProviderSessionTransition, TerminalService,
    TerminalState, TerminalSummary, Weak, canonical_uuid, canonical_uuid_v4,
    is_provider_session_id, lock, now, polling, provider_session_gate, thread,
    validated_manual_provider_cwd,
};

impl TerminalService {
    /// Polls one capturing generation's provider session: on each wake, and
    /// on a bounded backoff that no lost wake can stall. `initially_attached`
    /// says the generation's direct child is already its provider process.
    pub(super) fn start_provider_session_poll(
        &self,
        id: TerminalId,
        generation: TerminalGeneration,
        wake: Arc<ProviderSessionWake>,
        initially_attached: bool,
    ) {
        let shared = Arc::downgrade(&self.shared);
        thread::spawn(move || {
            let mut attached = initially_attached;
            let mut poll_count = 0_usize;
            loop {
                if super::workspace_sleep::sleeping(&shared, id, generation) {
                    // A stopped process sends no hook wake, so a sleeping pane
                    // checks back at the awake idle pace. A wake that arrives
                    // just after the resume still starts a lookup burst.
                    if wake.wait(polling::recovery_delay(usize::MAX)) {
                        poll_count = 0;
                    }
                    continue;
                }
                if !attached {
                    if !provider_capture_is_active(&shared, id, generation) {
                        return;
                    }
                    // The private OSC frame is only a latency hint. The hook
                    // commits its receipt before writing that frame, so a
                    // short stdout failure or an orderly close must not make
                    // the durable wrapper-owned process invisible forever.
                    // Probe the wrapper's durable process claim on every
                    // bounded timeout as well as on explicit wakes. The PTY's
                    // foreground process is not provider authentication: an
                    // unrelated job may own the foreground group while this
                    // shell remains armed.
                    if wake.wait(polling::recovery_delay(
                        poll_count.max(PROVIDER_SESSION_STARTUP_POLLS),
                    )) {
                        poll_count = 0;
                    } else {
                        poll_count = poll_count.saturating_add(1);
                    }
                    attached = activate_manual_provider_process(&shared, id, generation);
                    if !attached {
                        continue;
                    }
                    poll_count = 0;
                }
                let lookup_started = Instant::now();
                refresh_provider_session_binding(&shared, id, generation);
                super::titles::refresh_codex_title(&shared, id, generation);
                match demote_finished_manual_provider(&shared, id, generation) {
                    ManualProviderExit::RearmedShell => {
                        attached = false;
                        poll_count = 0;
                        continue;
                    }
                    ManualProviderExit::ReplacedProvider => {
                        // A hook wake for the replacement may already have
                        // been consumed by the previous iteration's wait.
                        // Stay attached and immediately begin a fresh bounded
                        // lookup burst against the new exact process.
                        attached = true;
                        poll_count = 0;
                        continue;
                    }
                    ManualProviderExit::Unchanged => {}
                }
                let active = {
                    let Some(shared) = shared.upgrade() else {
                        return;
                    };
                    let state = lock(&shared.state);
                    state.entries.get(&id).is_some_and(|entry| {
                        entry.summary.generation == generation && entry.is_live()
                    })
                };
                if !active {
                    return;
                }
                let delay = polling::recovery_delay(poll_count);
                if wake.wait(delay) {
                    // Claude updates the process pointer asynchronously after
                    // SessionStart. One coalesced wake therefore starts a
                    // fresh bounded retry burst rather than buying one read.
                    poll_count = 0;
                    thread::sleep(
                        PROVIDER_SESSION_STARTUP_POLL_DELAY
                            .saturating_sub(lookup_started.elapsed()),
                    );
                } else {
                    poll_count = poll_count.saturating_add(1);
                }
            }
        });
    }
}

/// Publish only presentation metadata. Provider receipts remain the sole
/// authority for resumable session IDs and nested-process ownership. An
/// attached manual Claude has no receipt until its first prompt writes a
/// transcript, so its mark is shown until one binds.
pub(super) fn refresh_shell_display(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    let (power, control, shown) = {
        let Some(shared) = shared.upgrade() else {
            return false;
        };
        let state = lock(&shared.state);
        let Some(entry) = state.entries.get(&id) else {
            return false;
        };
        if entry.summary.generation != generation
            || entry.closing
            || entry.summary.state != TerminalState::Running
        {
            return false;
        }
        let Some(display) = entry.shell_display.as_ref() else {
            return false;
        };
        if entry.provider_session_process.is_some() && entry.summary.launch_session.is_some() {
            return true;
        }
        (
            Arc::clone(&entry.power),
            Arc::clone(&entry.resources.control),
            display.profile,
        )
    };
    // Never under the state lock, which every keystroke needs. Suspends and
    // resumes hold only the transition gate across their process spawns.
    if lock(&power).suspended() {
        return true;
    }
    let Ok(profile) = control.foreground_profile() else {
        return true;
    };
    if profile == shown {
        return true;
    }
    let Some(shared) = shared.upgrade() else {
        return false;
    };
    let _event_gate = lock(&shared.event_gate);
    let event = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return false;
        };
        if entry.summary.generation != generation
            || entry.closing
            || entry.summary.state != TerminalState::Running
        {
            return false;
        }
        if entry.provider_session_process.is_some() && entry.summary.launch_session.is_some() {
            return true;
        }
        let Some(display) = entry.shell_display.as_mut() else {
            return false;
        };
        if display.profile == profile {
            return true;
        }
        if !entry.title_is_user
            && (profile.is_none()
                || display.profile.is_some()
                || entry.summary.title == display.title)
        {
            entry.summary.title = match profile {
                Some(TerminalLaunchProfileId::Codex) => "Codex".to_owned(),
                Some(TerminalLaunchProfileId::Claude) => "Claude Code".to_owned(),
                _ => display.title.clone(),
            };
        }
        display.profile = profile;
        entry.summary.fast_mode = None;
        entry.summary.launch = profile
            .map(|profile_id| TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id,
            })
            .or(display.launch);
        entry.summary.launch_session = None;
        entry.summary.updated_at = now();
        let terminal = entry.summary.clone();
        state.revision = state.revision.saturating_add(1);
        TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal,
        }
    };
    shared.events.publish(event);
    true
}

/// Separate the provider-owned session identity from the renderer's launch
/// request while a fresh Claude process is starting.
pub(super) fn provider_session_capture_kind(
    launches: &TerminalLaunchCatalog,
    launch: Option<TerminalLaunchRequestV1>,
) -> Option<ProviderSessionCaptureKind> {
    let profile_id = launch.map_or(TerminalLaunchProfileId::Shell, |launch| launch.profile_id);
    if !launches.captures_provider_session(profile_id) {
        return None;
    }
    match profile_id {
        TerminalLaunchProfileId::Claude
        | TerminalLaunchProfileId::Codex
        | TerminalLaunchProfileId::Opencode
        | TerminalLaunchProfileId::Gemini
        | TerminalLaunchProfileId::Muse
        | TerminalLaunchProfileId::Kimi => Some(ProviderSessionCaptureKind::Direct),
        TerminalLaunchProfileId::Shell => Some(ProviderSessionCaptureKind::ManualShell {
            original_launch: launch,
        }),
        _ => None,
    }
}

/// The provider a capture generation is polled as: the launched CLI for a
/// direct capture, Claude for an armed login shell. A shell whose wrapper ran
/// Codex instead is told apart by its receipt; see [`manual_provider_profile`].
pub(super) fn provider_session_capture_profile(
    kind: &ProviderSessionCaptureKind,
    launch: Option<TerminalLaunchRequestV1>,
) -> TerminalLaunchProfileId {
    match kind {
        ProviderSessionCaptureKind::Direct => {
            launch.map_or(TerminalLaunchProfileId::Claude, |launch| launch.profile_id)
        }
        ProviderSessionCaptureKind::ManualShell { .. } => TerminalLaunchProfileId::Claude,
    }
}

pub(super) fn captured_provider(profile_id: Option<TerminalLaunchProfileId>) -> bool {
    matches!(
        profile_id,
        Some(
            TerminalLaunchProfileId::Claude
                | TerminalLaunchProfileId::Codex
                | TerminalLaunchProfileId::Opencode
                | TerminalLaunchProfileId::Gemini
                | TerminalLaunchProfileId::Muse
                | TerminalLaunchProfileId::Kimi
        )
    )
}

/// The profile an armed login shell is promoted to. Codex threads are UUID
/// v7 and Claude chats v4, so the receipt's id names the wrapper that ran.
pub(super) fn manual_provider_profile(
    profile_id: TerminalLaunchProfileId,
    session_id: &str,
) -> TerminalLaunchProfileId {
    if canonical_uuid(session_id, b'7') {
        TerminalLaunchProfileId::Codex
    } else {
        profile_id
    }
}

/// Keep fresh Claude request IDs unconfirmed, but preserve provider-proven
/// resume IDs across launch/restart. OSC is wake-only, so erasing a known
/// resume before the live process and receipt reappear would turn an
/// update-time crash into an unnecessary recovery picker.
pub(super) fn public_provider_session_state(
    captures_provider_session: bool,
    requested: Option<&TerminalLaunchSessionV1>,
) -> Option<TerminalLaunchSessionV1> {
    if captures_provider_session {
        requested.map(|session| TerminalLaunchSessionV1 {
            version: session.version,
            id: if session.resume {
                session.id.clone()
            } else {
                None
            },
            resume: session.resume,
        })
    } else {
        requested.cloned()
    }
}

/// Treat a private `SessionStart` OSC as a coalesced wake-up only. Nothing
/// after the generation token is trusted or interpreted; the durable UUID is
/// derived independently from a host-private, transcript-corroborated receipt.
pub(super) fn handle_provider_session_wake(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    payload: &str,
) {
    let Some((capture_token, marker)) = payload.split_once(':') else {
        return;
    };
    if !canonical_uuid_v4(capture_token) || marker.is_empty() {
        return;
    }
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let wake = {
        let state = lock(&shared.state);
        state.entries.get(&id).and_then(|entry| {
            (entry.summary.generation == generation
                && entry.provider_session_capture_token.as_deref() == Some(capture_token)
                && entry.provider_session_profile == Some(TerminalLaunchProfileId::Claude))
            .then(|| entry.provider_session_wake.clone())
            .flatten()
        })
    };
    if let Some(wake) = wake {
        wake.signal();
    }
}

pub(super) fn provider_capture_is_active(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    let Some(shared) = shared.upgrade() else {
        return false;
    };
    let state = lock(&shared.state);
    state.entries.get(&id).is_some_and(|entry| {
        entry.summary.generation == generation
            && entry.is_live()
            && entry.provider_session_capture_token.is_some()
            && captured_provider(entry.provider_session_profile)
    })
}

/// Turn a hook wake or bounded recovery poll from an armed login shell into an
/// exact provider-process attachment. Only the wrapper's durable process claim
/// is authoritative; arbitrary foreground PTY jobs must remain ordinary shell
/// activity.
pub(super) fn activate_manual_provider_process(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    let Some(shared) = shared.upgrade() else {
        return false;
    };
    let gate = provider_session_gate(&shared, id, generation);
    let _provider_session_gate = lock(&gate);
    let (lookup, capture_token, root) = {
        let state = lock(&shared.state);
        let Some(entry) = state.entries.get(&id) else {
            return false;
        };
        if entry.summary.generation != generation
            || entry.closing
            || entry.provider_session_process.is_some()
            || !entry.manual_capture()
            || entry.provider_session_profile != Some(TerminalLaunchProfileId::Claude)
        {
            return false;
        }
        let Some(capture_token) = entry.provider_session_capture_token.clone() else {
            return false;
        };
        (
            Arc::clone(&shared.provider_sessions),
            capture_token,
            entry.root.clone(),
        )
    };
    match lookup.capture_process(TerminalLaunchProfileId::Claude, &capture_token) {
        Ok(Some(captured)) => {
            let Some(process_cwd) = validated_manual_provider_cwd(&captured.cwd, &root) else {
                return false;
            };
            let mut state = lock(&shared.state);
            let Some(entry) = state.entries.get_mut(&id) else {
                return false;
            };
            if entry.summary.generation != generation
                || entry.closing
                || entry.provider_session_process.is_some()
                || entry.provider_session_capture_token.as_deref() != Some(capture_token.as_str())
            {
                return false;
            }
            entry.provider_session_process = Some(captured.process);
            entry.provider_session_cwd = Some(process_cwd);
            entry.provider_session_replacing = entry.summary.launch_session.is_some()
                && entry
                    .summary
                    .launch
                    .is_some_and(|launch| captured_provider(Some(launch.profile_id)));
            true
        }
        Ok(None) | Err(_) => false,
    }
}

#[allow(clippy::too_many_arguments)] // Exact owner and predecessor fences stay explicit.
pub(super) fn adopt_durable_manual_replacement(
    shared: &Arc<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    previous: &TerminalProcessIdentity,
    capture_token: &str,
    root: &str,
    prior_launch_session: Option<&TerminalLaunchSessionV1>,
    captured: TerminalProviderSessionCaptureProcess,
) -> bool {
    if captured.process == *previous {
        return false;
    }
    let Some(replacement_cwd) = validated_manual_provider_cwd(&captured.cwd, Path::new(root))
    else {
        return false;
    };
    let mut state = lock(&shared.state);
    let Some(entry) = state.entries.get_mut(&id) else {
        return false;
    };
    if !entry.owns_capture(generation, previous, capture_token) {
        return false;
    }
    entry.provider_session_process = Some(captured.process);
    entry.provider_session_cwd = Some(replacement_cwd);
    entry.provider_session_replacing = prior_launch_session.is_some()
        && entry
            .summary
            .launch
            .is_some_and(|launch| captured_provider(Some(launch.profile_id)));
    true
}

/// A manually invoked Claude returns to its parent shell instead of ending the
/// terminal generation. Once the exact child identity is gone, restore the
/// original shell launch and re-arm the same private generation token.
#[allow(clippy::too_many_lines)] // One provider gate keeps settlement, replacement, and shell rearm atomic.
pub(super) fn demote_finished_manual_provider(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> ManualProviderExit {
    let Some(shared) = shared.upgrade() else {
        return ManualProviderExit::Unchanged;
    };
    let shared_weak = Arc::downgrade(&shared);
    let gate = provider_session_gate(&shared, id, generation);
    let _provider_session_gate = lock(&gate);
    let (control, lookup, process, capture_token, original_launch, root, prior_before_settle) = {
        let state = lock(&shared.state);
        let Some(entry) = state.entries.get(&id) else {
            return ManualProviderExit::Unchanged;
        };
        let Some(ProviderSessionCaptureKind::ManualShell { original_launch }) =
            entry.provider_session_capture_kind.clone()
        else {
            return ManualProviderExit::Unchanged;
        };
        if entry.summary.generation != generation || entry.closing {
            return ManualProviderExit::Unchanged;
        }
        let Some(process) = entry.provider_session_process.clone() else {
            return ManualProviderExit::Unchanged;
        };
        let Some(capture_token) = entry.provider_session_capture_token.clone() else {
            return ManualProviderExit::Unchanged;
        };
        (
            Arc::clone(&entry.resources.control),
            Arc::clone(&shared.provider_sessions),
            process,
            capture_token,
            original_launch,
            entry.summary.root.clone(),
            entry.summary.launch_session.clone(),
        )
    };
    match control.process_group_identity_is_live(&process) {
        Ok(true) | Err(_) => return ManualProviderExit::Unchanged,
        Ok(false) => {}
    }

    // A checkpointed predecessor may already have been atomically handed to
    // a new wrapper before this poll observes the old process exit. In that
    // case the durable owner names the replacement, so do not ask a settled
    // lookup to authenticate the predecessor against the replacement owner.
    let adopt = |captured| {
        if adopt_durable_manual_replacement(
            &shared,
            id,
            generation,
            &process,
            &capture_token,
            &root,
            prior_before_settle.as_ref(),
            captured,
        ) {
            ManualProviderExit::ReplacedProvider
        } else {
            ManualProviderExit::Unchanged
        }
    };
    let replace = |prior: Option<&TerminalLaunchSessionV1>| {
        replace_finished_manual_provider(
            &shared,
            &shared_weak,
            &*lookup,
            &*control,
            id,
            generation,
            &process,
            &capture_token,
            &root,
            prior,
        )
    };
    match lookup.capture_process(TerminalLaunchProfileId::Claude, &capture_token) {
        Ok(Some(captured)) if captured.process != process => return adopt(captured),
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => return ManualProviderExit::Unchanged,
    }

    // The hook receipt can land after the last live polling read. File and
    // checkpoint that exact dead invocation before either reusing this
    // generation for another Claude process or returning the pane to Shell.
    // The host's settled lookup deliberately leaves the generation unsealed.
    match lookup.settled_session(TerminalLaunchProfileId::Claude, &process, &capture_token) {
        Ok(Some(provider_session)) => {
            let persistence = apply_provider_session_binding(
                &shared_weak,
                id,
                generation,
                false,
                TerminalLaunchProfileId::Claude,
                &process,
                &provider_session,
            );
            if persistence == TerminalProviderSessionPersistence::RetryableFailure {
                return ManualProviderExit::Unchanged;
            }
            let Some(launch_session) = capture_checkpoint_session(
                &shared_weak,
                id,
                generation,
                TerminalLaunchProfileId::Claude,
                &process,
                &capture_token,
            ) else {
                return ManualProviderExit::Unchanged;
            };
            if lookup
                .checkpoint_capture(
                    TerminalLaunchProfileId::Claude,
                    &capture_token,
                    &launch_session,
                )
                .is_err()
            {
                return ManualProviderExit::Unchanged;
            }
        }
        Ok(None) => {}
        Err(_) => {
            return lookup
                .capture_process(TerminalLaunchProfileId::Claude, &capture_token)
                .ok()
                .flatten()
                .map_or(ManualProviderExit::Unchanged, adopt);
        }
    }

    let prior_launch_session = {
        let state = lock(&shared.state);
        let Some(entry) = state.entries.get(&id) else {
            return ManualProviderExit::Unchanged;
        };
        if !entry.owns_capture(generation, &process, &capture_token) {
            return ManualProviderExit::Unchanged;
        }
        entry.summary.launch_session.clone()
    };

    if replace(prior_launch_session.as_ref()) {
        return ManualProviderExit::ReplacedProvider;
    }

    let event_gate = lock(&shared.event_gate);
    let proposed_summary = {
        let state = lock(&shared.state);
        let Some(entry) = state.entries.get(&id) else {
            return ManualProviderExit::Unchanged;
        };
        if !(entry.owns_capture(generation, &process, &capture_token) && entry.is_live()) {
            return ManualProviderExit::Unchanged;
        }
        let mut summary = entry.summary.clone();
        summary.fast_mode = None;
        summary.launch = original_launch;
        summary.launch_session = None;
        summary.updated_at = now();
        summary
    };
    if shared.events.persist_provider_reopen(&proposed_summary)
        == TerminalProviderSessionPersistence::RetryableFailure
    {
        return ManualProviderExit::Unchanged;
    }
    if lookup
        .rearm_nested_process(
            TerminalLaunchProfileId::Claude,
            &capture_token,
            &process,
            Path::new(&proposed_summary.cwd),
        )
        .is_err()
    {
        drop(event_gate);
        return if replace(None) {
            ManualProviderExit::ReplacedProvider
        } else {
            ManualProviderExit::Unchanged
        };
    }

    let event = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return ManualProviderExit::Unchanged;
        };
        if !(entry.owns_capture(generation, &process, &capture_token) && entry.is_live()) {
            return ManualProviderExit::Unchanged;
        }
        entry.summary = proposed_summary;
        entry.provider_session_process = None;
        entry.provider_session_cwd = None;
        entry.provider_session_finalizing = false;
        entry.provider_session_replacing = false;
        let summary = entry.summary.clone();
        state.revision = state.revision.saturating_add(1);
        TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal: summary,
        }
    };
    shared.events.publish(event);
    drop(event_gate);
    ManualProviderExit::RearmedShell
}

pub(super) fn manual_provider_replacement(
    control: &dyn TerminalProcessControl,
    finished: &TerminalProcessIdentity,
    root: &str,
) -> Option<(TerminalProcessIdentity, PathBuf)> {
    let replacement = control.foreground_process_identity().ok().flatten()?;
    if replacement == *finished || control.process_identity().as_ref() == Some(&replacement) {
        return None;
    }
    if control.process_identity_is_live(&replacement).ok() != Some(true) {
        return None;
    }
    let cwd = control
        .process_cwd(&replacement)
        .ok()
        .flatten()
        .and_then(|cwd| std::fs::canonicalize(cwd).ok())?;
    let root = Path::new(root);
    (cwd == root || cwd.starts_with(root)).then_some((replacement, cwd))
}

#[allow(clippy::too_many_arguments)] // Exact owner, PTY, and predecessor fences stay explicit.
pub(super) fn replace_finished_manual_provider(
    shared: &Arc<Shared>,
    shared_weak: &Weak<Shared>,
    lookup: &dyn TerminalProviderSessionLookup,
    control: &dyn TerminalProcessControl,
    id: TerminalId,
    generation: TerminalGeneration,
    previous: &TerminalProcessIdentity,
    capture_token: &str,
    root: &str,
    prior_launch_session: Option<&TerminalLaunchSessionV1>,
) -> bool {
    let Some((replacement, replacement_cwd)) = manual_provider_replacement(control, previous, root)
    else {
        return false;
    };
    let Ok(Some(provider_session)) = lookup.replace_nested_process(
        TerminalLaunchProfileId::Claude,
        capture_token,
        previous,
        &replacement,
        &replacement_cwd,
        prior_launch_session,
    ) else {
        return false;
    };
    {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return false;
        };
        if !(entry.owns_capture(generation, previous, capture_token) && entry.is_live()) {
            return false;
        }
        entry.provider_session_process = Some(replacement.clone());
        entry.provider_session_cwd = Some(replacement_cwd);
        entry.provider_session_finalizing = false;
        entry.provider_session_replacing = true;
    }
    let persistence = apply_provider_session_binding(
        shared_weak,
        id,
        generation,
        false,
        TerminalLaunchProfileId::Claude,
        &replacement,
        &provider_session,
    );
    if persistence != TerminalProviderSessionPersistence::RetryableFailure
        && let Some(launch_session) = capture_checkpoint_session(
            shared_weak,
            id,
            generation,
            TerminalLaunchProfileId::Claude,
            &replacement,
            capture_token,
        )
    {
        let _ = lookup.checkpoint_capture(
            TerminalLaunchProfileId::Claude,
            capture_token,
            &launch_session,
        );
    }
    true
}

pub(super) fn refresh_provider_session_binding(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) {
    let _ = refresh_provider_session_binding_with(shared, id, generation, false);
}

pub(super) fn mark_terminal_closing(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let gate = provider_session_gate(&shared, id, generation);
    let _provider_session_gate = lock(&gate);
    let mut state = lock(&shared.state);
    let Some(entry) = state.entries.get_mut(&id) else {
        return;
    };
    if entry.summary.generation == generation && entry.is_live() {
        entry.closing = true;
    }
}

pub(super) fn has_resumable_provider_binding(
    entry: &TerminalEntry,
    profile_id: TerminalLaunchProfileId,
) -> bool {
    entry
        .summary
        .launch
        .is_some_and(|launch| launch.profile_id == profile_id)
        && entry
            .summary
            .launch_session
            .as_ref()
            .is_some_and(|session| {
                session.version == 1
                    && session.resume
                    && session.id.as_deref().is_some_and(is_provider_session_id)
            })
}

pub(super) fn seal_nested_provider_for_terminal(
    shared: &Arc<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> Result<(), TerminalBackendError> {
    let gate = provider_session_gate(shared, id, generation);
    let _provider_session_gate = lock(&gate);
    let (lookup, profile_id, capture_token, in_memory_process, root) = {
        let state = lock(&shared.state);
        let Some(entry) = state.entries.get(&id) else {
            return Ok(());
        };
        if entry.summary.generation != generation || !entry.manual_capture() {
            return Ok(());
        }
        let profile_id = entry
            .provider_session_profile
            .ok_or_else(|| TerminalBackendError::new("provider capture profile was unavailable"))?;
        let capture_token = entry
            .provider_session_capture_token
            .clone()
            .ok_or_else(|| TerminalBackendError::new("provider capture token was unavailable"))?;
        (
            Arc::clone(&shared.provider_sessions),
            profile_id,
            capture_token,
            entry.provider_session_process.clone(),
            entry.root.clone(),
        )
    };
    let captured = lookup.seal_capture(profile_id, &capture_token)?;
    let Some(captured) = captured else {
        return if in_memory_process.is_none() {
            Ok(())
        } else {
            Err(TerminalBackendError::new(
                "sealed provider capture omitted its attached process",
            ))
        };
    };
    // Cwd validity controls whether a recovered reopen tuple can be filed;
    // it must never prevent teardown of an already sealed process owner. A
    // process can legitimately retain a directory that was renamed/unlinked.
    let safe_cwd = validated_manual_provider_cwd(&captured.cwd, &root);
    let mut state = lock(&shared.state);
    let Some(entry) = state.entries.get_mut(&id) else {
        return Ok(());
    };
    if entry.summary.generation != generation
        || entry.provider_session_capture_token.as_deref() != Some(capture_token.as_str())
    {
        return Err(TerminalBackendError::new(
            "provider capture changed during terminal teardown",
        ));
    }
    if entry
        .provider_session_process
        .as_ref()
        .is_some_and(|process| process != &captured.process)
        && has_resumable_provider_binding(entry, profile_id)
    {
        // A replacement wrapper can claim B after A exits but before the live
        // poll observes it. Preserve that ordering across shutdown so B's
        // ordinary Startup receipt is allowed to supersede the resumable A
        // binding at the final, sealed lookup.
        entry.provider_session_replacing = true;
    }
    entry.provider_session_process = Some(captured.process);
    entry.provider_session_cwd = safe_cwd;
    Ok(())
}

pub(super) fn refresh_final_provider_session_binding(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    refresh_provider_session_binding_with(shared, id, generation, true)
}

#[allow(clippy::too_many_lines)] // One gate keeps exact-child proof and receipt persistence atomic.
pub(super) fn refresh_provider_session_binding_with(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    process_reaped: bool,
) -> bool {
    let Some(current_shared) = shared.upgrade() else {
        return true;
    };
    let manual_capture = {
        let state = lock(&current_shared.state);
        state
            .entries
            .get(&id)
            .is_some_and(|entry| entry.summary.generation == generation && entry.manual_capture())
    };
    // Direct-child finalization must be able to fence a blocked live lookup.
    // Only nested manual owners need the durable attach/seal transaction gate.
    let gate = manual_capture.then(|| provider_session_gate(&current_shared, id, generation));
    let _provider_session_gate = gate.as_ref().map(|gate| lock(gate));
    let (lookup, control, profile_id, process, capture_token, manual_shell) = {
        let mut state = lock(&current_shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return true;
        };
        if entry.summary.generation != generation || !entry.is_live() {
            return true;
        }
        let manual_shell = entry.manual_capture();
        if process_reaped {
            if !manual_shell {
                entry.provider_session_finalizing = true;
            }
        } else if entry.provider_session_finalizing {
            return true;
        }
        let Some(profile_id) = entry.provider_session_profile else {
            return true;
        };
        let Some(capture_token) = entry.provider_session_capture_token.clone() else {
            return true;
        };
        (
            Arc::clone(&current_shared.provider_sessions),
            Arc::clone(&entry.resources.control),
            profile_id,
            entry.provider_session_process.clone(),
            capture_token,
            manual_shell,
        )
    };
    let reaped_manual = process_reaped && manual_shell;
    let process = if reaped_manual {
        let captured = match lookup.seal_capture(profile_id, &capture_token) {
            Ok(Some(captured)) => captured,
            Ok(None) if process.is_none() => {
                if lookup.end_capture(profile_id, &capture_token).is_err() {
                    return false;
                }
                clear_processless_provider_capture(
                    shared,
                    id,
                    generation,
                    profile_id,
                    &capture_token,
                );
                return true;
            }
            Ok(None) | Err(_) => return false,
        };
        let mut recovered_cwd = std::fs::canonicalize(&captured.cwd)
            .ok()
            .filter(|cwd| cwd == &captured.cwd && cwd.is_dir());
        match control.process_group_identity_is_live(&captured.process) {
            Ok(false) => {}
            Ok(true) | Err(_) => return false,
        }
        {
            let mut state = lock(&current_shared.state);
            let Some(entry) = state.entries.get_mut(&id) else {
                return true;
            };
            if entry.summary.generation != generation
                || !entry.is_live()
                || entry.provider_session_capture_token.as_deref() != Some(capture_token.as_str())
            {
                // The provider-session gate prevents a capture transition here.
                // Treat any unexpected drift as retryable instead of finalizing
                // an identity we did not just prove absent.
                return false;
            }
            if recovered_cwd
                .as_ref()
                .is_some_and(|cwd| cwd != &entry.root && !cwd.starts_with(&entry.root))
            {
                recovered_cwd = None;
            }
            // A trusted wrapper can commit its exact owner before the OSC wake is
            // serialized. Recover that durable identity at the final boundary so
            // orderly close cannot skip or delete the only provider receipt.
            entry.provider_session_process = Some(captured.process.clone());
            entry.provider_session_cwd.clone_from(&recovered_cwd);
            entry.provider_session_finalizing = true;
        }
        if recovered_cwd.is_none() {
            // The process is gone and the terminal may settle, but persisting a
            // provider identity against a missing/renamed cwd would create an
            // unsafe reopen tuple. Keep the sealed receipt for startup recovery.
            return true;
        }
        captured.process
    } else {
        let Some(process) = process else {
            return true;
        };
        process
    };
    let lookup_result = if process_reaped {
        lookup.final_session(profile_id, &process, &capture_token)
    } else {
        lookup.current_session(profile_id, &process, &capture_token)
    };
    let Ok(provider_session) = lookup_result else {
        // For a nested child, a backend refusal after the fast liveness probe
        // is still an ambiguous live generation (for example the PTY device
        // disappeared while PID+start-time remained). Keep the terminal and
        // its lease live until the authoritative final lookup agrees that the
        // exact process is gone. Direct children were already reaped by the
        // waiter and keep their existing startup-recovery fallback.
        return !reaped_manual;
    };
    let had_provider_session = provider_session.is_some();
    let mut persistence = if let Some(provider_session) = provider_session {
        apply_provider_session_binding(
            shared,
            id,
            generation,
            process_reaped,
            profile_id,
            &process,
            &provider_session,
        )
    } else {
        TerminalProviderSessionPersistence::NotSaved
    };
    if had_provider_session && persistence != TerminalProviderSessionPersistence::RetryableFailure {
        let checkpoint = capture_checkpoint_session(
            shared,
            id,
            generation,
            profile_id,
            &process,
            &capture_token,
        )
        .and_then(|launch_session| {
            lookup
                .checkpoint_capture(profile_id, &capture_token, &launch_session)
                .ok()
        });
        if checkpoint.is_none() {
            // The canonical saved binding may already be durable, but without
            // the matching predecessor checkpoint a later transition receipt
            // cannot be replayed safely after a crash. Keep polling and retain
            // the capture until both writes agree.
            persistence = TerminalProviderSessionPersistence::RetryableFailure;
        }
    }
    if process_reaped && persistence != TerminalProviderSessionPersistence::RetryableFailure {
        // `final_session` seals the provider generation first. Retire its
        // private receipt only after the lifecycle sink has synchronously
        // filed any new binding, so an orderly app shutdown cannot delete the
        // sole durable evidence before persistence.
        if lookup.end_capture(profile_id, &capture_token).is_ok() {
            clear_provider_capture(shared, id, generation, &process, &capture_token);
        }
    }
    true
}

pub(super) fn clear_provider_capture(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    process: &TerminalProcessIdentity,
    capture_token: &str,
) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let mut state = lock(&shared.state);
    let Some(entry) = state.entries.get_mut(&id) else {
        return;
    };
    if !entry.owns_capture(generation, process, capture_token) {
        return;
    }
    entry.clear_provider_capture();
    entry.provider_session_replacing = false;
}

pub(super) fn clear_processless_provider_capture(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    profile_id: TerminalLaunchProfileId,
    capture_token: &str,
) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let mut state = lock(&shared.state);
    let Some(entry) = state.entries.get_mut(&id) else {
        return;
    };
    if entry.summary.generation != generation
        || entry.provider_session_process.is_some()
        || entry.provider_session_profile != Some(profile_id)
        || entry.provider_session_capture_token.as_deref() != Some(capture_token)
    {
        return;
    }
    entry.clear_provider_capture();
    entry.provider_session_replacing = false;
}

pub(super) fn capture_checkpoint_session(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    profile_id: TerminalLaunchProfileId,
    process: &TerminalProcessIdentity,
    capture_token: &str,
) -> Option<TerminalLaunchSessionV1> {
    let shared = shared.upgrade()?;
    let state = lock(&shared.state);
    let entry = state.entries.get(&id)?;
    (entry.summary.generation == generation
        && entry.provider_session_process.as_ref() == Some(process)
        && entry.provider_session_capture_token.as_deref() == Some(capture_token)
        && entry.provider_session_profile == Some(profile_id))
    .then(|| entry.summary.launch_session.clone())
    .flatten()
}

pub(super) fn apply_provider_session_binding(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    process_reaped: bool,
    profile_id: TerminalLaunchProfileId,
    process: &TerminalProcessIdentity,
    provider_session: &TerminalProviderSession,
) -> TerminalProviderSessionPersistence {
    if !is_provider_session_id(&provider_session.id) {
        return TerminalProviderSessionPersistence::RetryableFailure;
    }
    let Some(shared) = shared.upgrade() else {
        return TerminalProviderSessionPersistence::RetryableFailure;
    };
    let event_gate = lock(&shared.event_gate);
    let (event, effective_summary) = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return TerminalProviderSessionPersistence::RetryableFailure;
        };
        let manual_shell = entry.manual_capture();
        if entry.summary.generation != generation
            || !entry.is_live()
            || (entry.provider_session_finalizing && !process_reaped)
            || entry.provider_session_process.as_ref() != Some(process)
            || entry.provider_session_profile != Some(profile_id)
            || (!manual_shell
                && entry
                    .summary
                    .launch
                    .is_none_or(|launch| launch.profile_id != profile_id))
        {
            return TerminalProviderSessionPersistence::RetryableFailure;
        }
        let manual_cwd = entry.provider_session_cwd.clone();
        match bind_provider_session(
            entry.binding_target(),
            profile_id,
            provider_session,
            manual_cwd,
        ) {
            Binding::Changed => {
                let summary = entry.summary.clone();
                state.revision = state.revision.saturating_add(1);
                (
                    Some(TerminalLifecycleEvent::Upsert {
                        revision: state.revision,
                        terminal: summary.clone(),
                    }),
                    summary,
                )
            }
            Binding::Unchanged => (None, entry.summary.clone()),
            // Keep the capture and keep polling.
            Binding::Refused => return TerminalProviderSessionPersistence::RetryableFailure,
        }
    };
    let persistence = shared.events.persist_provider_reopen(&effective_summary);
    if let Some(event) = event {
        shared.events.publish(event);
    }
    drop(event_gate);
    persistence
}

/// What one provider-proven session did to a generation's reopen binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Binding {
    /// The binding now names the session; the caller bumps its revision and
    /// publishes the summary.
    Changed,
    /// The existing resumable binding stands.
    Unchanged,
    /// Nothing may be filed: the receipt is not accepted and there is no
    /// binding to keep, or a manual promotion or replacement has no validated
    /// cwd. The capture stays for a retry or startup recovery.
    Refused,
}

/// The one rule for adopting a provider-proven session as a generation's
/// reopen binding, shared by the live and final lookups, restart
/// settlement and shutdown. An armed login shell whose wrapper ran a provider
/// is promoted to that provider, and a replacement nested process takes over
/// the binding; either is reopenable only as the complete
/// provider/session/cwd tuple, so it needs `manual_cwd`. Otherwise a session
/// replaces the binding only as [`provider_session_should_replace`] allows.
/// The caller has checked the session id's shape.
pub(super) fn bind_provider_session(
    target: BindingTarget<'_>,
    profile_id: TerminalLaunchProfileId,
    session: &TerminalProviderSession,
    manual_cwd: Option<PathBuf>,
) -> Binding {
    let BindingTarget {
        summary,
        cwd,
        replacing: replacing_flag,
        manual,
    } = target;
    let manual_profile = manual_provider_profile(profile_id, &session.id);
    let promote = manual
        && (summary.launch_session.is_none()
            || summary
                .launch
                .is_none_or(|launch| launch.profile_id != manual_profile));
    let replacing = manual && *replacing_flag;
    if !(promote
        || replacing
        || provider_session_should_replace(summary.launch_session.as_ref(), session))
    {
        // An unaccepted receipt without an existing resumable binding is
        // ambiguous and must remain recoverable.
        return if summary.launch_session.is_some() {
            Binding::Unchanged
        } else {
            Binding::Refused
        };
    }
    if promote || replacing {
        let Some(manual_cwd) = manual_cwd else {
            return Binding::Refused;
        };
        if promote {
            summary.launch = Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id: manual_profile,
            });
        }
        summary.cwd = manual_cwd.to_string_lossy().into_owned();
        *cwd = manual_cwd;
    }
    summary.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some(session.id.clone()),
        resume: true,
    });
    *replacing_flag = false;
    summary.updated_at = now();
    Binding::Changed
}

/// The entry fields a binding decision reads and writes, borrowed one by one
/// so shutdown can bind an entry whose resources it has already moved out.
pub(super) struct BindingTarget<'a> {
    pub(super) summary: &'a mut TerminalSummary,
    pub(super) cwd: &'a mut PathBuf,
    pub(super) replacing: &'a mut bool,
    /// The capture is an armed login shell's.
    pub(super) manual: bool,
}

impl TerminalEntry {
    pub(super) fn binding_target(&mut self) -> BindingTarget<'_> {
        BindingTarget {
            manual: self.manual_capture(),
            summary: &mut self.summary,
            cwd: &mut self.cwd,
            replacing: &mut self.provider_session_replacing,
        }
    }
}

pub(super) fn provider_session_should_replace(
    existing: Option<&TerminalLaunchSessionV1>,
    candidate: &TerminalProviderSession,
) -> bool {
    if !is_provider_session_id(&candidate.id) {
        return false;
    }
    if existing.is_some_and(|session| {
        session.version == 1
            && session.resume
            && session.id.as_deref() == Some(candidate.id.as_str())
    }) {
        return false;
    }
    // Claude can emit a second provisional `startup` hook while an exact
    // resume or picker selection is still converging. Never let that
    // lower-authority transition replace a known durable resume binding. The
    // provider's subsequent `resume` hook is the authoritative selection.
    // Codex has no provisional startup: its `startup` receipt is the thread
    // this pane opened, so a resume binding whose thread is gone (the pane
    // launched fresh) follows it instead of pointing at nothing forever.
    candidate.transition != TerminalProviderSessionTransition::Startup
        || existing.is_none_or(|session| !session.resume)
        || canonical_uuid(&candidate.id, b'7')
        || crate::native_checkpoint::session_id(TerminalLaunchProfileId::Gemini, &candidate.id)
            .is_some()
        || crate::native_checkpoint::session_id(TerminalLaunchProfileId::Muse, &candidate.id)
            .is_some()
        || crate::native_checkpoint::session_id(TerminalLaunchProfileId::Kimi, &candidate.id)
            .is_some()
}
