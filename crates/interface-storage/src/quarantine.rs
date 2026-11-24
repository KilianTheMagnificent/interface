use std::fs;

use uuid::Uuid;

use crate::atomic::is_atomic_temp_name_for;
use crate::error::{Result, StorageError};
use crate::lock::WriterLock;
use crate::secure::{
    FileSnapshot, PrivateDirectory, TargetSnapshot, read_bounded, snapshot_target,
};

const QUARANTINE_DIRECTORY: &str = ".quarantine";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuarantineReason {
    Corrupt,
    StaleAtomicWrite,
}

impl QuarantineReason {
    const fn token(self) -> &'static str {
        match self {
            Self::Corrupt => "corrupt",
            Self::StaleAtomicWrite => "stale",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedArtifact {
    name: String,
    path: std::path::PathBuf,
    snapshot: FileSnapshot,
}

impl QuarantinedArtifact {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the set-aside bytes now live, for showing someone.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    #[must_use]
    pub const fn snapshot(&self) -> &FileSnapshot {
        &self.snapshot
    }
}

/// Moves exact, previously-inspected bytes into owner-private forensic storage.
///
/// # Errors
///
/// Returns an error when the source snapshot or lease drifted, either
/// directory is unsafe, or the move and both directory fsyncs are unproven.
pub fn quarantine_file(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    name: &str,
    expected: &FileSnapshot,
    reason: QuarantineReason,
    max_bytes: usize,
) -> Result<QuarantinedArtifact> {
    lock.assert_held_for(directory)?;
    let current =
        read_bounded(directory, name, max_bytes)?.ok_or_else(|| StorageError::NotFound {
            path: directory.path().join(name),
        })?;
    if current.snapshot() != expected {
        return Err(StorageError::ExternalDrift {
            path: directory.path().join(name),
        });
    }
    let quarantine = directory.open_or_create_child(QUARANTINE_DIRECTORY)?;
    let artifact_name = format!("q-{}-{}.quarantined", Uuid::new_v4(), reason.token());
    if snapshot_target(&quarantine, &artifact_name, max_bytes)? != TargetSnapshot::Missing {
        return Err(StorageError::ExternalDrift {
            path: quarantine.path().join(&artifact_name),
        });
    }
    let source = directory.entry_path(name)?;
    let destination = quarantine.entry_path(&artifact_name)?;
    let mut renamed = false;
    let operation = (|| -> Result<QuarantinedArtifact> {
        lock.assert_held_for(directory)?;
        let before =
            read_bounded(directory, name, max_bytes)?.ok_or_else(|| StorageError::NotFound {
                path: source.clone(),
            })?;
        if before.snapshot() != expected {
            return Err(StorageError::ExternalDrift {
                path: source.clone(),
            });
        }
        fs::rename(&source, &destination)
            .map_err(|error| StorageError::io("move file into quarantine", &source, error))?;
        renamed = true;
        lock.assert_held_for(directory)?;
        let installed = read_bounded(&quarantine, &artifact_name, max_bytes)?.ok_or_else(|| {
            StorageError::unsafe_at(&destination, "quarantined artifact is missing")
        })?;
        if installed.snapshot() != expected || installed.bytes() != before.bytes() {
            return Err(StorageError::unsafe_at(
                &destination,
                "quarantined artifact does not match the source inode and bytes",
            ));
        }
        if snapshot_target(directory, name, max_bytes)? != TargetSnapshot::Missing {
            return Err(StorageError::ExternalDrift {
                path: source.clone(),
            });
        }
        quarantine.sync()?;
        directory.sync()?;
        lock.assert_held_for(directory)?;
        Ok(QuarantinedArtifact {
            path: destination.clone(),
            name: artifact_name,
            snapshot: installed.snapshot().clone(),
        })
    })();

    match operation {
        Ok(artifact) => Ok(artifact),
        Err(error) if renamed => Err(StorageError::WriteUncertain {
            path: source,
            reason: format!("quarantine move may have committed: {error}"),
        }),
        Err(error) => Err(error),
    }
}

/// Quarantines only [`crate::atomic_write`] temps whose digest prefix matches
/// `target`.
///
/// Other stores' in-flight atomic temps in the same directory are left
/// untouched. The caller must already hold `lock`; there is no age threshold.
///
/// # Errors
///
/// Returns an error if a matching temp is unsafe or changes, the lease is
/// unavailable, or any quarantine move is not durable.
pub fn quarantine_stale_atomic_writes_for(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    target: &str,
    max_bytes: usize,
) -> Result<Vec<QuarantinedArtifact>> {
    lock.assert_held_for(directory)?;
    let stale = directory
        .read_names()?
        .into_iter()
        .filter(|name| is_atomic_temp_name_for(name, target))
        .collect::<Vec<_>>();
    let mut artifacts = Vec::with_capacity(stale.len());
    for name in stale {
        let snapshot = read_bounded(directory, &name, max_bytes)?
            .ok_or_else(|| StorageError::ExternalDrift {
                path: directory.path().join(&name),
            })?
            .snapshot()
            .clone();
        artifacts.push(quarantine_file(
            directory,
            lock,
            &name,
            &snapshot,
            QuarantineReason::StaleAtomicWrite,
            max_bytes,
        )?);
    }
    Ok(artifacts)
}
