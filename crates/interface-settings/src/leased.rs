//! The one commit protocol the settings stores share: a writer lease, the
//! exact snapshot of the last write, and when a failed write must latch.
use std::path::{Path, PathBuf};

use interface_storage::{
    JsonLimits, PrivateDirectory, QuarantineReason, StorageError, TargetSnapshot, WriterLock,
    atomic_write, atomic_write_json, decode_json_strict, prepare_json, quarantine_file,
    read_bounded, read_json, snapshot_target,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::private_path::{SettingsError, StoreLocation, prepare_store};

/// A settings document whose writer lease lives as long as its store.
///
/// A failed commit latches the document uncertain, refusing every later
/// commit until restart, only when disk may now differ from memory: drift, a
/// lost lease, a failure after the rename, or unsafe target state met while
/// writing. When nothing reached the target (the value is over its bounds or
/// cannot be represented strictly, or the atomic write failed before its
/// rename), disk still equals memory, so the refusal does not latch.
pub(crate) struct LeasedDocument {
    location: StoreLocation,
    lock: WriterLock,
    snapshot: TargetSnapshot,
    limits: JsonLimits,
    uncertain: bool,
    uncertain_message: &'static str,
}

impl LeasedDocument {
    /// Hardens `file`, leases its directory under `lock_name` and reads the
    /// document strictly; `None` when it does not exist yet.
    /// `uncertain_message` is the refusal every commit makes once latched.
    pub(crate) fn open<T: DeserializeOwned>(
        file: &Path,
        lock_name: &str,
        limits: JsonLimits,
        uncertain_message: &'static str,
    ) -> Result<(Self, Option<T>), SettingsError> {
        let location = prepare_store(file)?;
        let lock = WriterLock::acquire(&location.directory, lock_name)?;
        let (value, snapshot) = match read_json::<T>(&location.directory, &location.name, limits)? {
            Some(loaded) => {
                let (value, snapshot) = loaded.into_parts();
                (Some(value), TargetSnapshot::File(snapshot))
            }
            None => (None, TargetSnapshot::Missing),
        };
        let document = Self {
            location,
            lock,
            snapshot,
            limits,
            uncertain: false,
            uncertain_message,
        };
        Ok((document, value))
    }

    /// Like [`Self::open`], but a document that does not decode strictly or
    /// that `valid` refuses is moved into the store's quarantine under this
    /// lease, and the store starts empty. Answers where it was moved, when
    /// that happened.
    /// Unsafe storage, a lease held elsewhere and an oversized file still fail.
    pub(crate) fn open_or_quarantine<T: DeserializeOwned>(
        file: &Path,
        lock_name: &str,
        limits: JsonLimits,
        uncertain_message: &'static str,
        valid: impl FnOnce(&T) -> bool,
    ) -> Result<(Self, Option<T>, Option<PathBuf>), SettingsError> {
        let location = prepare_store(file)?;
        let lock = WriterLock::acquire(&location.directory, lock_name)?;
        let max_bytes = limits.max_file_bytes();
        let (value, snapshot, quarantined) =
            match read_bounded(&location.directory, &location.name, max_bytes)? {
                None => (None, TargetSnapshot::Missing, None),
                Some(read) => {
                    let path = location.directory.path().join(&location.name);
                    match decode_json_strict::<T>(read.bytes(), limits, &path) {
                        Ok(value) if valid(&value) => (
                            Some(value),
                            TargetSnapshot::File(read.snapshot().clone()),
                            None,
                        ),
                        Ok(_) | Err(StorageError::Corrupt { .. }) => {
                            let artifact = quarantine_file(
                                &location.directory,
                                &lock,
                                &location.name,
                                read.snapshot(),
                                QuarantineReason::Corrupt,
                                max_bytes,
                            )?;
                            (
                                None,
                                TargetSnapshot::Missing,
                                Some(artifact.path().to_path_buf()),
                            )
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            };
        let document = Self {
            location,
            lock,
            snapshot,
            limits,
            uncertain: false,
            uncertain_message,
        };
        Ok((document, value, quarantined))
    }

    /// True once a failed commit may have left disk ahead of memory.
    pub(crate) const fn is_uncertain(&self) -> bool {
        self.uncertain
    }

    /// True when the document exists as this writer last read or wrote it.
    pub(crate) const fn is_written(&self) -> bool {
        matches!(self.snapshot, TargetSnapshot::File(_))
    }

    /// The private directory holding the document.
    pub(crate) const fn directory(&self) -> &PrivateDirectory {
        &self.location.directory
    }

    /// Whether the small private marker file `name` exists beside the
    /// document (a one-time change it records as done).
    ///
    /// # Errors
    /// Unsafe storage: a link, a foreign mode or a file over the bound.
    pub(crate) fn has_marker(&self, name: &str) -> Result<bool, SettingsError> {
        Ok(read_bounded(&self.location.directory, name, MARKER_BYTES)?.is_some())
    }

    /// Durably writes the marker file `name` (which must not exist yet)
    /// beside the document, under this writer's lease.
    ///
    /// # Errors
    /// The marker already exists, the lease was lost, or the write failed.
    pub(crate) fn write_marker(&self, name: &str, bytes: &[u8]) -> Result<(), SettingsError> {
        atomic_write(
            &self.location.directory,
            &self.lock,
            name,
            bytes,
            &TargetSnapshot::Missing,
            MARKER_BYTES,
        )?;
        Ok(())
    }

    /// The bytes of the small private marker file `name` beside the
    /// document, or `None` when it does not exist.
    ///
    /// # Errors
    /// Unsafe storage: a link, a foreign mode or a file over the bound.
    pub(crate) fn read_marker(&self, name: &str) -> Result<Option<Vec<u8>>, SettingsError> {
        Ok(read_bounded(&self.location.directory, name, MARKER_BYTES)?
            .map(|file| file.into_parts().0))
    }

    /// Durably writes the marker file `name` beside the document, replacing
    /// the one there only if it is still the file just read, under this
    /// writer's lease.
    ///
    /// # Errors
    /// The marker changed meanwhile, the lease was lost, or the write failed.
    pub(crate) fn replace_marker(&self, name: &str, bytes: &[u8]) -> Result<(), SettingsError> {
        let expected = match read_bounded(&self.location.directory, name, MARKER_BYTES)? {
            Some(file) => TargetSnapshot::File(file.into_parts().1),
            None => TargetSnapshot::Missing,
        };
        atomic_write(
            &self.location.directory,
            &self.lock,
            name,
            bytes,
            &expected,
            MARKER_BYTES,
        )?;
        Ok(())
    }

    /// The refusal a latched document makes before any other check.
    pub(crate) fn ensure_writable(&self) -> Result<(), SettingsError> {
        if self.uncertain {
            return Err(SettingsError::InvalidAgent(self.uncertain_message.into()));
        }
        Ok(())
    }

    /// Durably replaces the document with `value`, which the caller installs
    /// in memory only after this returns `Ok`.
    pub(crate) fn commit<T: Serialize + DeserializeOwned>(
        &mut self,
        value: &T,
    ) -> Result<(), SettingsError> {
        self.ensure_writable()?;
        let path = self.location.directory.path().join(&self.location.name);
        // Preparation does no I/O: its refusals leave the target untouched.
        let prepared = prepare_json(value, self.limits, path)?;
        match prepared.write(
            &self.location.directory,
            &self.lock,
            &self.location.name,
            &self.snapshot,
        ) {
            Ok(written) => {
                self.snapshot = TargetSnapshot::File(written.into_parts().1);
                Ok(())
            }
            Err(error) => {
                self.uncertain |= latches(&error);
                Err(error.into())
            }
        }
    }
}

/// The most a marker file holds.
const MARKER_BYTES: usize = 1024;

/// Whether a failed [`interface_storage::PreparedJson::write`] may have left
/// disk different from memory. Only `WriteFailed` proves the target untouched:
/// `atomic_write` reports every failure before its rename that way, except
/// drift and a lost lease, and every failure after it as `WriteUncertain`.
const fn latches(error: &StorageError) -> bool {
    !matches!(error, StorageError::WriteFailed { .. })
}

/// Saves one document under a lease held only for this write: snapshot the
/// target, lease, write, release. `write_error` maps the write's own error
/// (its bounds refusal names the store's document); the snapshot, lease and
/// release errors stay storage errors.
pub(crate) fn write_document<T: Serialize + DeserializeOwned>(
    location: &StoreLocation,
    lock_name: &str,
    value: &T,
    limits: JsonLimits,
    write_error: impl FnOnce(StorageError) -> SettingsError,
) -> Result<(), SettingsError> {
    let expected = snapshot_target(&location.directory, &location.name, limits.max_file_bytes())?;
    let lock = WriterLock::acquire(&location.directory, lock_name)?;
    atomic_write_json(
        &location.directory,
        &lock,
        &location.name,
        value,
        &expected,
        limits,
    )
    .map_err(write_error)?;
    lock.release(&location.directory)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use interface_storage::{JsonLimits, StorageError};

    use super::{LeasedDocument, latches};
    use crate::SettingsError;

    const LATCHED: &str = "Test storage is uncertain.";

    fn open(file: &Path) -> (LeasedDocument, Option<Vec<String>>) {
        LeasedDocument::open(
            file,
            ".test.lock",
            JsonLimits::new(256, 4, 64).unwrap(),
            LATCHED,
        )
        .unwrap()
    }

    fn refused_as_latched(error: &SettingsError) -> bool {
        matches!(error, SettingsError::InvalidAgent(message) if message == LATCHED)
    }

    #[test]
    fn only_a_write_that_may_have_reached_disk_latches() {
        let path = PathBuf::from("/private/store.json");
        let reason = String::from("fixture");
        assert!(latches(&StorageError::WriteUncertain {
            path: path.clone(),
            reason: reason.clone(),
        }));
        assert!(latches(&StorageError::ExternalDrift { path: path.clone() }));
        assert!(latches(&StorageError::LockLost { path: path.clone() }));
        assert!(!latches(&StorageError::WriteFailed { path, reason }));
    }

    #[test]
    fn an_oversized_document_is_refused_without_latching() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("store.json");
        let (mut document, _) = open(&file);
        document.commit(&vec!["kept".to_owned()]).unwrap();
        let before = std::fs::read(&file).unwrap();
        let error = document.commit(&vec!["x".repeat(512)]).unwrap_err();
        assert!(
            matches!(
                error,
                SettingsError::Storage(StorageError::BoundsExceeded { .. })
            ),
            "{error:?}"
        );
        assert!(!document.is_uncertain());
        assert_eq!(std::fs::read(&file).unwrap(), before);
        document.commit(&vec!["next".to_owned()]).unwrap();
        drop(document);
        assert_eq!(open(&file).1, Some(vec!["next".to_owned()]));
    }

    #[test]
    fn external_drift_latches_and_keeps_the_foreign_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("store.json");
        let (mut document, _) = open(&file);
        document.commit(&vec!["mine".to_owned()]).unwrap();
        std::fs::write(&file, "[\"theirs\"]\n").unwrap();
        let error = document.commit(&vec!["next".to_owned()]).unwrap_err();
        assert!(
            matches!(
                error,
                SettingsError::Storage(StorageError::ExternalDrift { .. })
            ),
            "{error:?}"
        );
        assert!(document.is_uncertain());
        assert!(refused_as_latched(
            &document.commit(&vec!["again".to_owned()]).unwrap_err()
        ));
        assert_eq!(std::fs::read(&file).unwrap(), b"[\"theirs\"]\n");
    }

    #[test]
    fn a_lost_lease_latches() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("store.json");
        let (mut document, _) = open(&file);
        let lock = temp.path().join(".test.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(&lock, "").unwrap();
        let error = document.commit(&vec!["next".to_owned()]).unwrap_err();
        assert!(
            matches!(error, SettingsError::Storage(StorageError::LockLost { .. })),
            "{error:?}"
        );
        assert!(document.is_uncertain());
        assert!(refused_as_latched(
            &document.commit(&vec!["again".to_owned()]).unwrap_err()
        ));
        assert!(!file.exists());
    }

    /// A directory flagged immutable refuses the temporary file, so the atomic
    /// write fails before its rename with the target untouched.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_write_that_failed_before_its_rename_does_not_latch() {
        struct Immutable<'a>(&'a Path);
        impl Immutable<'_> {
            fn set(path: &Path, flag: &str) {
                let status = std::process::Command::new("/usr/bin/chflags")
                    .arg(flag)
                    .arg(path)
                    .status()
                    .unwrap();
                assert!(status.success());
            }
        }
        impl Drop for Immutable<'_> {
            fn drop(&mut self) {
                Self::set(self.0, "nouchg");
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("store.json");
        let (mut document, _) = open(&file);
        document.commit(&vec!["kept".to_owned()]).unwrap();
        let before = std::fs::read(&file).unwrap();
        {
            Immutable::set(temp.path(), "uchg");
            let _guard = Immutable(temp.path());
            let error = document.commit(&vec!["next".to_owned()]).unwrap_err();
            assert!(
                matches!(
                    error,
                    SettingsError::Storage(StorageError::WriteFailed { .. })
                ),
                "{error:?}"
            );
        }
        assert!(!document.is_uncertain());
        assert_eq!(std::fs::read(&file).unwrap(), before);
        document.commit(&vec!["next".to_owned()]).unwrap();
        drop(document);
        assert_eq!(open(&file).1, Some(vec!["next".to_owned()]));
    }
}
