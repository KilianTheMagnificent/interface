use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{Result, StorageError};
use crate::lock::WriterLock;
use crate::secure::{
    FileSnapshot, PrivateDirectory, TargetSnapshot, create_private_file, read_bounded,
    revalidate_named_file, set_private_file_mode, stable_regular_metadata, validate_bound,
    verify_expected_target,
};

const ATOMIC_TEMP_PREFIX: &str = ".interface-atomic-";
const ATOMIC_TEMP_SUFFIX: &str = ".tmp";

#[must_use]
pub fn is_atomic_temp_name(name: &str) -> bool {
    let middle = name
        .strip_prefix(ATOMIC_TEMP_PREFIX)
        .and_then(|value| value.strip_suffix(ATOMIC_TEMP_SUFFIX));
    middle.is_some_and(|value| {
        let mut parts = value.split('-');
        parts.next().is_some_and(|digest| {
            digest.len() == 16
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) && parts.clone().count() == 5
            && Uuid::parse_str(&value[17..]).is_ok()
    })
}

/// First 16 lowercase hex characters of `sha256(target)`, the digest segment
/// embedded in [`atomic_write`] temporary names.
#[must_use]
pub fn atomic_temp_digest_prefix(target: &str) -> String {
    let mut digest = format!("{:x}", Sha256::digest(target.as_bytes()));
    digest.truncate(16);
    digest
}

/// True only for an [`atomic_write`] temp whose digest matches `target`.
#[must_use]
pub fn is_atomic_temp_name_for(name: &str, target: &str) -> bool {
    is_atomic_temp_name(name)
        && name.get(ATOMIC_TEMP_PREFIX.len()..ATOMIC_TEMP_PREFIX.len() + 16)
            == Some(atomic_temp_digest_prefix(target).as_str())
}

/// Atomically installs bytes after verifying both the writer lease and the
/// exact previously-read target snapshot.
///
/// # Errors
///
/// Returns an error when bounds, path safety, the lease, target snapshot, or
/// any durability step cannot be proven. A post-rename failure is always
/// [`StorageError::WriteUncertain`].
pub fn atomic_write(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    name: &str,
    bytes: &[u8],
    expected: &TargetSnapshot,
    max_bytes: usize,
) -> Result<FileSnapshot> {
    validate_bound(max_bytes)?;
    if bytes.len() > max_bytes {
        return Err(StorageError::BoundsExceeded {
            path: directory.entry_path(name)?,
            max_bytes,
            actual_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }
    lock.assert_held_for(directory)?;
    verify_expected_target(directory, name, expected, max_bytes)?;
    let destination = directory.entry_path(name)?;
    let temp_name = temp_name(name);
    let temporary = directory.entry_path(&temp_name)?;
    let mut renamed = false;
    let mut created = None;

    let operation = (|| -> Result<FileSnapshot> {
        let mut file = create_private_file(&temporary)?;
        created = file
            .metadata()
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()));
        set_private_file_mode(&file, &temporary)?;
        file.write_all(bytes)
            .map_err(|error| StorageError::io("write atomic temporary file", &temporary, error))?;
        // The one full flush (`F_FULLFSYNC` on Apple), and it comes before the
        // rename: the new bytes are on stable storage before any name points
        // at them, so a power loss leaves the old file or the new one, never a
        // torn one. The same barrier-before-publish order as checkpoints.
        file.sync_all()
            .map_err(|error| StorageError::io("fsync atomic temporary file", &temporary, error))?;
        #[cfg(test)]
        tests::step("full flush temporary");
        let temporary_metadata = stable_regular_metadata(directory, &temporary, &file, max_bytes)?;
        if temporary_metadata.len != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
            return Err(StorageError::unsafe_at(
                &temporary,
                "temporary file length changed during write",
            ));
        }
        revalidate_named_file(directory, &temporary, &temporary_metadata, max_bytes)?;
        drop(file);
        #[cfg(test)]
        tests::BEFORE_INSTALL.with_borrow_mut(|hook| {
            if let Some(hook) = hook {
                hook();
            }
        });

        lock.assert_held_for(directory)?;
        verify_expected_target(directory, name, expected, max_bytes)?;
        fs::rename(&temporary, &destination).map_err(|error| {
            StorageError::io("atomically rename temporary file over", &destination, error)
        })?;
        renamed = true;
        #[cfg(test)]
        tests::step("rename");

        lock.assert_held_for(directory)?;
        let installed = read_bounded(directory, name, max_bytes)?.ok_or_else(|| {
            StorageError::unsafe_at(&destination, "renamed file is unexpectedly missing")
        })?;
        if installed.snapshot().identity() != temporary_metadata.identity
            || installed.bytes() != bytes
        {
            return Err(StorageError::unsafe_at(
                &destination,
                "installed inode or bytes do not match the synced temporary file",
            ));
        }
        // A plain fsync hands the rename to the device: it survives a process
        // or OS crash, and a power loss can at worst roll it back to the old
        // file until the drive next flushes its cache.
        directory.hand_off()?;
        #[cfg(test)]
        tests::step("fsync directory");
        lock.assert_held_for(directory)?;
        Ok(installed.snapshot().clone())
    })();

    if operation.is_err()
        && !renamed
        && let Some(created) = created
    {
        remove_own_temporary(&temporary, created);
    }
    match operation {
        Ok(snapshot) => Ok(snapshot),
        Err(error) if renamed => Err(StorageError::WriteUncertain {
            path: destination,
            reason: error.to_string(),
        }),
        Err(error @ (StorageError::ExternalDrift { .. } | StorageError::LockLost { .. })) => {
            Err(error)
        }
        Err(error) => Err(StorageError::WriteFailed {
            path: destination,
            reason: error.to_string(),
        }),
    }
}

/// Best-effort removal of a temporary this write created and never renamed.
/// The name is fresh and created exclusively, but the entry is unlinked only
/// while it is still that file; a failure leaves it for stale-temp recovery.
fn remove_own_temporary(temporary: &Path, created: (u64, u64)) {
    if fs::symlink_metadata(temporary)
        .is_ok_and(|metadata| (metadata.dev(), metadata.ino()) == created)
    {
        let _ = fs::remove_file(temporary);
    }
}

fn temp_name(target: &str) -> String {
    format!(
        "{ATOMIC_TEMP_PREFIX}{}-{}{ATOMIC_TEMP_SUFFIX}",
        atomic_temp_digest_prefix(target),
        Uuid::new_v4()
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::{PrivateDirectory, WriterLock, snapshot_target};

    thread_local! {
        /// Runs after the temporary is synced and before the target is
        /// checked again, so a test can make the install fail there.
        pub(super) static BEFORE_INSTALL: RefCell<Option<Box<dyn FnMut()>>> =
            const { RefCell::new(None) };
        /// The durability steps this thread's writes took, in order.
        static STEPS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn step(name: &'static str) {
        STEPS.with_borrow_mut(|steps| steps.push(name));
    }

    #[test]
    fn a_write_flushes_the_drive_once_before_the_rename() {
        let scratch = tempfile::tempdir().unwrap();
        let directory = PrivateDirectory::open_or_create(scratch.path().join("store")).unwrap();
        let lock = WriterLock::acquire(&directory, "writer.lock").unwrap();
        STEPS.with_borrow_mut(Vec::clear);
        atomic_write(
            &directory,
            &lock,
            "state.json",
            b"first",
            &crate::TargetSnapshot::Missing,
            1024,
        )
        .unwrap();
        assert_eq!(
            STEPS.with_borrow(Clone::clone),
            ["full flush temporary", "rename", "fsync directory"]
        );
        assert_eq!(
            fs::read(directory.path().join("state.json")).unwrap(),
            b"first"
        );
    }

    fn temporaries(directory: &PrivateDirectory) -> Vec<String> {
        fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| is_atomic_temp_name(name))
            .collect()
    }

    #[test]
    fn a_write_that_fails_before_the_rename_removes_its_temporary() {
        let scratch = tempfile::tempdir().unwrap();
        let directory = PrivateDirectory::open_or_create(scratch.path().join("store")).unwrap();
        let lock = WriterLock::acquire(&directory, "writer.lock").unwrap();
        let installed = atomic_write(
            &directory,
            &lock,
            "state.json",
            b"first",
            &crate::TargetSnapshot::Missing,
            1024,
        )
        .unwrap();
        let expected = snapshot_target(&directory, "state.json", 1024).unwrap();
        let crate::TargetSnapshot::File(current) = &expected else {
            panic!("the first write left no target");
        };
        assert_eq!(installed.identity(), current.identity());

        // Another writer changes the target between the two checks.
        let target = directory.path().join("state.json");
        BEFORE_INSTALL.set(Some(Box::new(move || {
            fs::write(&target, b"external").unwrap();
        })));
        let outcome = atomic_write(&directory, &lock, "state.json", b"ours", &expected, 1024);
        BEFORE_INSTALL.set(None);

        assert!(matches!(outcome, Err(StorageError::ExternalDrift { .. })));
        assert_eq!(
            fs::read(directory.path().join("state.json")).unwrap(),
            b"external"
        );
        assert!(temporaries(&directory).is_empty());
    }

    #[test]
    fn a_temporary_replaced_under_its_name_is_left_alone() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("temporary");
        fs::write(&path, b"ours").unwrap();
        let metadata = fs::symlink_metadata(&path).unwrap();
        let created = (metadata.dev(), metadata.ino());
        let replacement = scratch.path().join("replacement");
        fs::write(&replacement, b"someone else's").unwrap();
        fs::rename(&replacement, &path).unwrap();

        remove_own_temporary(&path, created);
        assert_eq!(fs::read(&path).unwrap(), b"someone else's");

        let metadata = fs::symlink_metadata(&path).unwrap();
        remove_own_temporary(&path, (metadata.dev(), metadata.ino()));
        assert!(!path.exists());
    }
}
