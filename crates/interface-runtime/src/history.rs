use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};

use interface_contracts::engine::{EngineEvent, EngineEventKind, SessionStatus, SessionSummary};
use interface_engine::{DeliveryOutcome, EngineError};
use serde::{Deserialize, Serialize};

use crate::{RuntimeError, RuntimeErrorKind};

const HISTORY_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryLimits {
    pub events_per_session: usize,
    pub effects_per_session: usize,
    /// Estimated serialized bytes of disposable transcript content. Approval
    /// inputs and the latest lifecycle/configuration facts are an exact reserve.
    pub bytes_per_session: usize,
    pub bytes_total: usize,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            events_per_session: 10_000,
            effects_per_session: 2_000,
            bytes_per_session: 4 * 1024 * 1024,
            bytes_total: 32 * 1024 * 1024,
        }
    }
}

impl HistoryLimits {
    /// # Errors
    ///
    /// Returns an error when either retention bound is zero.
    pub fn validate(self) -> Result<Self, RuntimeError> {
        if self.events_per_session == 0
            || self.effects_per_session == 0
            || self.bytes_per_session == 0
            || self.bytes_total == 0
        {
            return Err(RuntimeError::definitely_unsent(
                RuntimeErrorKind::InvalidInput,
                "history retention limits must be positive",
                false,
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeStreamState {
    Active,
    Closed,
    Overflow,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeDeliveryState {
    Open,
    OutcomeUnknown,
    PersistenceFailed,
    Closed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeEffectKind {
    Send,
    Cancel,
    Approval,
    SetModel,
    SetEffort,
    SetFastMode,
    SetPermissionMode,
    Close,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectAuditDisposition {
    Confirmed,
    DefinitelyUnsent,
    DefinitelyRejected,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryEvent {
    pub sequence: u64,
    pub received_at: String,
    pub event: EngineEvent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryEffect {
    pub sequence: u64,
    pub effect_id: String,
    pub kind: RuntimeEffectKind,
    pub completed_at: String,
    pub disposition: EffectAuditDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EngineError>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistorySession {
    pub key: String,
    pub summary: SessionSummary,
    pub attachment_count: u64,
    pub last_attached_at: String,
    pub next_event_sequence: u64,
    pub next_effect_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_events_through: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted_effects_through: Option<u64>,
    pub stream_state: RuntimeStreamState,
    pub delivery_state: RuntimeDeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_message: Option<String>,
    /// Ring-backed retention windows. `VecDeque` serializes as the same frozen
    /// camelCase JSON array a `Vec` does, so the wire shape is unchanged.
    pub events: VecDeque<HistoryEvent>,
    pub effects: VecDeque<HistoryEffect>,
}

/// The scalar facts of one [`HistorySession`], read without copying its
/// retained events or effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistorySessionState {
    pub attachment_count: u64,
    pub next_event_sequence: u64,
    pub omitted_events_through: Option<u64>,
    pub stream_state: RuntimeStreamState,
    pub delivery_state: RuntimeDeliveryState,
}

impl From<&HistorySession> for HistorySessionState {
    fn from(session: &HistorySession) -> Self {
        Self {
            attachment_count: session.attachment_count,
            next_event_sequence: session.next_event_sequence,
            omitted_events_through: session.omitted_events_through,
            stream_state: session.stream_state,
            delivery_state: session.delivery_state,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeHistorySnapshot {
    pub version: u8,
    pub revision: u64,
    pub sessions: Vec<HistorySession>,
}

impl Default for RuntimeHistorySnapshot {
    fn default() -> Self {
        Self {
            version: HISTORY_VERSION,
            revision: 0,
            sessions: Vec::new(),
        }
    }
}

/// Process-local bounded provider event and delivery projection.
///
/// The frozen application has no authoritative event-history file. Provider
/// stores remain the transcript source of truth; this projection exists only
/// to preserve live lifecycle/fencing state for the current process.
///
/// `mutate` re-validates the mutated session's tail under `debug_assert!` only:
/// debug and test builds panic on a violated invariant, release builds never
/// pay the pass. Every event is validated before it is appended, so the tail is
/// the only edge a single mutation can break.
#[derive(Debug)]
pub struct EngineHistoryStore {
    snapshot: RuntimeHistorySnapshot,
    limits: HistoryLimits,
    retained_bytes: HashMap<String, usize>,
    total_bytes: usize,
}

impl EngineHistoryStore {
    /// Opens a fresh process-local projection.
    ///
    /// # Errors
    ///
    /// Returns an error only when the requested retention limits are invalid.
    pub fn open(_root: impl AsRef<Path>, limits: HistoryLimits) -> Result<Self, RuntimeError> {
        let limits = limits.validate()?;
        Ok(Self {
            snapshot: RuntimeHistorySnapshot::default(),
            limits,
            retained_bytes: HashMap::new(),
            total_bytes: 0,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeHistorySnapshot {
        self.snapshot.clone()
    }

    #[must_use]
    pub fn session(&self, key: &str) -> Option<HistorySession> {
        self.find(key).cloned()
    }

    /// The session's scalar state only: callers that read a few fields must
    /// not deep-copy every retained event under the history lock.
    #[must_use]
    pub fn session_state(&self, key: &str) -> Option<HistorySessionState> {
        self.find(key).map(HistorySessionState::from)
    }

    /// The session's state plus only the retained events after
    /// `after_sequence` (events are kept in sequence order).
    #[must_use]
    pub fn session_events_after(
        &self,
        key: &str,
        after_sequence: u64,
    ) -> Option<(HistorySessionState, Vec<HistoryEvent>)> {
        let session = self.find(key)?;
        let start = session
            .events
            .partition_point(|event| event.sequence <= after_sequence);
        Some((
            HistorySessionState::from(session),
            session.events.range(start..).cloned().collect(),
        ))
    }

    fn find(&self, key: &str) -> Option<&HistorySession> {
        self.snapshot
            .sessions
            .iter()
            .find(|session| session.key == key)
    }

    /// Registers a process-local attachment or reopens an existing provider session.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid key or exhausted revision/attachment count.
    pub fn register_session(
        &mut self,
        key: String,
        summary: SessionSummary,
        at: String,
    ) -> Result<(), RuntimeError> {
        let tail_key = key.clone();
        self.mutate(&tail_key, |candidate, _| {
            if let Some(session) = candidate
                .sessions
                .iter_mut()
                .find(|session| session.key == key)
            {
                session.attachment_count = session
                    .attachment_count
                    .checked_add(1)
                    .ok_or_else(revision_exhausted)?;
                session.summary = merge_runtime_summary(&session.summary, summary);
                session.last_attached_at = at;
                session.stream_state = RuntimeStreamState::Active;
                session.delivery_state = RuntimeDeliveryState::Open;
                session.terminal_message = None;
            } else {
                if key.is_empty() || candidate.sessions.iter().any(|session| session.key == key) {
                    return Err(internal(
                        "engine history contains an empty or duplicate session key",
                    ));
                }
                candidate.sessions.push(HistorySession {
                    key,
                    summary,
                    attachment_count: 1,
                    last_attached_at: at,
                    next_event_sequence: 1,
                    next_effect_sequence: 1,
                    omitted_events_through: None,
                    omitted_effects_through: None,
                    stream_state: RuntimeStreamState::Active,
                    delivery_state: RuntimeDeliveryState::Open,
                    terminal_message: None,
                    events: VecDeque::new(),
                    effects: VecDeque::new(),
                });
            }
            Ok(())
        })
    }

    /// Retains and projects one validated provider event in this process.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid provider data, missing sessions, or exhausted sequences.
    pub fn append_event(
        &mut self,
        key: &str,
        event: EngineEvent,
        received_at: String,
    ) -> Result<HistoryEvent, RuntimeError> {
        event.validate().map_err(|error| {
            RuntimeError::definitely_unsent(
                RuntimeErrorKind::History,
                format!("provider event failed validation: {error}"),
                false,
            )
        })?;
        let bytes = disposable_bytes(&event);
        let mut stored = None;
        self.mutate(key, |candidate, _| {
            let session = find_session_mut(candidate, key)?;
            let item = HistoryEvent {
                sequence: session.next_event_sequence,
                received_at,
                event,
            };
            session.next_event_sequence = session
                .next_event_sequence
                .checked_add(1)
                .ok_or_else(revision_exhausted)?;
            project_event(&mut session.summary, &item.event, &item.received_at);
            session.events.push_back(item.clone());
            stored = Some(item);
            Ok(())
        })?;
        self.retain_content(key, bytes);
        stored.ok_or_else(|| internal("history append produced no event"))
    }

    /// Appends a content-free effect receipt, preserving provider certainty.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing session or exhausted sequences.
    pub fn append_effect(
        &mut self,
        key: &str,
        effect_id: String,
        kind: RuntimeEffectKind,
        completed_at: String,
        outcome: &DeliveryOutcome<()>,
    ) -> Result<HistoryEffect, RuntimeError> {
        let (disposition, error) = effect_parts(outcome);
        let mut stored = None;
        self.mutate(key, |candidate, limits| {
            let session = find_session_mut(candidate, key)?;
            let item = HistoryEffect {
                sequence: session.next_effect_sequence,
                effect_id,
                kind,
                completed_at,
                disposition,
                error,
            };
            session.next_effect_sequence = session
                .next_effect_sequence
                .checked_add(1)
                .ok_or_else(revision_exhausted)?;
            if disposition == EffectAuditDisposition::OutcomeUnknown {
                session.delivery_state = RuntimeDeliveryState::OutcomeUnknown;
                session.terminal_message = item.error.as_ref().map(|error| error.message.clone());
            } else if kind == RuntimeEffectKind::Close
                && disposition == EffectAuditDisposition::Confirmed
            {
                session.delivery_state = RuntimeDeliveryState::Closed;
            }
            session.effects.push_back(item.clone());
            while session.effects.len() > limits.effects_per_session {
                if let Some(removed) = session.effects.pop_front() {
                    session.omitted_effects_through = Some(removed.sequence);
                }
            }
            stored = Some(item);
            Ok(())
        })?;
        stored.ok_or_else(|| internal("history append produced no effect"))
    }

    /// Records a provider-stream terminal state without discarding prior uncertainty.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing session or exhausted sequences.
    pub fn mark_stream_terminal(
        &mut self,
        key: &str,
        state: RuntimeStreamState,
        message: Option<String>,
        at: String,
    ) -> Result<(), RuntimeError> {
        self.mutate(key, |candidate, _| {
            let session = find_session_mut(candidate, key)?;
            session.stream_state = state;
            session.summary.last_activity = at;
            session.summary.status = match state {
                RuntimeStreamState::Active => session.summary.status,
                RuntimeStreamState::Closed => SessionStatus::Idle,
                RuntimeStreamState::Overflow => SessionStatus::Failed,
            };
            if session.delivery_state != RuntimeDeliveryState::OutcomeUnknown {
                session.delivery_state = match state {
                    RuntimeStreamState::Active => RuntimeDeliveryState::Open,
                    RuntimeStreamState::Closed => RuntimeDeliveryState::Closed,
                    RuntimeStreamState::Overflow => RuntimeDeliveryState::PersistenceFailed,
                };
                session.terminal_message = message;
            }
            Ok(())
        })?;
        if state != RuntimeStreamState::Active {
            self.compact_content(key, 0, 0);
        }
        Ok(())
    }

    /// Preserves a delivery quarantine reported by the runtime.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing session or exhausted sequences.
    pub fn mark_persistence_failed(
        &mut self,
        key: &str,
        message: String,
        at: String,
    ) -> Result<(), RuntimeError> {
        self.mutate(key, |candidate, _| {
            let session = find_session_mut(candidate, key)?;
            if session.delivery_state != RuntimeDeliveryState::OutcomeUnknown {
                session.delivery_state = RuntimeDeliveryState::PersistenceFailed;
                session.terminal_message = Some(message);
            }
            session.summary.status = SessionStatus::Failed;
            session.summary.last_activity = at;
            Ok(())
        })
    }

    fn compact_content(&mut self, key: &str, bytes: usize, count: usize) {
        let Some(session) = self.snapshot.sessions.iter_mut().find(|s| s.key == key) else {
            return;
        };
        let retained = compact_events(session, bytes, count);
        let previous = self
            .retained_bytes
            .insert(key.to_owned(), retained)
            .unwrap_or(0);
        self.total_bytes = self
            .total_bytes
            .saturating_sub(previous)
            .saturating_add(retained);
    }

    fn retain_content(&mut self, key: &str, added: usize) {
        let bytes = self.retained_bytes.entry(key.to_owned()).or_default();
        *bytes = bytes.saturating_add(added);
        self.total_bytes = self.total_bytes.saturating_add(added);
        let over_count = self
            .snapshot
            .sessions
            .iter()
            .find(|s| s.key == key)
            .is_some_and(|s| s.events.len() > self.limits.events_per_session);
        if *bytes > self.limits.bytes_per_session || over_count {
            // Compact in batches so a full window is not rescanned on every delta.
            let count = self
                .limits
                .events_per_session
                .min(self.limits.events_per_session / 2 + 128);
            self.compact_content(key, self.limits.bytes_per_session / 2, count);
        }
        if self.total_bytes > self.limits.bytes_total {
            let mut oldest: Vec<_> = self
                .snapshot
                .sessions
                .iter()
                .filter(|s| self.retained_bytes.get(&s.key).copied().unwrap_or(0) > 0)
                .map(|s| {
                    (
                        s.stream_state == RuntimeStreamState::Active,
                        s.summary.last_activity.clone(),
                        s.key.clone(),
                    )
                })
                .collect();
            oldest.sort();
            for (_, _, key) in oldest {
                self.compact_content(&key, 0, 0);
                if self.total_bytes <= self.limits.bytes_total / 2 {
                    break;
                }
            }
        }
    }

    /// Applies one change to session `key` and bumps the revision.
    fn mutate(
        &mut self,
        key: &str,
        change: impl FnOnce(&mut RuntimeHistorySnapshot, HistoryLimits) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let revision = self
            .snapshot
            .revision
            .checked_add(1)
            .ok_or_else(revision_exhausted)?;
        change(&mut self.snapshot, self.limits)?;
        self.snapshot.revision = revision;
        debug_assert!(validate_session_tail(&self.snapshot, key).is_ok());
        Ok(())
    }
}

/// Counts serialized content without allocating another JSON copy. This is a
/// retention estimate, not a hard allocator/RSS bound.
fn disposable_bytes(event: &EngineEvent) -> usize {
    if !matches!(
        event.event,
        EngineEventKind::MessageDelta { .. }
            | EngineEventKind::UserMessage { .. }
            | EngineEventKind::PeerResult { .. }
            | EngineEventKind::ToolCall { .. }
            | EngineEventKind::ToolResult { .. }
            | EngineEventKind::DiffProduced { .. }
            | EngineEventKind::Notice { .. }
            | EngineEventKind::TurnRetry { .. }
            | EngineEventKind::ContextCompact { .. }
    ) {
        return 0;
    }
    event.serialized_len()
}

fn compact_events(session: &mut HistorySession, budget: usize, count: usize) -> usize {
    let mut bytes = 0_usize;
    let mut disposable = 0;
    let mut latest = HashSet::new();
    for item in std::mem::take(&mut session.events).into_iter().rev() {
        let size = disposable_bytes(&item.event);
        let keep = if size > 0 {
            if bytes.saturating_add(size) <= budget && disposable < count {
                bytes += size;
                disposable += 1;
                true
            } else {
                false
            }
        } else if matches!(item.event.event, EngineEventKind::ApprovalRequested { .. }) {
            // No explicit approval-resolution event exists in this frozen DTO.
            // Keep exact requests rather than infer resolution from stream closure.
            true
        } else {
            let agent = match &item.event.event {
                EngineEventKind::ThinkingSignal { agent_key, .. } => agent_key.clone(),
                EngineEventKind::SubagentStarted { agent_key, .. }
                | EngineEventKind::SubagentFinished { agent_key, .. } => Some(agent_key.clone()),
                _ => None,
            };
            latest.insert((std::mem::discriminant(&item.event.event), agent))
        };
        if keep {
            session.events.push_front(item);
        } else {
            session.omitted_events_through = Some(
                session
                    .omitted_events_through
                    .unwrap_or(0)
                    .max(item.sequence),
            );
        }
    }
    bytes
}

fn find_session_mut<'a>(
    snapshot: &'a mut RuntimeHistorySnapshot,
    key: &str,
) -> Result<&'a mut HistorySession, RuntimeError> {
    snapshot
        .sessions
        .iter_mut()
        .find(|session| session.key == key)
        .ok_or_else(|| {
            RuntimeError::definitely_unsent(
                RuntimeErrorKind::History,
                format!("history has no registered session {key}"),
                false,
            )
        })
}

fn effect_parts(outcome: &DeliveryOutcome<()>) -> (EffectAuditDisposition, Option<EngineError>) {
    match outcome {
        DeliveryOutcome::Confirmed(()) => (EffectAuditDisposition::Confirmed, None),
        DeliveryOutcome::DefinitelyUnsent(error) => (
            EffectAuditDisposition::DefinitelyUnsent,
            Some(error.clone()),
        ),
        DeliveryOutcome::DefinitelyRejected(error) => (
            EffectAuditDisposition::DefinitelyRejected,
            Some(error.clone()),
        ),
        DeliveryOutcome::OutcomeUnknown(error) => {
            (EffectAuditDisposition::OutcomeUnknown, Some(error.clone()))
        }
    }
}

fn project_event(summary: &mut SessionSummary, event: &EngineEvent, at: &str) {
    summary.status = interface_contracts::engine::next_status(summary.status, event);
    at.clone_into(&mut summary.last_activity);
    match &event.event {
        EngineEventKind::SessionMeta { session_id, .. } => {
            summary.session_id.clone_from(session_id);
        }
        EngineEventKind::SessionClosed => summary.status = SessionStatus::Idle,
        _ => {}
    }
    apply_provider_meta(summary, &event.event);
}

/// Copies provider-reported metadata onto a summary: `SessionMeta`'s cwd,
/// non-empty model and title, and `ConfigChanged`'s model and effort when
/// named. The one projection the history summary and the registry row share.
/// Returns whether the event carried metadata to apply (every `SessionMeta`,
/// a `ConfigChanged` naming either field), not whether a value differed.
pub(crate) fn apply_provider_meta(summary: &mut SessionSummary, event: &EngineEventKind) -> bool {
    match event {
        EngineEventKind::SessionMeta {
            model, cwd, title, ..
        } => {
            summary.cwd.clone_from(cwd);
            if !model.is_empty() {
                summary.model = Some(model.clone());
            }
            if let Some(title) = title {
                summary.title.clone_from(title);
            }
            true
        }
        EngineEventKind::ConfigChanged { model, effort } => {
            if model.is_some() {
                summary.model.clone_from(model);
            }
            if effort.is_some() {
                summary.effort.clone_from(effort);
            }
            model.is_some() || effort.is_some()
        }
        _ => false,
    }
}

fn merge_runtime_summary(
    previous: &SessionSummary,
    mut incoming: SessionSummary,
) -> SessionSummary {
    incoming.first_seen.clone_from(&previous.first_seen);
    incoming.archived = previous.archived;
    if incoming.title.trim().is_empty() || incoming.title == incoming.session_id {
        incoming.title.clone_from(&previous.title);
    }
    incoming.model = incoming.model.or_else(|| previous.model.clone());
    incoming.effort = incoming.effort.or_else(|| previous.effort.clone());
    incoming.project_root = incoming
        .project_root
        .or_else(|| previous.project_root.clone());
    incoming.worktree_home = incoming
        .worktree_home
        .or_else(|| previous.worktree_home.clone());
    incoming.worktree_path = incoming
        .worktree_path
        .or_else(|| previous.worktree_path.clone());
    incoming.setting_sources = incoming
        .setting_sources
        .or_else(|| previous.setting_sources.clone());
    incoming.disallowed_tools = incoming
        .disallowed_tools
        .or_else(|| previous.disallowed_tools.clone());
    incoming
}

/// Constant-time guard run after every mutation. Every new provider event is
/// fully validated before insertion; this checks the only sequence edge a
/// single mutation can introduce without rescanning all 10,000 retained events
/// in the debug Dev host.
fn validate_session_tail(snapshot: &RuntimeHistorySnapshot, key: &str) -> Result<(), RuntimeError> {
    let session = snapshot
        .sessions
        .iter()
        .find(|session| session.key == key)
        .ok_or_else(|| internal("engine history lost the mutated session"))?;
    if session.attachment_count == 0
        || session.next_event_sequence == 0
        || session.next_effect_sequence == 0
    {
        return Err(internal("engine history contains an exhausted sequence"));
    }
    if let Some(last) = session.events.back() {
        if last.sequence >= session.next_event_sequence
            || session
                .events
                .len()
                .checked_sub(2)
                .and_then(|index| session.events.get(index))
                .is_some_and(|previous| previous.sequence >= last.sequence)
        {
            return Err(internal(
                "engine history event tail is not strictly increasing",
            ));
        }
        last.event.validate().map_err(|error| {
            RuntimeError::definitely_unsent(
                RuntimeErrorKind::History,
                format!("persisted provider event failed validation: {error}"),
                false,
            )
        })?;
    }
    if let Some(last) = session.effects.back()
        && (last.sequence >= session.next_effect_sequence
            || session
                .effects
                .len()
                .checked_sub(2)
                .and_then(|index| session.effects.get(index))
                .is_some_and(|previous| previous.sequence >= last.sequence))
    {
        return Err(internal(
            "engine history effect tail is not strictly increasing",
        ));
    }
    Ok(())
}

fn revision_exhausted() -> RuntimeError {
    internal("engine history revision or sequence is exhausted")
}

fn internal(message: impl Into<String>) -> RuntimeError {
    RuntimeError::definitely_unsent(RuntimeErrorKind::History, message, false)
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
