//! Provider ingress and the per-session event pump: the byte-bounded
//! forwarder, persisting each event before it is published, the fail-closed
//! stream-terminal and quarantine transitions, and the history/registry
//! persistence helpers the open and delivery paths share.

use std::sync::{Arc, atomic::Ordering};

use interface_contracts::engine::{EngineEvent, EngineEventKind, SessionStatus, SessionSummary};
use interface_engine::{
    BoundedEventStream, DeliveryOutcome, EventStreamError, event_budget::BufferedEvent,
};
use tokio::sync::mpsc;

use super::{
    LIVE_CLOSED, LIVE_OUTCOME_UNKNOWN, LIVE_PERSISTENCE_FAILED, LiveSession, RuntimeClock,
    RuntimeEvent, RuntimeEventFrame, RuntimeEventTermination, RuntimeInner, RuntimeServices,
    RuntimeSession, blocking, lock, registry_error, remove_if_current,
};
use crate::{
    HistoryEvent, RuntimeEffectKind, RuntimeError, RuntimeStreamState,
    history::apply_provider_meta, project_root::ProjectRootProjector,
};

pub(super) const PROVIDER_INGRESS_BYTES: usize = 32 * 1024 * 1024;

pub(super) enum ProviderIngress {
    Event(EngineEvent, Option<tokio::sync::OwnedSemaphorePermit>),
    Closed,
    Overflow { capacity: usize },
    ByteBudget { bytes: usize },
}

pub(super) async fn forward_provider_events(
    mut stream: BoundedEventStream<EngineEvent>,
    sender: mpsc::Sender<ProviderIngress>,
) {
    let budget = Arc::new(tokio::sync::Semaphore::new(PROVIDER_INGRESS_BYTES));
    loop {
        let inbound = match stream.recv().await {
            Ok(Some(event)) => {
                let bytes = event.payload_bytes();
                if bytes > PROVIDER_INGRESS_BYTES {
                    let _ = sender
                        .send(ProviderIngress::ByteBudget {
                            bytes: PROVIDER_INGRESS_BYTES,
                        })
                        .await;
                    break;
                }
                let Ok(permit) = budget
                    .clone()
                    .acquire_many_owned(u32::try_from(bytes.max(1)).expect("bounded ingress bytes"))
                    .await
                else {
                    break;
                };
                ProviderIngress::Event(event, Some(permit))
            }
            Ok(None) => ProviderIngress::Closed,
            Err(EventStreamError::Overflow { capacity }) => ProviderIngress::Overflow { capacity },
            Err(EventStreamError::ByteBudget { bytes }) => ProviderIngress::ByteBudget { bytes },
        };
        let terminal = !matches!(inbound, ProviderIngress::Event(..));
        if sender.send(inbound).await.is_err() || terminal {
            break;
        }
    }
}

fn subscriber_overflow_message(overflow: &EventStreamError) -> String {
    match overflow {
        EventStreamError::Overflow { capacity } => {
            format!("runtime event subscriber exceeded its {capacity}-event capacity")
        }
        EventStreamError::ByteBudget { bytes } => {
            format!(
                "runtime event subscriber control events exceeded their {bytes}-byte payload budget"
            )
        }
    }
}

fn ingress_overflow_message(overflow: &ProviderIngress) -> String {
    match overflow {
        ProviderIngress::Overflow { capacity } => {
            format!("provider event stream exceeded its {capacity}-event capacity")
        }
        ProviderIngress::ByteBudget { bytes } => {
            format!("provider control events exceeded their {bytes}-byte payload budget")
        }
        _ => unreachable!("overflow arm"),
    }
}

pub(super) async fn run_event_pump(
    services: RuntimeServices,
    project_roots: Arc<ProjectRootProjector>,
    clock: Arc<dyn RuntimeClock>,
    runtime: std::sync::Weak<RuntimeInner>,
    live: Arc<LiveSession>,
    mut ingress: mpsc::Receiver<ProviderIngress>,
) {
    while let Some(inbound) = ingress.recv().await {
        match inbound {
            ProviderIngress::Event(event, _permit) => {
                if let EngineEventKind::SessionMeta { session_id, .. } = &event.event {
                    if session_id != &live.descriptor.session_id {
                        quarantine_live(
                            &services,
                            &clock,
                            &runtime,
                            &live,
                            "provider changed its definitive session identifier".into(),
                        )
                        .await;
                        break;
                    }
                }
                let received_at = event.at.clone().unwrap_or_else(|| clock.now());
                match persist_event(
                    services.clone(),
                    Arc::clone(&project_roots),
                    Arc::clone(&live.descriptor),
                    event,
                    received_at,
                )
                .await
                {
                    Ok(stored) => {
                        let emitted = live.subscriber.emit(RuntimeEventFrame::Event {
                            value: RuntimeEvent {
                                session_key: live.descriptor.key.clone(),
                                sequence: stored.sequence,
                                received_at: stored.received_at,
                                event: stored.event,
                            },
                        });
                        // A consumer that stopped reading is not a failure,
                        // but one whose queue overflowed has lost control
                        // events (an approval, a status): the provider must
                        // not keep acting unseen, so the session fails closed.
                        if emitted.is_err()
                            && let Some(overflow) = live.subscriber.overflow()
                        {
                            let message = subscriber_overflow_message(&overflow);
                            finish_stream(&services, &clock, &runtime, &live, Some(message)).await;
                            break;
                        }
                    }
                    Err(error) => {
                        quarantine_live(&services, &clock, &runtime, &live, error.message).await;
                        break;
                    }
                }
            }
            ProviderIngress::Closed => {
                finish_stream(&services, &clock, &runtime, &live, None).await;
                break;
            }
            overflow @ (ProviderIngress::Overflow { .. } | ProviderIngress::ByteBudget { .. }) => {
                let message = ingress_overflow_message(&overflow);
                finish_stream(&services, &clock, &runtime, &live, Some(message)).await;
                break;
            }
        }
    }
}

/// Ends the pump after the provider stream closed (`overflow` is `None`), or
/// it or the subscriber's queue overflowed (`Some(message)`): records the
/// terminal stream state, closes and evicts the attachment, tells the
/// subscriber why, closes it and flushes the registry cache. Only a clean
/// close's record is load-bearing; if it fails the attachment is quarantined
/// instead. An overflowed stream has already failed, so its record is
/// best-effort.
async fn finish_stream(
    services: &RuntimeServices,
    clock: &Arc<dyn RuntimeClock>,
    runtime: &std::sync::Weak<RuntimeInner>,
    live: &Arc<LiveSession>,
    overflow: Option<String>,
) {
    let (stream_state, live_state, reason) = if overflow.is_some() {
        (
            RuntimeStreamState::Overflow,
            LIVE_PERSISTENCE_FAILED,
            RuntimeEventTermination::ProviderOverflow,
        )
    } else {
        (
            RuntimeStreamState::Closed,
            LIVE_CLOSED,
            RuntimeEventTermination::Closed,
        )
    };
    let recorded = persist_stream_terminal(
        services.clone(),
        Arc::clone(&live.descriptor),
        stream_state,
        overflow.clone(),
        clock.now(),
    )
    .await;
    if let Err(error) = recorded
        && overflow.is_none()
    {
        quarantine_live(services, clock, runtime, live, error.message).await;
        return;
    }
    close_and_evict_terminal_attachment(runtime, live, live_state, false).await;
    let _ = live.subscriber.emit(RuntimeEventFrame::Terminal {
        session_key: live.descriptor.key.clone(),
        reason,
        message: overflow,
    });
    live.subscriber.close();
    services.flush_registry_cache().await;
}

async fn close_and_evict_terminal_attachment(
    runtime: &std::sync::Weak<RuntimeInner>,
    live: &Arc<LiveSession>,
    terminal_state: u8,
    preserve_unknown: bool,
) {
    {
        let _serial = live.serial_for_close().await;
        let _dispatch = live.dispatch.lock().await;
        let state = live.state.load(Ordering::Acquire);
        if state != LIVE_CLOSED {
            if !(preserve_unknown && state == LIVE_OUTCOME_UNKNOWN) {
                live.state.store(terminal_state, Ordering::Release);
            }
            let _ = live.handle.close().await;
        }
    }
    let Some(runtime) = runtime.upgrade() else {
        return;
    };
    let mut attachments = runtime.live.write().await;
    remove_if_current(&mut attachments, &live.descriptor.key, live);
}

async fn quarantine_live(
    services: &RuntimeServices,
    clock: &Arc<dyn RuntimeClock>,
    runtime: &std::sync::Weak<RuntimeInner>,
    live: &Arc<LiveSession>,
    message: String,
) {
    let _ = mark_persistence_failed(
        services.clone(),
        live.descriptor.key.clone(),
        message.clone(),
        clock.now(),
    )
    .await;
    close_and_evict_terminal_attachment(runtime, live, LIVE_PERSISTENCE_FAILED, true).await;
    let _ = live.subscriber.emit(RuntimeEventFrame::Terminal {
        session_key: live.descriptor.key.clone(),
        reason: RuntimeEventTermination::PersistenceFailed,
        message: Some(message),
    });
    live.subscriber.close();
}

pub(super) async fn persist_registration(
    services: RuntimeServices,
    project_roots: Arc<ProjectRootProjector>,
    key: String,
    mut summary: SessionSummary,
    at: String,
) -> Result<(), RuntimeError> {
    let persistence = services.clone();
    let project_root_services = services.clone();
    let project_root_projection = Arc::clone(&project_roots);
    let unresolved = blocking(move || {
        let mut history = lock(&services.history, "engine history")?;
        let mut registry = lock(&services.sessions, "session registry")?;
        let engine = summary.engine.clone();
        let unresolved =
            project_root_projection.prepare(&registry, &engine, std::slice::from_mut(&mut summary));
        history.register_session(key, summary.clone(), at)?;
        let revision = registry.revision();
        registry
            .upsert_all(revision, &engine, [summary])
            .map_err(registry_error)?;
        Ok(unresolved)
    })
    .await?;
    persistence.schedule_registry_cache_save();
    project_roots.schedule(&project_root_services, unresolved);
    Ok(())
}

async fn persist_event(
    services: RuntimeServices,
    project_roots: Arc<ProjectRootProjector>,
    descriptor: Arc<RuntimeSession>,
    event: EngineEvent,
    received_at: String,
) -> Result<HistoryEvent, RuntimeError> {
    let persistence = services.clone();
    let project_root_services = services.clone();
    let project_root_projection = Arc::clone(&project_roots);
    let (stored, unresolved, content_changed) = blocking(move || {
        let stored = lock(&services.history, "engine history")?.append_event(
            &descriptor.key,
            event,
            received_at,
        )?;
        let mut registry = lock(&services.sessions, "session registry")?;
        let content_before = registry.content_revision();
        let revision = registry.revision();
        let revision = registry
            .apply_event(
                revision,
                &descriptor.key,
                &stored.event,
                stored.received_at.as_str(),
            )
            .map_err(registry_error)?;
        if matches!(
            &stored.event.event,
            EngineEventKind::SessionMeta { .. } | EngineEventKind::ConfigChanged { .. }
        ) && let Some(mut summary) = registry.get(&descriptor.key).cloned()
        {
            // The registry row, unlike the history projection, re-resolves
            // its project root when the provider reports a new cwd.
            if let EngineEventKind::SessionMeta { cwd, .. } = &stored.event.event
                && summary.cwd != *cwd
            {
                summary.project_root = None;
            }
            if apply_provider_meta(&mut summary, &stored.event.event) {
                let unresolved = project_root_projection.prepare(
                    &registry,
                    &descriptor.engine,
                    std::slice::from_mut(&mut summary),
                );
                registry
                    .upsert_all(revision, &descriptor.engine, [summary])
                    .map_err(registry_error)?;
                return Ok((stored, unresolved, true));
            }
        } else if matches!(&stored.event.event, EngineEventKind::SessionClosed) {
            let idle = EngineEvent {
                at: stored.event.at.clone(),
                event: EngineEventKind::StatusChanged {
                    status: SessionStatus::Idle,
                },
            };
            registry
                .apply_event(
                    revision,
                    &descriptor.key,
                    &idle,
                    stored.received_at.as_str(),
                )
                .map_err(registry_error)?;
        }
        let content_changed = registry.content_revision() != content_before
            || matches!(
                &stored.event.event,
                EngineEventKind::SessionMeta { .. }
                    | EngineEventKind::ConfigChanged { .. }
                    | EngineEventKind::SessionClosed
            );
        Ok((stored, Vec::new(), content_changed))
    })
    .await?;
    // Streaming moves only last_activity on almost every event; rewriting and
    // fsyncing the whole cache for that once a second held the registry lock
    // every pump needs. Anything more than a timestamp still saves within 1 s.
    if content_changed {
        persistence.schedule_registry_cache_save();
    } else {
        persistence.schedule_registry_activity_save();
    }
    project_roots.schedule(&project_root_services, unresolved);
    Ok(stored)
}

pub(super) async fn persist_effect(
    services: RuntimeServices,
    key: String,
    effect_id: String,
    kind: RuntimeEffectKind,
    completed_at: String,
    outcome: DeliveryOutcome<()>,
) -> Result<(), RuntimeError> {
    blocking(move || {
        lock(&services.history, "engine history")?
            .append_effect(&key, effect_id, kind, completed_at, &outcome)
            .map(|_| ())
    })
    .await
}

async fn persist_stream_terminal(
    services: RuntimeServices,
    descriptor: Arc<RuntimeSession>,
    state: RuntimeStreamState,
    message: Option<String>,
    at: String,
) -> Result<(), RuntimeError> {
    let persistence = services.clone();
    blocking(move || {
        lock(&services.history, "engine history")?.mark_stream_terminal(
            &descriptor.key,
            state,
            message,
            at.clone(),
        )?;
        let mut registry = lock(&services.sessions, "session registry")?;
        let revision = registry.revision();
        let status = match state {
            RuntimeStreamState::Active | RuntimeStreamState::Closed => SessionStatus::Idle,
            RuntimeStreamState::Overflow => SessionStatus::Failed,
        };
        let projection = EngineEvent {
            at: Some(at.clone()),
            event: if status == SessionStatus::Failed {
                EngineEventKind::Error {
                    cause: None,
                    message: "provider event stream overflowed".into(),
                    retryable: false,
                }
            } else {
                EngineEventKind::StatusChanged { status }
            },
        };
        registry
            .apply_event(revision, &descriptor.key, &projection, at)
            .map_err(registry_error)?;
        Ok(())
    })
    .await?;
    persistence.schedule_registry_cache_save();
    Ok(())
}

pub(super) async fn mark_persistence_failed(
    services: RuntimeServices,
    key: String,
    message: String,
    at: String,
) -> Result<(), RuntimeError> {
    blocking(move || {
        lock(&services.history, "engine history")?.mark_persistence_failed(&key, message, at)
    })
    .await
}
