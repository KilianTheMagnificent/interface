//! Privacy-preserving collaboration audit projection and durable JSONL sink.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use interface_storage::{
    JsonLimits, PrivateDirectory, StorageError, WriterLock, decode_json_strict,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::state::{CollaborationAuditEventV1, CollaborationErrorCode, CollaborationState};

mod storage;

const AUDIT_LOG: &str = "audit.jsonl";
const AUDIT_LOCK: &str = "audit.lock";
const MAX_AUDIT_BYTES: usize = 64 * 1_024 * 1_024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditDistribution {
    pub count: u64,
    pub sum: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
}

impl AuditDistribution {
    fn add(&mut self, value: u64) {
        self.count = self.count.saturating_add(1);
        self.sum = self.sum.saturating_add(value);
        self.min = Some(self.min.map_or(value, |current| current.min(value)));
        self.max = Some(self.max.map_or(value, |current| current.max(value)));
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuditCounters {
    pub requests: u64,
    pub approvals: u64,
    pub denials: u64,
    pub expired_approvals: u64,
    pub active_by_state: BTreeMap<CollaborationState, u64>,
    pub failures_by_code: BTreeMap<CollaborationErrorCode, u64>,
    pub ambiguous_outcomes: u64,
    pub recovery_actions: u64,
    pub queue_latency_ms: AuditDistribution,
    pub provider_duration_ms: AuditDistribution,
    pub packet_bytes: AuditDistribution,
    pub result_bytes: AuditDistribution,
}

#[must_use]
pub fn aggregate_audit_counters(
    events: &[CollaborationAuditEventV1],
    records: &[CollaborationState],
) -> AuditCounters {
    let mut projection = AuditProjection::default();
    for event in events {
        projection.record(event);
    }
    projection.snapshot(records)
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuditProjection {
    counters: AuditCounters,
    held_at: BTreeMap<String, i64>,
}

impl AuditProjection {
    fn record(&mut self, event: &CollaborationAuditEventV1) {
        let Self { counters, held_at } = self;
        if event.from == CollaborationState::Requested
            && event.to == CollaborationState::AwaitingApproval
        {
            counters.requests = counters.requests.saturating_add(1);
        }
        if event.from == CollaborationState::AwaitingApproval
            && event.approval_decision == Some(crate::state::AuditApprovalDecision::Approved)
        {
            counters.approvals = counters.approvals.saturating_add(1);
        }
        match event.approval_decision {
            Some(crate::state::AuditApprovalDecision::Denied) => {
                counters.denials = counters.denials.saturating_add(1);
            }
            Some(crate::state::AuditApprovalDecision::Expired) => {
                counters.expired_approvals = counters.expired_approvals.saturating_add(1);
            }
            _ => {}
        }
        if let Some(code) = event.reason_code {
            let count = counters.failures_by_code.entry(code).or_default();
            *count = count.saturating_add(1);
        }
        if event.to == CollaborationState::Unknown
            || event.reason_code.is_some_and(|code| {
                matches!(
                    code,
                    CollaborationErrorCode::ProviderOutcomeUnknown
                        | CollaborationErrorCode::SourceParkUnknown
                        | CollaborationErrorCode::DeliveryUnknown
                )
            })
        {
            counters.ambiguous_outcomes = counters.ambiguous_outcomes.saturating_add(1);
        }
        if event.from == event.to {
            counters.recovery_actions = counters.recovery_actions.saturating_add(1);
        }
        let event_millis = DateTime::parse_from_rfc3339(&event.at)
            .map(|value| value.timestamp_millis())
            .ok();
        // Every exit from peer review (delivery, discard, cancel, failure)
        // ends the hold, so a checkpoint never carries an orphaned entry.
        let held_since = if event.from == CollaborationState::AwaitingPeerReview {
            held_at.remove(event.collaboration_id.as_str())
        } else {
            None
        };
        if event.to == CollaborationState::AwaitingPeerReview {
            if let Some(at) = event_millis {
                held_at.insert(event.collaboration_id.clone(), at);
            }
        }
        if event.from == CollaborationState::AwaitingPeerReview
            && event.to == CollaborationState::DeliveryPending
            && let (Some(started), Some(finished)) = (held_since, event_millis)
            && finished >= started
        {
            counters
                .queue_latency_ms
                .add(u64::try_from(finished - started).unwrap_or(u64::MAX));
        }
        if let Some(duration) = event.duration_ms
            && (provider_state(event.from) || provider_state(event.to))
        {
            counters.provider_duration_ms.add(duration);
        }
        if let Some(bytes) = event.packet_bytes {
            counters.packet_bytes.add(bytes);
        }
        if let Some(bytes) = event.result_bytes {
            counters.result_bytes.add(bytes);
        }
    }

    fn snapshot(&self, records: &[CollaborationState]) -> AuditCounters {
        let mut counters = self.counters.clone();
        for state in records {
            let count = counters.active_by_state.entry(*state).or_default();
            *count = count.saturating_add(1);
        }
        counters
    }
}

const fn provider_state(state: CollaborationState) -> bool {
    matches!(
        state,
        CollaborationState::PreparingTarget
            | CollaborationState::RunningTarget
            | CollaborationState::ParkingSource
            | CollaborationState::OwnershipTransfer
            | CollaborationState::ActivatingTarget
    )
}

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("collaboration audit sink is closed")]
    Closed,
    #[error("collaboration audit sink is poisoned and must be reopened")]
    Poisoned,
    #[error("invalid collaboration audit event")]
    InvalidEvent,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Owner-private, lossless audit segments with cumulative v2 checkpoints.
/// Each append atomically replaces at most one 256 KiB active segment.
pub struct CollaborationAuditSink {
    root: PrivateDirectory,
    lock: WriterLock,
    storage: storage::Storage,
    projection: AuditProjection,
    closed: bool,
    poisoned: bool,
}

impl CollaborationAuditSink {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, AuditError> {
        let root_path = root.as_ref();
        if !root_path.is_absolute() || root_path.parent().is_none() {
            return Err(AuditError::Storage(StorageError::InvalidPath {
                path: root_path.to_path_buf(),
                reason: "audit root must be a non-root absolute path",
            }));
        }
        let root = PrivateDirectory::open_or_create(root_path)?;
        let lock = WriterLock::acquire(&root, AUDIT_LOCK)?;
        let (storage, projection) = storage::Storage::open(&root)?;
        Ok(Self {
            root,
            lock,
            storage,
            projection,
            closed: false,
            poisoned: false,
        })
    }

    pub fn record(&mut self, event: CollaborationAuditEventV1) -> Result<(), AuditError> {
        if self.closed {
            return Err(AuditError::Closed);
        }
        if self.poisoned {
            return Err(AuditError::Poisoned);
        }
        event.validate().map_err(|_| AuditError::InvalidEvent)?;
        let mut line = serde_json::to_vec(&event).map_err(|error| {
            AuditError::Storage(StorageError::InvalidData {
                path: self.root.path().join(AUDIT_LOG),
                reason: error.to_string(),
            })
        })?;
        line.push(b'\n');
        match self
            .storage
            .append(&self.root, &self.lock, &self.projection, &line)
        {
            Ok(()) => {
                self.projection.record(&event);
                Ok(())
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self, records: &[CollaborationState]) -> AuditCounters {
        self.projection.snapshot(records)
    }

    pub fn close(&mut self) -> Result<(), AuditError> {
        self.closed = true;
        if self.poisoned {
            return Err(AuditError::Poisoned);
        }
        self.lock.assert_held_for(&self.root)?;
        Ok(())
    }

    #[must_use]
    pub fn log_file(&self) -> PathBuf {
        self.root.path().join(&self.storage.name)
    }
}

fn decode_events(bytes: &[u8]) -> Result<Vec<CollaborationAuditEventV1>, AuditError> {
    let text = std::str::from_utf8(bytes).map_err(|error| {
        AuditError::Storage(StorageError::Corrupt {
            path: PathBuf::from(AUDIT_LOG),
            reason: format!("audit log is not UTF-8: {error}"),
        })
    })?;
    let mut events = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let limits = JsonLimits::new(line.len().max(1), 32, 2_048).map_err(|error| {
            AuditError::Storage(StorageError::Corrupt {
                path: PathBuf::from(AUDIT_LOG),
                reason: format!("audit log has invalid parser limits: {error}"),
            })
        })?;
        let event: CollaborationAuditEventV1 =
            decode_json_strict(line.as_bytes(), limits, AUDIT_LOG).map_err(|error| {
                AuditError::Storage(StorageError::Corrupt {
                    path: PathBuf::from(AUDIT_LOG),
                    reason: format!("audit log contains invalid JSONL: {error}"),
                })
            })?;
        event.validate().map_err(|_| AuditError::InvalidEvent)?;
        events.push(event);
    }
    Ok(events)
}
