//! A generation's process tasks and teardown: the PTY reader and the waiter
//! that settles the generation once its leader exits, and the teardown that
//! keeps a workspace lease until the process tree is proven gone.
use super::{
    Arc, BTreeMap, Binding, BindingTarget, ErrorKind, Instant, Mutex, OscTitleScanner,
    OutputBuffer, PROCESS_TEARDOWN_RETRY_DELAY, PROCESS_TEARDOWN_RETRY_MAX, PreparedTerminal, Read,
    Shared, SpawnedTerminal, TerminalEntry, TerminalGeneration, TerminalGenerationRequest,
    TerminalId, TerminalLaunchProfileId, TerminalLifecycleEvent, TerminalProcessControl,
    TerminalProviderSessionCaptureProcess, TerminalProviderSessionLookup,
    TerminalProviderSessionPersistence, TerminalResources, TerminalState, UNCLAIMED_OUTPUT_GRACE,
    Weak, apply_auto_title, bind_provider_session, handle_provider_session_wake,
    has_resumable_provider_binding, is_provider_session_id, lock, mark_terminal_closing, mpsc, now,
    refresh_final_provider_session_binding, seal_nested_provider_for_terminal, thread,
    validated_manual_provider_cwd, workspace_sleep::WorkspacePower,
};
use crate::opencode_activity::TerminalOutputFilter;

pub(super) fn release_terminal_entries(
    shared: &Shared,
    entries: BTreeMap<TerminalId, TerminalEntry>,
    revision: &mut u64,
) {
    for entry in entries.into_values() {
        release_terminal_entry(shared, entry, revision);
    }
}

/// Tears one entry down, then files its final provider receipt. The steps
/// stay in this order; an early `return` files nothing and leaves the capture
/// for startup recovery.
#[allow(clippy::too_many_lines)] // Teardown, final receipt filing, and lease release stay ordered.
fn release_terminal_entry(shared: &Shared, mut entry: TerminalEntry, revision: &mut u64) {
    let manual_capture = entry.manual_capture();
    let attached_provider_process = entry.provider_session_process.clone();
    let resumable_provider_profile = entry
        .provider_session_profile
        .filter(|profile_id| has_resumable_provider_binding(&entry, *profile_id));
    let provider_root = entry.root.clone();
    let teardown_requirement = if manual_capture {
        entry
            .provider_session_profile
            .zip(entry.provider_session_capture_token.clone())
            .map_or(
                TerminalTeardownRequirement::None,
                |(profile_id, capture_token)| TerminalTeardownRequirement::SealedProviderCapture {
                    lookup: Arc::clone(&shared.provider_sessions),
                    profile_id,
                    capture_token,
                    retire_if_processless: entry.provider_session_process.is_none(),
                },
            )
    } else {
        TerminalTeardownRequirement::None
    };
    let sealed_provider = if entry.is_live() || manual_capture {
        match terminate_before_releasing(entry.resources, &teardown_requirement) {
            // A detached retry still owns the live process resources and
            // lease. Its capture generation must survive untouched so
            // startup recovery can reconcile it after teardown;
            // final_session is only valid across a synchronously proven
            // reaped boundary.
            TerminalResourcesSettlement::Pending => return,
            TerminalResourcesSettlement::Settled { sealed_provider } => sealed_provider,
        }
    } else {
        entry.resources.output.close();
        None
    };
    if manual_capture {
        let Some(captured) = sealed_provider else {
            // A processless capture was either retired during teardown or
            // remains sealed for startup recovery. Never fall back to the
            // pre-seal in-memory process snapshot.
            return;
        };
        let Some(profile_id) = entry.provider_session_profile else {
            return;
        };
        if attached_provider_process
            .as_ref()
            .is_some_and(|process| process != &captured.process)
            && resumable_provider_profile == Some(profile_id)
        {
            entry.provider_session_replacing = true;
        }
        entry.provider_session_process = Some(captured.process);
        entry.provider_session_cwd = Some(captured.cwd);
    }
    let manual_provider_cwd = if manual_capture {
        let Some(cwd) = entry
            .provider_session_cwd
            .as_deref()
            .and_then(|cwd| validated_manual_provider_cwd(cwd, &provider_root))
        else {
            // The capture was claim-sealed, but a complete automatic
            // reopen tuple is no longer safe. Leave its receipt for
            // startup recovery rather than filing or retiring it.
            return;
        };
        Some(cwd)
    } else {
        None
    };
    let capture = entry
        .provider_session_profile
        .zip(entry.provider_session_capture_token.clone())
        .zip(entry.provider_session_process.clone());
    if let Some(((profile_id, capture_token), process)) = capture {
        match shared
            .provider_sessions
            .final_session(profile_id, &process, &capture_token)
        {
            Ok(Some(provider_session)) => {
                if !is_provider_session_id(&provider_session.id) {
                    return;
                }
                let target = BindingTarget {
                    summary: &mut entry.summary,
                    cwd: &mut entry.cwd,
                    replacing: &mut entry.provider_session_replacing,
                    manual: manual_capture,
                };
                let mut event = match bind_provider_session(
                    target,
                    profile_id,
                    &provider_session,
                    manual_provider_cwd,
                ) {
                    Binding::Changed => {
                        *revision = revision.saturating_add(1);
                        Some(TerminalLifecycleEvent::Upsert {
                            revision: *revision,
                            terminal: entry.summary.clone(),
                        })
                    }
                    Binding::Unchanged => None,
                    // An ambiguous receipt, or a manual binding whose saved
                    // cwd is no longer safe: preserve the sealed receipt for
                    // startup recovery.
                    Binding::Refused => return,
                };
                // Every path that reaches here has a launch session: it was
                // just bound above, or it already existed.
                let persistence = {
                    let _event_gate = lock(&shared.event_gate);
                    let persistence = shared.events.persist_provider_reopen(&entry.summary);
                    if let Some(event) = event.take() {
                        shared.events.publish(event);
                    }
                    persistence
                };
                if persistence != TerminalProviderSessionPersistence::RetryableFailure
                    && entry
                        .summary
                        .launch_session
                        .as_ref()
                        .is_some_and(|launch_session| {
                            shared
                                .provider_sessions
                                .checkpoint_capture(profile_id, &capture_token, launch_session)
                                .is_ok()
                        })
                {
                    let _ = shared
                        .provider_sessions
                        .end_capture(profile_id, &capture_token);
                }
            }
            Ok(None) => {
                let _ = shared
                    .provider_sessions
                    .end_capture(profile_id, &capture_token);
            }
            Err(_) => {}
        }
    }
}

/// Keep the resources (and, critically, the workspace lease) alive until the
/// provider process tree has actually been torn down. A transient backend
/// failure during final service destruction must not turn into permission for
/// the worktree manager to delete a directory that still has live processes.
fn terminate_before_releasing(
    resources: TerminalResources,
    requirement: &TerminalTeardownRequirement,
) -> TerminalResourcesSettlement {
    resources.output.close();
    let settlement = settle_terminal_resources(&resources, requirement);
    if matches!(settlement, TerminalResourcesSettlement::Settled { .. }) {
        return settlement;
    }

    let pending = Arc::new(Mutex::new(Some(resources)));
    // Shared destruction is the last chance to persist an exit-only provider
    // transition. Keep the resources and workspace lease alive until teardown
    // is proven without blocking the thread that dropped the final service
    // handle (which may itself need to release a backend termination gate).
    let retry = Arc::clone(&pending);
    let background_requirement = requirement.clone();
    if thread::Builder::new()
        .name("interface-terminal-teardown".to_owned())
        .spawn(move || {
            let _ = retry_teardown_until_terminated(&retry, &background_requirement);
        })
        .is_err()
    {
        // Thread creation failure must not drop the last lease while a child
        // may still be alive. The rare fallback is deliberately synchronous.
        TerminalResourcesSettlement::Settled {
            sealed_provider: retry_teardown_until_terminated(&pending, requirement),
        }
    } else {
        TerminalResourcesSettlement::Pending
    }
}

enum TerminalResourcesSettlement {
    Pending,
    Settled {
        sealed_provider: Option<TerminalProviderSessionCaptureProcess>,
    },
}

#[derive(Clone)]
enum TerminalTeardownRequirement {
    None,
    SealedProviderCapture {
        lookup: Arc<dyn TerminalProviderSessionLookup>,
        profile_id: TerminalLaunchProfileId,
        capture_token: String,
        retire_if_processless: bool,
    },
}

fn settle_terminal_resources(
    resources: &TerminalResources,
    requirement: &TerminalTeardownRequirement,
) -> TerminalResourcesSettlement {
    if let TerminalTeardownRequirement::SealedProviderCapture {
        lookup,
        profile_id,
        capture_token,
        retire_if_processless,
    } = requirement
    {
        let Ok(captured) = lookup.seal_capture(*profile_id, capture_token) else {
            return TerminalResourcesSettlement::Pending;
        };
        if resources.control.terminate_tree().is_err() {
            return TerminalResourcesSettlement::Pending;
        }
        if captured.as_ref().is_some_and(|captured| {
            resources
                .control
                .process_group_identity_is_live(&captured.process)
                .ok()
                != Some(false)
        }) {
            return TerminalResourcesSettlement::Pending;
        }
        if captured.is_none()
            && *retire_if_processless
            && lookup.end_capture(*profile_id, capture_token).is_err()
        {
            return TerminalResourcesSettlement::Pending;
        }
        return TerminalResourcesSettlement::Settled {
            sealed_provider: captured,
        };
    }
    if resources.control.terminate_tree().is_err() {
        return TerminalResourcesSettlement::Pending;
    }
    TerminalResourcesSettlement::Settled {
        sealed_provider: None,
    }
}

fn retry_teardown_until_terminated(
    pending: &Mutex<Option<TerminalResources>>,
    requirement: &TerminalTeardownRequirement,
) -> Option<TerminalProviderSessionCaptureProcess> {
    let mut delay = PROCESS_TEARDOWN_RETRY_DELAY;
    loop {
        thread::sleep(delay);
        delay = (delay * 2).min(PROCESS_TEARDOWN_RETRY_MAX);
        let settlement = {
            let resources = lock(pending);
            let resources = resources.as_ref()?;
            settle_terminal_resources(resources, requirement)
        };
        if let TerminalResourcesSettlement::Settled { sealed_provider } = settlement {
            drop(lock(pending).take());
            return sealed_provider;
        }
    }
}

pub(super) fn abort_prepared_terminal(prepared: PreparedTerminal) -> bool {
    let PreparedTerminal {
        spawned,
        output,
        lease,
    } = prepared;
    let SpawnedTerminal {
        input,
        control,
        reader: _,
        waiter: _,
    } = spawned;
    matches!(
        terminate_before_releasing(
            TerminalResources {
                input,
                control,
                output,
                lease: Some(lease),
            },
            &TerminalTeardownRequirement::None,
        ),
        TerminalResourcesSettlement::Settled { .. }
    )
}

/// Retries `step` until it succeeds, from `PROCESS_TEARDOWN_RETRY_DELAY`
/// doubling to `PROCESS_TEARDOWN_RETRY_MAX`, or returns false once the
/// service is gone.
fn retry_while_alive(shared: &Weak<Shared>, mut step: impl FnMut() -> bool) -> bool {
    let mut delay = PROCESS_TEARDOWN_RETRY_DELAY;
    loop {
        if step() {
            return true;
        }
        if shared.strong_count() == 0 {
            return false;
        }
        thread::sleep(delay);
        delay = (delay * 2).min(PROCESS_TEARDOWN_RETRY_MAX);
    }
}

/// Waits for the PTY leader, then tears the generation down in order and
/// publishes its exit. Fails closed, publishing nothing, once the service
/// is gone.
pub(super) fn run_process_waiter(
    shared: &Weak<Shared>,
    request: TerminalGenerationRequest,
    waiter: &mut dyn crate::embedded::TerminalProcessWaiter,
    control: &dyn TerminalProcessControl,
    reader_finished: &mpsc::Receiver<()>,
    output: &OutputBuffer,
) {
    let TerminalGenerationRequest { id, generation } = request;
    let result = waiter.wait();
    // Once the PTY leader has exited, this generation cannot return
    // to a reusable login shell. Fence the manual-child observer
    // before teardown so a nested Claude exit cannot race this
    // waiter into publishing a stale Shell demotion.
    mark_terminal_closing(shared, id, generation);
    // Seal any independently claimed foreground owner before the
    // root shell loses the retained PTY identity that fences later
    // wrapper claims. This does not signal the nested group; the
    // finalizer below simply retains state and lease until it exits.
    if !retry_while_alive(shared, || {
        shared.upgrade().is_some_and(|current| {
            seal_nested_provider_for_terminal(&current, id, generation).is_ok()
        })
    }) {
        return;
    }
    // Reaping the PTY leader does not prove that its process group is
    // empty: a provider can exit naturally after leaving descendants
    // behind. Teardown verifies group absence and is deliberately
    // completed before finish_process can publish Exited or release
    // the worktree lease. Keep the waiter alive across transient
    // teardown failures so either it or an explicit close can retry;
    // fail closed without publishing an exit if the service is gone.
    if !retry_while_alive(shared, || control.terminate_tree().is_ok()) {
        return;
    }
    // Wait for PTY EOF so an attached consumer receives every visible
    // exit-tail byte and the lookup worker sees every private wake.
    // If the consumer was never claimed or has been abandoned, close
    // the bounded queue to release a reader blocked on backpressure;
    // reevaluate after each timeout so a later reader drop also
    // unblocks teardown.
    let drain_started = Instant::now();
    loop {
        match reader_finished.recv_timeout(PROCESS_TEARDOWN_RETRY_DELAY) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                output.close_if_unconsumed_backpressure(
                    drain_started.elapsed() >= UNCLAIMED_OUTPUT_GRACE,
                );
            }
        }
    }
    // The lookup worker is independent from PTY draining, so its
    // final wake can still be queued when EOF arrives. Reaping is a
    // trusted boundary established by the service: synchronously
    // consume the last receipt before publishing a terminal state and
    // retiring this generation's capture material.
    // A retained login shell and its foreground Claude command have
    // distinct process groups. Reaping the shell therefore is not a
    // trusted boundary for Claude's final receipt. Keep both the
    // terminal state and its workspace lease live until the exact
    // nested identity is observably gone; liveness probe failures are
    // retryable, never evidence of exit.
    if !retry_while_alive(shared, || {
        refresh_final_provider_session_binding(shared, id, generation)
    }) {
        return;
    }
    finish_process(shared, id, generation, result.ok());
}

/// Reads one generation's PTY through EOF. Private wakes go to the provider
/// lookup, visible bytes to the sleep policy, the title scan and the bounded
/// output queue.
pub(super) fn run_output_reader(
    titles: &Weak<Shared>,
    request: TerminalGenerationRequest,
    reader: &mut dyn Read,
    read_chunk_bytes: usize,
    output_for_reader: &OutputBuffer,
    power: &Mutex<WorkspacePower>,
    mut private_titles: TerminalOutputFilter,
) {
    let TerminalGenerationRequest { id, generation } = request;
    let mut buffer = vec![0_u8; read_chunk_bytes];
    let mut scanner = OscTitleScanner::new();
    let mut announced: Option<String> = None;
    let mut forward_output = true;
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                let filtered = private_titles.feed(&buffer[..read]);
                for payload in filtered.reserved_payloads {
                    handle_provider_session_wake(titles, id, generation, &payload);
                }
                // The power lock is released before the publish takes it again.
                let attention = lock(power).output(&filtered.visible);
                if attention && let Some(shared) = titles.upgrade() {
                    super::workspace_sleep::publish_attention(&shared, id, generation, power);
                }
                for title in scanner.feed(&filtered.visible) {
                    // Agent CLIs repaint an animated title several
                    // times a second. Sanitizing collapses those frames
                    // to one string, and comparing here keeps the
                    // repeats off the service locks entirely.
                    if announced.as_deref() == Some(title.as_str()) {
                        continue;
                    }
                    apply_auto_title(titles, id, generation, &title);
                    announced = Some(title);
                }
                if forward_output
                    && !filtered.visible.is_empty()
                    && !output_for_reader.push(filtered.visible)
                {
                    // Closing or abandoning the public output plane
                    // must not stop the private identity scan. Keep
                    // draining the PTY through EOF so the final
                    // `/resume`, `/clear`, or branch frame is applied
                    // before this generation can become restartable.
                    forward_output = false;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let trailing = private_titles.finish();
    for title in scanner.feed(&trailing) {
        if announced.as_deref() != Some(title.as_str()) {
            apply_auto_title(titles, id, generation, &title);
            announced = Some(title);
        }
    }
    if forward_output && !trailing.is_empty() {
        let _ = output_for_reader.push(trailing);
    }
    output_for_reader.close();
}

fn finish_process(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
    exit: Option<interface_contracts::terminal::TerminalExit>,
) {
    let Some(shared) = shared.upgrade() else {
        return;
    };
    let event_gate = lock(&shared.event_gate);
    let lease = {
        let mut state = lock(&shared.state);
        let Some(entry) = state.entries.get_mut(&id) else {
            return;
        };
        if entry.summary.generation != generation || !entry.is_live() {
            return;
        }
        entry.resources.lease.take()
    };

    // The process is already reaped, but keep its public state Running until
    // the worktree guard has completed its release side effect. This makes it
    // impossible for a snapshot consumer to schedule cleanup based on Exited
    // while the terminal still owns a lease.
    drop(lease);

    let event = {
        let mut state = lock(&shared.state);
        let summary = {
            let Some(entry) = state.entries.get_mut(&id) else {
                return;
            };
            if entry.summary.generation != generation || !entry.is_live() {
                return;
            }
            if let Some(exit) = exit {
                entry.summary.state = TerminalState::Exited;
                entry.summary.exit = Some(exit);
            } else {
                entry.summary.state = TerminalState::Failed;
                entry.summary.exit = None;
            }
            // A finished program no longer waits on its startup files, nor
            // for the user.
            entry.summary.startup_download = None;
            entry.summary.attention = None;
            entry.startup_fetch = None;
            entry.summary.updated_at = now();
            entry.summary.clone()
        };
        state.revision = state.revision.saturating_add(1);
        TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal: summary,
        }
    };
    shared.events.publish(event);
    drop(event_gate);
}
