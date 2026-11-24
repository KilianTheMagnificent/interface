//! Bounded live transcript, approval and delivery projection. No provider effects.
use super::{CollaborationServiceError, MAX_LIVE_TRANSCRIPT_EVENTS};
use interface_collaboration::{
    packet::{MAX_PROJECTED_TEXT_BYTES, bounded_tail, sanitize_text},
    state::CollaborationEngineId,
};
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    ipc::CollaborationDraftStateUpdate,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex as StdMutex, OnceLock},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedSourceState {
    pub queue_revision: u64,
    pub queue_depth: u64,
    pub delivery_revision: u64,
    pub delivery_unknown: bool,
    pub pending_approval_set_hash: String,
    pub pending_approval_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeldPeerProjection {
    pub collaboration_id: String,
    pub queue_entry_id: String,
    pub result_id: String,
    pub result_digest: String,
    pub source_session_key: String,
    pub source_engine: CollaborationEngineId,
    pub target_session_key: String,
    pub target_engine: CollaborationEngineId,
    pub objective: String,
    pub body: String,
}

#[derive(Default)]
struct ProjectionSource {
    queue_revision: u64,
    delivery_revision: u64,
    history_sequence: u64,
    delivery_unknown: bool,
    draft_revision: Option<u64>,
    draft_empty: Option<bool>,
    send_in_flight: bool,
    pending_approvals: HashSet<String>,
    events: VecDeque<EngineEvent>,
}

#[derive(Default)]
struct ProjectionState {
    sources: HashMap<String, ProjectionSource>,
    held: HashMap<String, HashMap<String, HeldPeerProjection>>,
}

#[derive(Default)]
pub struct DesktopCollaborationProjection {
    state: StdMutex<ProjectionState>,
    source_changed: OnceLock<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl DesktopCollaborationProjection {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_source_changed(&self, callback: Arc<dyn Fn(&str) + Send + Sync>) {
        let _ = self.source_changed.set(callback);
    }

    pub fn activate_session(&self, session_key: &str) -> Result<(), CollaborationServiceError> {
        self.lock_state()?
            .sources
            .entry(session_key.to_owned())
            .or_default();
        self.notify(session_key);
        Ok(())
    }

    pub fn set_draft_state(
        &self,
        update: &CollaborationDraftStateUpdate,
    ) -> Result<(), CollaborationServiceError> {
        {
            let mut state = self.lock_state()?;
            let source = state
                .sources
                .entry(update.source_session_key.clone())
                .or_default();
            if source
                .draft_revision
                .is_some_and(|current| update.revision <= current)
            {
                return Err(CollaborationServiceError {
                    code: "collaboration-draft-revision-stale".to_owned(),
                    message: "Collaboration draft revision must increase for this session."
                        .to_owned(),
                    retryable: false,
                    replay_safe: true,
                });
            }
            source.draft_revision = Some(update.revision);
            source.draft_empty = Some(update.empty);
            source.queue_revision = checked_revision(source.queue_revision)?;
        }
        self.notify(&update.source_session_key);
        Ok(())
    }

    pub fn user_send_started(&self, session_key: &str) -> Result<(), CollaborationServiceError> {
        {
            let mut state = self.lock_state()?;
            let source = state.sources.entry(session_key.to_owned()).or_default();
            if source.send_in_flight {
                source.delivery_unknown = true;
                source.send_in_flight = false;
                source.draft_empty = None;
                source.queue_revision = checked_revision(source.queue_revision)?;
                source.delivery_revision = checked_revision(source.delivery_revision)?;
                return Err(CollaborationServiceError {
                    code: "collaboration-source-overlap".to_owned(),
                    message: "Collaboration source projection observed overlapping sends."
                        .to_owned(),
                    retryable: false,
                    replay_safe: false,
                });
            }
            source.send_in_flight = true;
            source.draft_empty = None;
            source.queue_revision = checked_revision(source.queue_revision)?;
        }
        self.notify(session_key);
        Ok(())
    }

    pub fn user_send_settled(
        &self,
        session_key: &str,
        text: &str,
        outcome: ProjectedSendOutcome,
    ) -> Result<(), CollaborationServiceError> {
        {
            let mut state = self.lock_state()?;
            let source = state.sources.entry(session_key.to_owned()).or_default();
            source.send_in_flight = false;
            match outcome {
                ProjectedSendOutcome::AdapterAccepted => {
                    append_projected(
                        source,
                        EngineEvent {
                            at: None,
                            event: EngineEventKind::UserMessage {
                                text: bounded_text(text),
                                agent_key: None,
                                images: None,
                                image_references: None,
                            },
                        },
                    )?;
                }
                ProjectedSendOutcome::DefinitelyUnsent => {}
                ProjectedSendOutcome::OutcomeUnknown => source.delivery_unknown = true,
            }
            source.queue_revision = checked_revision(source.queue_revision)?;
        }
        self.notify(session_key);
        Ok(())
    }

    pub fn observe_event(
        &self,
        session_key: &str,
        event: &EngineEvent,
    ) -> Result<(), CollaborationServiceError> {
        {
            let mut state = self.lock_state()?;
            let source = state.sources.entry(session_key.to_owned()).or_default();
            apply_approval_event(source, event);
            if let Some(projected) = project_transcript_event(event) {
                append_projected(source, projected)?;
            }
            if matches!(event.event, EngineEventKind::SessionClosed) {
                source.events.clear();
                source.events.shrink_to_fit();
            }
        }
        self.notify(session_key);
        Ok(())
    }

    /// The last history sequence this source has applied; a caller fetches
    /// only the events after it before calling [`Self::hydrate_history`].
    pub fn history_sequence(&self, session_key: &str) -> Result<u64, CollaborationServiceError> {
        Ok(self
            .lock_state()?
            .sources
            .get(session_key)
            .map_or(0, |source| source.history_sequence))
    }

    pub fn hydrate_history(
        &self,
        session_key: &str,
        omitted_events_through: Option<u64>,
        events: &[interface_runtime::HistoryEvent],
    ) -> Result<(), CollaborationServiceError> {
        let changed = {
            let mut state = self.lock_state()?;
            let source = state.sources.entry(session_key.to_owned()).or_default();
            let mut changed = false;
            if omitted_events_through.is_some_and(|omitted| omitted > source.history_sequence) {
                source.delivery_unknown = true;
                changed = true;
            }
            // Events fetched before a live event advanced the sequence are
            // filtered here, so a tail read from an older sequence is safe.
            let history_sequence = source.history_sequence;
            for item in events
                .iter()
                .filter(|item| item.sequence > history_sequence)
            {
                apply_approval_event(source, &item.event);
                source.history_sequence = item.sequence;
                changed = true;
            }
            changed
        };
        if changed {
            self.notify(session_key);
        }
        Ok(())
    }

    pub fn fail_closed(&self, session_key: &str) -> Result<(), CollaborationServiceError> {
        {
            let mut state = self.lock_state()?;
            let source = state.sources.entry(session_key.to_owned()).or_default();
            source.delivery_unknown = true;
            source.send_in_flight = false;
            source.draft_empty = None;
            source.queue_revision = checked_revision(source.queue_revision)?;
            source.delivery_revision = checked_revision(source.delivery_revision)?;
        }
        self.notify(session_key);
        Ok(())
    }

    pub fn source_state(
        &self,
        session_key: &str,
    ) -> Result<ProjectedSourceState, CollaborationServiceError> {
        let mut state = self.lock_state()?;
        let source = state.sources.entry(session_key.to_owned()).or_default();
        let pending = source.pending_approvals.iter().cloned().collect::<Vec<_>>();
        Ok(ProjectedSourceState {
            queue_revision: source.queue_revision,
            queue_depth: u64::from(source.send_in_flight || source.draft_empty != Some(true)),
            delivery_revision: source
                .history_sequence
                .checked_add(source.delivery_revision)
                .filter(|value| *value <= interface_collaboration::state::JS_MAX_SAFE_INTEGER)
                .ok_or_else(|| CollaborationServiceError {
                    code: "collaboration-revision-exhausted".to_owned(),
                    message: "Collaboration source projection revision exhausted.".to_owned(),
                    retryable: false,
                    replay_safe: false,
                })?,
            delivery_unknown: source.delivery_unknown,
            pending_approval_set_hash: approval_set_hash(pending),
            pending_approval_count: u64::try_from(source.pending_approvals.len())
                .unwrap_or(u64::MAX),
        })
    }

    pub fn live_transcript(
        &self,
        session_key: &str,
    ) -> Result<Vec<EngineEvent>, CollaborationServiceError> {
        let mut state = self.lock_state()?;
        Ok(state
            .sources
            .entry(session_key.to_owned())
            .or_default()
            .events
            .iter()
            .cloned()
            .collect())
    }

    pub fn hold_peer(&self, peer: HeldPeerProjection) -> Result<(), CollaborationServiceError> {
        let source_key = peer.source_session_key.clone();
        {
            let mut state = self.lock_state()?;
            let rows = state.held.entry(source_key.clone()).or_default();
            if rows
                .get(&peer.queue_entry_id)
                .is_some_and(|current| current != &peer)
            {
                return Err(CollaborationServiceError {
                    code: "collaboration-peer-identity-conflict".to_owned(),
                    message: "Durable peer projection identity conflict.".to_owned(),
                    retryable: false,
                    replay_safe: false,
                });
            }
            rows.insert(peer.queue_entry_id.clone(), peer);
        }
        self.notify(&source_key);
        Ok(())
    }

    pub fn remove_peer(
        &self,
        source_session_key: &str,
        collaboration_id: &str,
        queue_entry_id: &str,
        result_id: &str,
    ) -> Result<(), CollaborationServiceError> {
        {
            let mut state = self.lock_state()?;
            let Some(rows) = state.held.get_mut(source_session_key) else {
                return Ok(());
            };
            let Some(current) = rows.get(queue_entry_id) else {
                return Ok(());
            };
            if current.collaboration_id != collaboration_id || current.result_id != result_id {
                return Err(CollaborationServiceError {
                    code: "collaboration-peer-identity-conflict".to_owned(),
                    message: "Durable peer projection identity conflict.".to_owned(),
                    retryable: false,
                    replay_safe: false,
                });
            }
            rows.remove(queue_entry_id);
            if rows.is_empty() {
                state.held.remove(source_session_key);
            }
        }
        self.notify(source_session_key);
        Ok(())
    }

    pub fn held_peer(
        &self,
        source_session_key: &str,
        queue_entry_id: &str,
    ) -> Result<Option<HeldPeerProjection>, CollaborationServiceError> {
        Ok(self
            .lock_state()?
            .held
            .get(source_session_key)
            .and_then(|rows| rows.get(queue_entry_id))
            .cloned())
    }

    #[cfg(test)]
    pub(super) fn held_collaboration_for_test(
        &self,
        source: &str,
        collaboration: &str,
    ) -> Option<HeldPeerProjection> {
        self.lock_state()
            .expect("projection state")
            .held
            .get(source)
            .and_then(|rows| {
                rows.values()
                    .find(|peer| peer.collaboration_id == collaboration)
                    .cloned()
            })
    }

    fn lock_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, ProjectionState>, CollaborationServiceError> {
        self.state.lock().map_err(|_| CollaborationServiceError {
            code: "collaboration-projection-unavailable".to_owned(),
            message: "Collaboration source projection is unavailable.".to_owned(),
            retryable: false,
            replay_safe: false,
        })
    }

    fn notify(&self, session_key: &str) {
        if let Some(callback) = self.source_changed.get() {
            callback(session_key);
        }
    }
}

fn apply_approval_event(source: &mut ProjectionSource, event: &EngineEvent) {
    match &event.event {
        EngineEventKind::ApprovalRequested { approval_id, .. } => {
            source.pending_approvals.insert(approval_id.clone());
        }
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle | SessionStatus::Failed,
        }
        | EngineEventKind::Error {
            retryable: false, ..
        }
        | EngineEventKind::SessionClosed => source.pending_approvals.clear(),
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectedSendOutcome {
    AdapterAccepted,
    DefinitelyUnsent,
    OutcomeUnknown,
}

fn append_projected(
    source: &mut ProjectionSource,
    event: EngineEvent,
) -> Result<(), CollaborationServiceError> {
    source.events.push_back(event);
    if source.events.len() > MAX_LIVE_TRANSCRIPT_EVENTS {
        source.events.pop_front();
    }
    source.delivery_revision = checked_revision(source.delivery_revision)?;
    Ok(())
}

fn checked_revision(value: u64) -> Result<u64, CollaborationServiceError> {
    value
        .checked_add(1)
        .filter(|value| *value <= interface_collaboration::state::JS_MAX_SAFE_INTEGER)
        .ok_or_else(|| CollaborationServiceError {
            code: "collaboration-revision-exhausted".to_owned(),
            message: "Collaboration source projection revision exhausted.".to_owned(),
            retryable: false,
            replay_safe: false,
        })
}

fn approval_set_hash(mut identifiers: Vec<String>) -> String {
    identifiers.sort();
    let encoded = serde_json::to_string(&identifiers).unwrap_or_else(|_| "[]".to_owned());
    let mut digest = Sha256::new();
    digest.update(b"iface-collaboration-approval-set-v1\0");
    digest.update(encoded.as_bytes());
    format!("{:x}", digest.finalize())
}

fn project_transcript_event(event: &EngineEvent) -> Option<EngineEvent> {
    let projected = match &event.event {
        EngineEventKind::MessageDelta { text, agent_key } => EngineEventKind::MessageDelta {
            text: bounded_text(text),
            agent_key: agent_key.clone(),
        },
        EngineEventKind::UserMessage {
            text, agent_key, ..
        } => EngineEventKind::UserMessage {
            text: bounded_text(text),
            agent_key: agent_key.as_deref().map(bounded_text),
            images: None,
            image_references: None,
        },
        EngineEventKind::PeerResult {
            operation_id,
            source_engine,
            source_session_id,
            objective,
            text,
            untrusted,
        } => EngineEventKind::PeerResult {
            operation_id: bounded_text(operation_id),
            source_engine: source_engine.clone(),
            source_session_id: bounded_text(source_session_id),
            objective: bounded_text(objective),
            text: bounded_text(text),
            untrusted: *untrusted,
        },
        EngineEventKind::DiffProduced {
            call_id,
            path,
            agent_key,
            ..
        } => EngineEventKind::DiffProduced {
            call_id: call_id.clone(),
            path: bounded_text(path),
            before: String::new(),
            after: String::new(),
            agent_key: agent_key.clone(),
        },
        EngineEventKind::PlanUpdated { entries } => EngineEventKind::PlanUpdated {
            entries: entries
                .iter()
                .take(100)
                .cloned()
                .map(|mut entry| {
                    entry.content = bounded_text(&entry.content);
                    entry
                })
                .collect(),
        },
        _ => return None,
    };
    Some(EngineEvent {
        at: event.at.clone(),
        event: projected,
    })
}

fn bounded_text(value: &str) -> String {
    bounded_tail(&sanitize_text(value), MAX_PROJECTED_TEXT_BYTES)
}
