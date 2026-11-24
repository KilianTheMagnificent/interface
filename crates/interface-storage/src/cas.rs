use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Result, StorageError};
use crate::json::{JsonLimits, LoadedJson, atomic_write_json, decode_json_strict};
use crate::lock::WriterLock;
use crate::secure::{PrivateDirectory, TargetSnapshot, read_bounded};

/// Implemented by persisted domain records that carry an optimistic revision.
pub trait Revisioned {
    fn revision(&self) -> u64;
}

/// # Errors
///
/// Returns [`StorageError::CasConflict`] when `expected` is stale.
pub fn ensure_current_revision(expected: u64, current: u64) -> Result<()> {
    if expected != current {
        return Err(StorageError::CasConflict { expected, current });
    }
    Ok(())
}

/// # Errors
///
/// Returns [`StorageError::InvalidRevision`] unless `next` is exactly one
/// greater than `current`, including when the current revision is exhausted.
pub fn ensure_next_revision(current: u64, next: u64) -> Result<()> {
    if current.checked_add(1) != Some(next) {
        return Err(StorageError::InvalidRevision { current, next });
    }
    Ok(())
}

/// Writes one revision after re-reading the named target under the writer lock.
///
/// A newer valid on-disk revision is reported as a CAS conflict. Changed bytes
/// with the same revision are external drift and are never overwritten.
///
/// # Errors
///
/// Returns an error for stale/invalid revisions, unsafe or corrupt storage,
/// external drift, a lost writer lease, or an unproven atomic write.
pub fn compare_and_set_json<T>(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    name: &str,
    loaded: &LoadedJson<T>,
    expected_revision: u64,
    replacement: &T,
    limits: JsonLimits,
) -> Result<LoadedJson<T>>
where
    T: Serialize + DeserializeOwned + Revisioned,
{
    lock.assert_held_for(directory)?;
    ensure_current_revision(expected_revision, loaded.value().revision())?;
    ensure_next_revision(expected_revision, replacement.revision())?;

    // The write verifies the target against the loaded snapshot before it
    // creates anything, so an unchanged file is read once there, not twice.
    match atomic_write_json(
        directory,
        lock,
        name,
        replacement,
        &TargetSnapshot::File(loaded.snapshot().clone()),
        limits,
    ) {
        Err(StorageError::ExternalDrift { .. }) => Err(classify_drift::<T>(
            directory,
            name,
            expected_revision,
            limits,
        )?),
        result => result,
    }
}

/// Reads a target that no longer matches its loaded snapshot and tells a
/// newer revision (conflict) from same-revision drift. A missing target is
/// drift; a changed file that cannot be decoded reports its decode error.
fn classify_drift<T>(
    directory: &PrivateDirectory,
    name: &str,
    expected_revision: u64,
    limits: JsonLimits,
) -> Result<StorageError>
where
    T: DeserializeOwned + Revisioned,
{
    let path = directory.path().join(name);
    let Some(disk) = read_bounded(directory, name, limits.max_file_bytes())? else {
        return Ok(StorageError::ExternalDrift { path });
    };
    let current = decode_json_strict::<T>(disk.bytes(), limits, &path)?.revision();
    Ok(if current == expected_revision {
        StorageError::ExternalDrift { path }
    } else {
        StorageError::CasConflict {
            expected: expected_revision,
            current,
        }
    })
}
