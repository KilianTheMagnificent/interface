//! The one commit protocol the collaboration snapshot stores share: a writer
//! lease, strict preparation before any I/O, the exact snapshot of the last
//! write, and a failure split that tells a store whether disk may have moved.
//!
//! Each store keeps its own validation, revision rule and latch policy (the
//! persisted `storage_uncertain` flag, the Away pilot's poison, the workers'
//! rollback to the committed index) and maps [`WriteError`] itself.
use std::fmt;

use interface_storage::{
    JsonLimits, PreparedJson, PrivateDirectory, StorageError, TargetSnapshot, WriterLock,
    prepare_json, read_json,
};
use serde::{Serialize, de::DeserializeOwned};

/// Why a snapshot write failed. Displays as the storage error it carries.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// Nothing reached the target, so disk still equals memory: preparation
    /// refused the value (over its bounds or not strictly representable) or
    /// the atomic write failed before its rename.
    Refused(StorageError),
    /// Disk may differ from memory: drift, a lost lease, a failure after the
    /// rename, or unsafe target state met while writing.
    Uncertain(StorageError),
}

impl WriteError {
    pub(crate) const fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(error) | Self::Uncertain(error) => error.fmt(formatter),
        }
    }
}

/// Reads `name` strictly: the value, moved out rather than copied, and the
/// snapshot the next write must still find (`Missing` when absent).
pub(crate) fn read_snapshot<T: DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &str,
    limits: JsonLimits,
) -> Result<(Option<T>, TargetSnapshot), StorageError> {
    Ok(match read_json::<T>(directory, name, limits)? {
        Some(loaded) => {
            let (value, snapshot) = loaded.into_parts();
            (Some(value), TargetSnapshot::File(snapshot))
        }
        None => (None, TargetSnapshot::Missing),
    })
}

/// Durably replaces `name` with `next` if it still matches `file`, then
/// advances `file`. Returns the strict-decoded copy of what was written.
pub(crate) fn write_snapshot<T: Serialize + DeserializeOwned>(
    directory: &PrivateDirectory,
    writer: &WriterLock,
    name: &str,
    next: &T,
    file: &mut TargetSnapshot,
    limits: JsonLimits,
) -> Result<T, WriteError> {
    let prepared = prepare(directory, name, next, limits)?;
    install(directory, writer, name, prepared, file)
}

/// Encodes and strictly re-decodes `next` without any I/O, so every refusal
/// here leaves the target as it is.
fn prepare<T: Serialize + DeserializeOwned>(
    directory: &PrivateDirectory,
    name: &str,
    next: &T,
    limits: JsonLimits,
) -> Result<PreparedJson<T>, WriteError> {
    prepare_json(next, limits, directory.path().join(name)).map_err(WriteError::Refused)
}

fn install<T>(
    directory: &PrivateDirectory,
    writer: &WriterLock,
    name: &str,
    prepared: PreparedJson<T>,
    file: &mut TargetSnapshot,
) -> Result<T, WriteError> {
    let (value, snapshot) = prepared
        .write(directory, writer, name, file)
        .map_err(classify)?
        .into_parts();
    *file = TargetSnapshot::File(snapshot);
    Ok(value)
}

/// Splits a failed install. `atomic_write` reports every failure before its
/// rename as `WriteFailed`, except drift and a lost lease, and every failure
/// after it as `WriteUncertain`.
fn classify(error: StorageError) -> WriteError {
    match error {
        StorageError::WriteFailed { .. } => WriteError::Refused(error),
        error => WriteError::Uncertain(error),
    }
}

/// One snapshot file held under a writer lease for the store's lifetime.
pub(crate) struct SnapshotFile {
    directory: PrivateDirectory,
    writer: WriterLock,
    file: TargetSnapshot,
    name: &'static str,
    limits: JsonLimits,
}

impl SnapshotFile {
    /// Leases `directory` under `lock_name`, then reads `name` strictly.
    pub(crate) fn open<T: DeserializeOwned>(
        directory: PrivateDirectory,
        lock_name: &str,
        name: &'static str,
        limits: JsonLimits,
    ) -> Result<(Self, Option<T>), StorageError> {
        let writer = WriterLock::acquire(&directory, lock_name)?;
        let (value, file) = read_snapshot(&directory, name, limits)?;
        let snapshot = Self {
            directory,
            writer,
            file,
            name,
            limits,
        };
        Ok((snapshot, value))
    }

    pub(crate) const fn directory(&self) -> &PrivateDirectory {
        &self.directory
    }

    /// Lowers the committed file's cap; reads keep the cap `open` used.
    #[cfg(test)]
    pub(crate) fn set_limits(&mut self, limits: JsonLimits) {
        self.limits = limits;
    }

    /// Durably replaces the file with the state a store keeps editing in place.
    pub(crate) fn write<T: Serialize + DeserializeOwned>(
        &mut self,
        next: &T,
    ) -> Result<T, WriteError> {
        write_snapshot(
            &self.directory,
            &self.writer,
            self.name,
            next,
            &mut self.file,
            self.limits,
        )
    }

    /// Durably replaces the file with a clone-edit-commit successor, freeing
    /// `next` before any I/O. Returns its strict-decoded copy to install.
    pub(crate) fn replace<T: Serialize + DeserializeOwned>(
        &mut self,
        next: T,
    ) -> Result<T, WriteError> {
        self.replace_within(next, usize::MAX)
    }

    /// [`Self::replace`] for a successor that must also stay within
    /// `max_bytes`, which may be below the file cap: over it, the write is
    /// refused as `BoundsExceeded` before any I/O. The file cap still governs
    /// reading and checking the file being replaced.
    pub(crate) fn replace_within<T: Serialize + DeserializeOwned>(
        &mut self,
        next: T,
        max_bytes: usize,
    ) -> Result<T, WriteError> {
        let prepared = prepare(&self.directory, self.name, &next, self.limits)?;
        drop(next);
        if prepared.byte_len() > max_bytes {
            return Err(WriteError::Refused(StorageError::BoundsExceeded {
                path: self.directory.path().join(self.name),
                max_bytes,
                actual_bytes: u64::try_from(prepared.byte_len()).unwrap_or(u64::MAX),
            }));
        }
        install(
            &self.directory,
            &self.writer,
            self.name,
            prepared,
            &mut self.file,
        )
    }
}

/// Flags a directory immutable (macOS `uchg`) until dropped, so creating an
/// atomic write's temporary file fails before its rename.
#[cfg(all(test, target_os = "macos"))]
pub(crate) struct ImmutableDirectory<'a>(&'a std::path::Path);

#[cfg(all(test, target_os = "macos"))]
impl<'a> ImmutableDirectory<'a> {
    pub(crate) fn new(path: &'a std::path::Path) -> Self {
        Self::chflags("uchg", path);
        Self(path)
    }

    fn chflags(flag: &str, path: &std::path::Path) {
        let status = std::process::Command::new("/usr/bin/chflags")
            .arg(flag)
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success());
    }
}

#[cfg(all(test, target_os = "macos"))]
impl Drop for ImmutableDirectory<'_> {
    fn drop(&mut self) {
        Self::chflags("nouchg", self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use interface_storage::{JsonLimits, PrivateDirectory, StorageError};

    use super::{SnapshotFile, WriteError, classify};

    fn open(root: &std::path::Path) -> (SnapshotFile, Option<Vec<String>>) {
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).unwrap();
        SnapshotFile::open(
            PrivateDirectory::open(root).unwrap(),
            "test.lock",
            "test.json",
            JsonLimits::new(256, 4, 64).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn only_a_write_that_failed_before_its_rename_is_refused() {
        let path = std::path::PathBuf::from("/private/store.json");
        let reason = String::from("fixture");
        for error in [
            StorageError::WriteUncertain {
                path: path.clone(),
                reason: reason.clone(),
            },
            StorageError::ExternalDrift { path: path.clone() },
            StorageError::LockLost { path: path.clone() },
        ] {
            assert!(classify(error).is_uncertain());
        }
        assert!(!classify(StorageError::WriteFailed { path, reason }).is_uncertain());
    }

    #[test]
    fn an_oversized_value_is_refused_and_nothing_is_written() {
        let root = tempfile::tempdir().unwrap();
        let (mut file, _) = open(root.path());
        file.write(&vec!["kept".to_owned()]).unwrap();
        let before = std::fs::read(root.path().join("test.json")).unwrap();
        let error = file.write(&vec!["x".repeat(512)]).unwrap_err();
        assert!(
            matches!(
                error,
                WriteError::Refused(StorageError::BoundsExceeded { .. })
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(root.path().join("test.json")).unwrap(),
            before
        );
        assert_eq!(
            file.write(&vec!["next".to_owned()]).unwrap(),
            vec!["next".to_owned()]
        );
    }

    #[test]
    fn a_value_over_a_lower_budget_is_refused_before_any_io() {
        let root = tempfile::tempdir().unwrap();
        let (mut file, _) = open(root.path());
        file.write(&vec!["kept".to_owned()]).unwrap();
        let before = std::fs::read(root.path().join("test.json")).unwrap();
        // Within the 256-byte file cap, but over the 32-byte budget.
        let error = file.replace_within(vec!["x".repeat(40)], 32).unwrap_err();
        assert!(
            matches!(
                error,
                WriteError::Refused(StorageError::BoundsExceeded { max_bytes: 32, .. })
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(root.path().join("test.json")).unwrap(),
            before
        );
        assert_eq!(
            file.replace_within(vec!["next".to_owned()], 32).unwrap(),
            vec!["next".to_owned()]
        );
    }

    #[test]
    fn drift_and_a_lost_lease_are_uncertain() {
        let root = tempfile::tempdir().unwrap();
        let (mut file, _) = open(root.path());
        file.write(&vec!["mine".to_owned()]).unwrap();
        std::fs::write(root.path().join("test.json"), "[\"theirs\"]\n").unwrap();
        let error = file.write(&vec!["next".to_owned()]).unwrap_err();
        assert!(
            matches!(
                error,
                WriteError::Uncertain(StorageError::ExternalDrift { .. })
            ),
            "{error:?}"
        );
        assert_eq!(
            std::fs::read(root.path().join("test.json")).unwrap(),
            b"[\"theirs\"]\n"
        );

        let root = tempfile::tempdir().unwrap();
        let (mut file, _) = open(root.path());
        let lock = root.path().join("test.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(&lock, "").unwrap();
        let error = file.write(&vec!["next".to_owned()]).unwrap_err();
        assert!(
            matches!(error, WriteError::Uncertain(StorageError::LockLost { .. })),
            "{error:?}"
        );
        assert!(error.is_uncertain());
    }

    /// A directory flagged immutable refuses the temporary file, so the atomic
    /// write fails before its rename with the target untouched.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_write_that_failed_before_its_rename_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let (mut file, _) = open(root.path());
        file.write(&vec!["kept".to_owned()]).unwrap();
        let before = std::fs::read(root.path().join("test.json")).unwrap();
        {
            let _immutable = super::ImmutableDirectory::new(root.path());
            let error = file.write(&vec!["next".to_owned()]).unwrap_err();
            assert!(
                matches!(error, WriteError::Refused(StorageError::WriteFailed { .. })),
                "{error:?}"
            );
            assert!(!error.is_uncertain());
        }
        assert_eq!(
            std::fs::read(root.path().join("test.json")).unwrap(),
            before
        );
        file.write(&vec!["next".to_owned()]).unwrap();
    }
}
