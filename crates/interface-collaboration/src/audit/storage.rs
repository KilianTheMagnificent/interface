//! Lossless v2 audit segments. Only the active segment is replaced per event;
//! a checkpoint published between segments owns the cumulative projection.
use super::{AUDIT_LOG, AuditError, AuditProjection, MAX_AUDIT_BYTES, decode_events};
use interface_storage::{
    JsonLimits, PrivateDirectory, StorageError, TargetSnapshot, WriterLock, atomic_write,
    atomic_write_json, read_bounded, read_json, snapshot_target,
};
use serde::{Deserialize, Serialize};

const CHECKPOINT: &str = "audit-state-v2.json";
pub(super) const SEGMENT_BYTES: usize = 256 * 1024;
const CHECKPOINT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    active_segment: u64,
    legacy_sha256: Option<[u8; 32]>,
    // The preceding segment anchors the checkpoint's publication boundary.
    preceding_sha256: Option<[u8; 32]>,
    projection: AuditProjection,
}

pub(super) struct Storage {
    pub(super) name: String,
    snapshot: TargetSnapshot,
    bytes: Vec<u8>,
    checkpoint_snapshot: TargetSnapshot,
    checkpoint: Option<Checkpoint>,
}
fn limits() -> JsonLimits {
    JsonLimits::new(CHECKPOINT_BYTES, 32, 1_000_000).expect("fixed limits")
}
fn segment(id: u64) -> String {
    format!("audit-segment-{id:020}.jsonl")
}
fn digest(snapshot: &TargetSnapshot) -> Option<[u8; 32]> {
    match snapshot {
        TargetSnapshot::Missing => None,
        TargetSnapshot::File(file) => Some(*file.sha256()),
    }
}
fn corrupt(root: &PrivateDirectory, name: &str, reason: &str) -> AuditError {
    StorageError::Corrupt {
        path: root.path().join(name),
        reason: reason.into(),
    }
    .into()
}
fn unchanged(
    root: &PrivateDirectory,
    name: &str,
    expected: &TargetSnapshot,
    max: usize,
) -> Result<(), AuditError> {
    if &snapshot_target(root, name, max)? != expected {
        return Err(StorageError::ExternalDrift {
            path: root.path().join(name),
        }
        .into());
    }
    Ok(())
}
impl Storage {
    pub(super) fn open(root: &PrivateDirectory) -> Result<(Self, AuditProjection), AuditError> {
        let legacy = read_bounded(root, AUDIT_LOG, MAX_AUDIT_BYTES)?;
        let legacy_snapshot = legacy.as_ref().map_or(TargetSnapshot::Missing, |file| {
            TargetSnapshot::File(file.snapshot().clone())
        });
        if let Some(loaded) = read_json::<Checkpoint>(root, CHECKPOINT, limits())? {
            let (checkpoint, snapshot) = loaded.into_parts();
            if checkpoint.version != 2
                || checkpoint.legacy_sha256 != digest(&legacy_snapshot)
                || !checkpoint.projection.counters.active_by_state.is_empty()
                || (checkpoint.active_segment == 0) != checkpoint.preceding_sha256.is_none()
            {
                return Err(corrupt(
                    root,
                    CHECKPOINT,
                    "unsupported version or inconsistent checkpoint",
                ));
            }
            if let Some(expected) = checkpoint.preceding_sha256 {
                let prior = segment(checkpoint.active_segment - 1);
                if digest(&snapshot_target(root, &prior, SEGMENT_BYTES)?) != Some(expected) {
                    return Err(corrupt(
                        root,
                        &prior,
                        "sealed segment changed after checkpoint",
                    ));
                }
            }
            let name = segment(checkpoint.active_segment);
            let file = read_bounded(root, &name, SEGMENT_BYTES)?
                .ok_or_else(|| corrupt(root, &name, "selected audit segment is absent"))?;
            let (bytes, snapshot_active) = file.into_parts();
            let mut projection = checkpoint.projection.clone();
            for event in decode_events(&bytes)? {
                projection.record(&event);
            }
            Ok((
                Self {
                    name,
                    bytes,
                    snapshot: TargetSnapshot::File(snapshot_active),
                    checkpoint_snapshot: TargetSnapshot::File(snapshot),
                    checkpoint: Some(checkpoint),
                },
                projection,
            ))
        } else {
            // Opening a v1 log is read-only. Migration starts with the next event.
            let bytes = legacy.map_or_else(Vec::new, |file| file.into_parts().0);
            let mut projection = AuditProjection::default();
            for event in decode_events(&bytes)? {
                projection.record(&event);
            }
            Ok((
                Self {
                    name: AUDIT_LOG.into(),
                    bytes,
                    snapshot: legacy_snapshot,
                    checkpoint_snapshot: TargetSnapshot::Missing,
                    checkpoint: None,
                },
                projection,
            ))
        }
    }

    fn prepare_empty(
        root: &PrivateDirectory,
        lock: &WriterLock,
        name: &str,
    ) -> Result<TargetSnapshot, AuditError> {
        match read_bounded(root, name, SEGMENT_BYTES)? {
            Some(file) if file.bytes().is_empty() => Ok(TargetSnapshot::File(file.into_parts().1)),
            Some(_) => Err(corrupt(
                root,
                name,
                "unreferenced segment contains forensic data",
            )),
            None => Ok(TargetSnapshot::File(atomic_write(
                root,
                lock,
                name,
                &[],
                &TargetSnapshot::Missing,
                SEGMENT_BYTES,
            )?)),
        }
    }

    fn rotate(
        &mut self,
        root: &PrivateDirectory,
        lock: &WriterLock,
        projection: &AuditProjection,
    ) -> Result<(), AuditError> {
        let (active_segment, legacy_sha256, preceding_sha256) = match &self.checkpoint {
            None => (0, digest(&self.snapshot), None),
            Some(previous) => (
                previous
                    .active_segment
                    .checked_add(1)
                    .ok_or_else(|| corrupt(root, CHECKPOINT, "audit segment sequence exhausted"))?,
                previous.legacy_sha256,
                digest(&self.snapshot),
            ),
        };
        let name = segment(active_segment);
        let next_snapshot = Self::prepare_empty(root, lock, &name)?;
        // Before publication the old checkpoint and active segment remain the
        // complete committed projection. An orphan empty preparation is inert.
        unchanged(
            root,
            &self.name,
            &self.snapshot,
            if self.checkpoint.is_some() {
                SEGMENT_BYTES
            } else {
                MAX_AUDIT_BYTES
            },
        )?;
        let checkpoint = Checkpoint {
            version: 2,
            active_segment,
            legacy_sha256,
            preceding_sha256,
            projection: projection.clone(),
        };
        let written = atomic_write_json(
            root,
            lock,
            CHECKPOINT,
            &checkpoint,
            &self.checkpoint_snapshot,
            limits(),
        )?;
        self.checkpoint_snapshot = TargetSnapshot::File(written.into_parts().1);
        self.checkpoint = Some(checkpoint);
        self.snapshot = next_snapshot;
        self.name = name;
        self.bytes = Vec::new();
        Ok(())
    }

    pub(super) fn append(
        &mut self,
        root: &PrivateDirectory,
        lock: &WriterLock,
        projection: &AuditProjection,
        line: &[u8],
    ) -> Result<(), AuditError> {
        if line.len() > SEGMENT_BYTES {
            return Err(corrupt(
                root,
                &self.name,
                "audit event exceeds segment capacity",
            ));
        }
        lock.assert_held_for(root)?;
        unchanged(
            root,
            CHECKPOINT,
            &self.checkpoint_snapshot,
            CHECKPOINT_BYTES,
        )?;
        if self.checkpoint.is_none() || self.bytes.len() + line.len() > SEGMENT_BYTES {
            self.rotate(root, lock, projection)?;
        }
        let previous_len = self.bytes.len();
        self.bytes.extend_from_slice(line);
        match atomic_write(
            root,
            lock,
            &self.name,
            &self.bytes,
            &self.snapshot,
            SEGMENT_BYTES,
        ) {
            Ok(snapshot) => {
                self.snapshot = TargetSnapshot::File(snapshot);
                Ok(())
            }
            Err(error) => {
                self.bytes.truncate(previous_len);
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests;
