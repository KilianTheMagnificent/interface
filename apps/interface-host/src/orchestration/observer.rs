//! Agent lifecycle observation: the per-agent event pump, desktop registry
//! and event pushes, observed message processing and settlement.

use std::sync::atomic::Ordering;

use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    orchestration::{AgentNodeState, MessageDeliveryState, OrchestrationMutation},
};
use interface_engine::BoundedEventStream;
use interface_orchestration::bounded_utf8;
use interface_runtime::{RuntimeEventFrame, RuntimeEventTermination};

use crate::{
    dispatcher::{
        DispatchError, OVERFLOW_DETACH_WAIT, session_event_frame, settled_session_activity,
        wait_for_detach,
    },
    registry_push::registry_push_frame,
};

use super::adversarial::adversarial_review_message_key;
use super::{
    ADVERSARIAL_REVIEWER_ROLE_PREFIX, OrchestrationHostDispatcher, agent_is_settleable,
    store_lock_error,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ObservedCompletion {
    Idle,
    Failed,
    Unknown,
}

impl OrchestrationHostDispatcher {
    pub(super) async fn push_desktop_registry(&self) {
        let Some(pushes) = &self.pushes else {
            return;
        };
        let sessions = match self.runtime.desktop_session_summaries() {
            Ok(Some(sessions)) => sessions,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(
                    error = %error.message,
                    "orchestration session registry projection failed"
                );
                return;
            }
        };
        let Ok(frame) = registry_push_frame(&sessions) else {
            tracing::error!("orchestration session registry push could not be framed");
            return;
        };
        if pushes.send(frame).await.is_err() {
            tracing::warn!("orchestration session registry push channel closed");
        }
    }

    async fn push_desktop_event(&self, session_key: &str, event: &EngineEvent) {
        // Mirror the event into the live collaboration transcript while
        // orchestration stays the sole consumer of the provider stream.
        if let Some(projection) = &self.projection
            && let Err(error) = projection.observe_event(session_key, event)
        {
            tracing::error!(
                session_key,
                code = %error.code,
                "orchestration desktop transcript projection failed"
            );
        }
        let Some(pushes) = &self.pushes else {
            return;
        };
        let Ok(frame) = session_event_frame(session_key, event) else {
            tracing::error!(
                session_key,
                "orchestration session event could not be framed"
            );
            return;
        };
        if pushes.send(frame).await.is_err() {
            tracing::warn!(
                session_key,
                "orchestration session event push channel closed"
            );
            return;
        }
        let closing = matches!(event.event, EngineEventKind::SessionClosed);
        if !closing
            && !matches!(
                event.event,
                EngineEventKind::SessionMeta { .. } | EngineEventKind::StatusChanged { .. }
            )
        {
            return;
        }
        match &self.registry_pushes {
            // A close's final registry must not sit behind a debounce window
            // that host teardown can cancel.
            Some(coordinator) if closing => {
                let _ = coordinator.flush().await;
            }
            Some(coordinator) => {
                let _ = coordinator.schedule();
            }
            None => self.push_desktop_registry().await,
        }
    }

    pub(super) async fn project_desktop_terminal_close(&self, session_key: &str) {
        if !matches!(
            settled_session_activity(
                || self.runtime.desktop_session_is_active(session_key),
                session_key,
                "could not prove orchestration session attachment closure",
            )
            .await,
            Some(false)
        ) {
            return;
        }
        let closed = EngineEvent {
            at: Some(self.clock.now()),
            event: EngineEventKind::SessionClosed,
        };
        if let Err(error) = self.runtime.project_desktop_event(session_key, &closed) {
            tracing::error!(
                session_key,
                error = %error.message,
                "could not project orchestration session closure"
            );
        }
        self.push_desktop_event(session_key, &closed).await;
    }

    pub(super) async fn observe_agent(&self, agent_id: String, session_key: String) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let stream = match self.runtime.take_events(&session_key).await {
            Ok(Some(stream)) => stream,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    agent_id,
                    session_key,
                    error = %error.message,
                    "orchestration agent event observation could not start"
                );
                let dispatcher = self.clone();
                let observer = tokio::spawn(async move {
                    dispatcher
                        .settle_observed_agent(
                            &agent_id,
                            ObservedCompletion::Unknown,
                            "Agent completion could not be observed.".into(),
                            None,
                        )
                        .await;
                });
                let mut observers = self.observer_tasks.lock().await;
                observers.retain(|handle| !handle.is_finished());
                observers.push(observer);
                return;
            }
        };
        let dispatcher = self.clone();
        let observer = tokio::spawn(async move {
            dispatcher
                .consume_agent_events(agent_id, session_key, stream)
                .await;
        });
        let mut observers = self.observer_tasks.lock().await;
        observers.retain(|handle| !handle.is_finished());
        observers.push(observer);
    }

    #[allow(clippy::too_many_lines)]
    async fn consume_agent_events(
        &self,
        agent_id: String,
        session_key: String,
        mut stream: BoundedEventStream<RuntimeEventFrame>,
    ) {
        let mut saw_activity = false;
        loop {
            let frame = tokio::select! {
                () = self.observer_cancel.cancelled() => {
                    self.forget_processing_messages(&agent_id);
                    return;
                }
                frame = stream.recv() => frame,
            };
            let completion = match frame {
                Ok(Some(RuntimeEventFrame::Event { mut value })) => {
                    if matches!(value.event.event, EngineEventKind::SessionClosed) {
                        // This is an adapter-internal marker. The canonical
                        // desktop close is emitted only after the terminal
                        // frame proves the live attachment was removed.
                        continue;
                    }
                    value.event.at = Some(value.received_at);
                    self.push_desktop_event(&value.session_key, &value.event)
                        .await;
                    match value.event.event {
                        EngineEventKind::StatusChanged {
                            status: SessionStatus::Working,
                        }
                        | EngineEventKind::MessageDelta { .. }
                        | EngineEventKind::ToolCall { .. }
                        | EngineEventKind::ThinkingSignal { .. }
                        | EngineEventKind::UserMessage { .. } => {
                            saw_activity = true;
                            None
                        }
                        EngineEventKind::StatusChanged {
                            status: SessionStatus::Idle,
                        } => Some((
                            ObservedCompletion::Idle,
                            "Agent became idle and is awaiting result review.".to_owned(),
                        )),
                        EngineEventKind::StatusChanged {
                            status: SessionStatus::Failed,
                        }
                        | EngineEventKind::Error {
                            retryable: false, ..
                        } => Some((
                            ObservedCompletion::Failed,
                            "Agent reported a terminal provider failure.".to_owned(),
                        )),
                        _ => None,
                    }
                }
                Ok(Some(RuntimeEventFrame::Terminal {
                    reason: RuntimeEventTermination::Closed,
                    session_key,
                    message,
                    ..
                })) => {
                    self.project_desktop_terminal_close(&session_key).await;
                    Some((
                        ObservedCompletion::Unknown,
                        message.unwrap_or_else(|| {
                            "Agent provider stream closed before an idle result was observed."
                                .to_owned()
                        }),
                    ))
                }
                Ok(Some(RuntimeEventFrame::Terminal {
                    reason:
                        RuntimeEventTermination::ProviderOverflow
                        | RuntimeEventTermination::PersistenceFailed,
                    session_key,
                    message,
                    ..
                })) => {
                    self.project_desktop_terminal_close(&session_key).await;
                    Some((
                        ObservedCompletion::Unknown,
                        message.unwrap_or_else(|| {
                            "Agent completion could not be persisted or reconciled.".to_owned()
                        }),
                    ))
                }
                Ok(None) => {
                    self.project_desktop_terminal_close(&session_key).await;
                    Some((
                        ObservedCompletion::Unknown,
                        "Agent event stream ended before completion could be reconciled."
                            .to_owned(),
                    ))
                }
                Err(error) => {
                    // The runtime fails an overflowed session closed, but the
                    // stream reports the overflow before the eviction.
                    wait_for_detach(
                        || self.runtime.desktop_session_is_active(&session_key),
                        OVERFLOW_DETACH_WAIT,
                    )
                    .await;
                    self.project_desktop_terminal_close(&session_key).await;
                    Some((
                        ObservedCompletion::Unknown,
                        format!("Agent event stream failed: {error}."),
                    ))
                }
            };
            let Some((completion, fallback_summary)) = completion else {
                continue;
            };
            if completion == ObservedCompletion::Idle && !saw_activity {
                let active = self.snapshot().is_ok_and(|snapshot| {
                    snapshot.agents.iter().any(|agent| {
                        agent.agent_id == agent_id && agent.state == AgentNodeState::Working
                    })
                });
                if !active {
                    continue;
                }
            }
            let transcript = if matches!(
                completion,
                ObservedCompletion::Idle | ObservedCompletion::Failed
            ) {
                self.agent_transcript(&session_key, 400)
                    .await
                    .ok()
                    .filter(|lines| !lines.is_empty())
            } else {
                None
            };
            let summary = transcript.as_ref().and_then(|lines| lines.last()).map_or(
                fallback_summary,
                |line| {
                    bounded_utf8(
                        &line.replace('\0', ""),
                        interface_contracts::orchestration::ORCHESTRATION_LIMITS.short_text_bytes,
                    )
                },
            );
            let transcript = transcript.map(|lines| lines.join("\n"));
            self.settle_observed_agent(&agent_id, completion, summary, transcript)
                .await;
            if completion != ObservedCompletion::Idle {
                self.forget_processing_messages(&agent_id);
                return;
            }
            saw_activity = false;
        }
    }

    fn forget_processing_messages(&self, agent_id: &str) {
        let _ = self
            .processing_messages
            .lock()
            .map(|mut map| map.remove(agent_id));
    }

    async fn mark_observed_messages_processed(&self, agent_id: &str) -> Result<(), DispatchError> {
        let pending = self
            .processing_messages
            .lock()
            .map_err(|_| store_lock_error())?
            .remove(agent_id)
            .unwrap_or_default();
        if pending.is_empty() {
            return Ok(());
        }
        let snapshot = self.snapshot()?;
        let settled = pending.into_iter().filter(|message_id| {
            snapshot.messages.iter().any(|message| {
                &message.message_id == message_id
                    && message
                        .receipts
                        .last()
                        .is_some_and(|receipt| receipt.state == MessageDeliveryState::Settled)
            })
        });
        for message_id in settled {
            self.append_receipt(
                &message_id,
                MessageDeliveryState::Processed,
                "A causally observed provider turn reached an idle or failed state.",
            )
            .await?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn settle_observed_agent(
        &self,
        agent_id: &str,
        completion: ObservedCompletion,
        summary: String,
        transcript: Option<String>,
    ) {
        let _guard = self.gate.lock().await;
        if self.ensure_open().is_err() {
            return;
        }
        let result: Result<(), DispatchError> = async {
            let snapshot = self.snapshot()?;
            let Some(agent) = snapshot
                .agents
                .iter()
                .find(|agent| agent.agent_id == agent_id)
                .cloned()
            else {
                return Ok(());
            };
            if !agent_is_settleable(agent.state) {
                return Ok(());
            }
            if completion != ObservedCompletion::Idle && agent.state == AgentNodeState::Idle {
                // A later stream-close edge cannot overwrite a turn whose idle
                // result was already reconciled. A new send first moves the
                // agent back to working, arming observation for that turn.
                return Ok(());
            }
            if matches!(
                completion,
                ObservedCompletion::Idle | ObservedCompletion::Failed
            ) {
                self.mark_observed_messages_processed(agent_id).await?;
            }
            let state = match completion {
                ObservedCompletion::Idle
                    if agent
                        .role
                        .as_deref()
                        .and_then(|role| role.strip_prefix(ADVERSARIAL_REVIEWER_ROLE_PREFIX))
                        .is_some_and(|pair_token| {
                            !snapshot.effects.iter().any(|effect| {
                                effect.idempotency_key == adversarial_review_message_key(pair_token)
                            })
                        }) =>
                {
                    AgentNodeState::Waiting
                }
                ObservedCompletion::Idle => AgentNodeState::Idle,
                ObservedCompletion::Failed => AgentNodeState::Failed,
                ObservedCompletion::Unknown => AgentNodeState::Unknown,
            };
            self.apply(OrchestrationMutation::SetAgentState {
                agent_id: agent_id.into(),
                state,
                session_key: None,
                provider_session_id: None,
                meta: self.meta(summary),
            })
            .await?;
            if completion == ObservedCompletion::Idle {
                self.advance_adversarial_review(
                    agent_id,
                    transcript.as_deref().unwrap_or_default(),
                )
                .await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            tracing::error!(
                agent_id,
                error = %error.message,
                "failed to durably settle an observed orchestration agent"
            );
        }
    }
}
