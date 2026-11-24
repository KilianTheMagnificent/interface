use std::io;
use std::path::{Path, PathBuf};

// fs2::FileExt stays until MSRV ≥ 1.89. std::fs::File::try_lock then returns
// `io::Result<bool>` (`Ok(false)` = held by another handle), not
// `Err(WouldBlock)`. Held-lock arms must switch to
// `Ok(false) => Err(StorageError::LockHeld)`.
use fs2::FileExt;

use crate::error::{Result, StorageError};
use crate::secure::{
    FileIdentity, PrivateDirectory, StableFileMetadata, create_private_file, open_regular,
    revalidate_named_file, set_private_file_mode, stable_regular_metadata,
};

const MAX_LOCK_BYTES: usize = 1;

/// A process-lifetime advisory exclusive lease over one private store directory.
///
/// The lock file is deliberately persistent. Unlinking a lock on release can
/// split ownership between an old open inode and a newly-created same-name inode.
#[derive(Debug)]
pub struct WriterLock {
    lock_directory: PrivateDirectory,
    protected_directory_identity: FileIdentity,
    path: PathBuf,
    file: std::fs::File,
    metadata: StableFileMetadata,
    held: bool,
}

impl WriterLock {
    /// Acquires an exclusive advisory lease over a stable, persistent lock inode.
    ///
    /// # Errors
    ///
    /// Returns an error when the lock path is unsafe, another writer holds the
    /// lease, or creation and fsync of a new lock inode cannot be proven.
    pub fn acquire(directory: &PrivateDirectory, name: &str) -> Result<Self> {
        Self::acquire_for(directory, name, directory)
    }

    /// Acquires a lock inode in `lock_directory` that protects a different
    /// private directory.
    ///
    /// This is needed for legacy journal layouts whose committed-record
    /// directory may contain only record and temporary files. Keeping the
    /// persistent Rust lock in the journal's parent preserves rollback
    /// compatibility while retaining the same descriptor-bound lease checks.
    ///
    /// # Errors
    ///
    /// Returns an error when either directory is unsafe, the lock path is
    /// unsafe, another writer owns the lease, or creation cannot be proven.
    pub fn acquire_for(
        lock_directory: &PrivateDirectory,
        name: &str,
        protected_directory: &PrivateDirectory,
    ) -> Result<Self> {
        lock_directory.assert_unchanged()?;
        protected_directory.assert_unchanged()?;
        let path = lock_directory.entry_path(name)?;
        // Existing persistent lock inodes are opened read-only. `flock` does
        // not require write access, and a writable open can itself mutate
        // macOS provenance metadata before the descriptor can be verified.
        let opened = open_regular(lock_directory, &path, MAX_LOCK_BYTES)?;
        let (file, metadata) = match opened {
            Some(opened) => (opened.file, opened.metadata),
            None => match create_private_file(&path) {
                Ok(file) => {
                    set_private_file_mode(&file, &path)?;
                    file.sync_all()
                        .map_err(|error| StorageError::io("fsync writer lock", &path, error))?;
                    let metadata =
                        stable_regular_metadata(lock_directory, &path, &file, MAX_LOCK_BYTES)?;
                    revalidate_named_file(lock_directory, &path, &metadata, MAX_LOCK_BYTES)?;
                    lock_directory.sync()?;
                    (file, metadata)
                }
                Err(error)
                    if matches!(
                        &error,
                        StorageError::Io { source, .. }
                            if source.kind() == io::ErrorKind::AlreadyExists
                    ) =>
                {
                    let opened = open_regular(lock_directory, &path, MAX_LOCK_BYTES)?
                        .ok_or_else(|| StorageError::ExternalDrift { path: path.clone() })?;
                    (opened.file, opened.metadata)
                }
                Err(error) => return Err(error),
            },
        };

        // The pattern above handles the EEXIST race, but a freshly created file
        // needs exact permissions and durability before it becomes a lock inode.
        if metadata.len != 0 {
            return Err(StorageError::unsafe_at(
                &path,
                "writer lock file must be empty",
            ));
        }

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Err(StorageError::LockHeld { path });
            }
            Err(error) => return Err(StorageError::io("acquire writer lock", &path, error)),
        }

        let after_lock = stable_regular_metadata(lock_directory, &path, &file, MAX_LOCK_BYTES)?;
        if after_lock.len != 0 || after_lock != metadata {
            let _ = FileExt::unlock(&file);
            return Err(StorageError::LockLost { path });
        }
        revalidate_named_file(lock_directory, &path, &after_lock, MAX_LOCK_BYTES)?;
        lock_directory.assert_unchanged()?;
        protected_directory.assert_unchanged()?;
        let lock_directory = PrivateDirectory::open(lock_directory.path())?;
        Ok(Self {
            lock_directory,
            protected_directory_identity: protected_directory.identity(),
            path,
            file,
            metadata: after_lock,
            held: true,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// # Errors
    ///
    /// Returns [`StorageError::LockLost`] when the lease was released, belongs
    /// to another directory, or its named inode/metadata changed.
    pub fn assert_held_for(&self, directory: &PrivateDirectory) -> Result<()> {
        if !self.held || self.protected_directory_identity != directory.identity() {
            return Err(StorageError::LockLost {
                path: self.path.clone(),
            });
        }
        directory.assert_unchanged()?;
        self.lock_directory.assert_unchanged()?;
        let current =
            stable_regular_metadata(&self.lock_directory, &self.path, &self.file, MAX_LOCK_BYTES)
                .map_err(|_| StorageError::LockLost {
                path: self.path.clone(),
            })?;
        if current != self.metadata || current.len != 0 {
            return Err(StorageError::LockLost {
                path: self.path.clone(),
            });
        }
        revalidate_named_file(&self.lock_directory, &self.path, &current, MAX_LOCK_BYTES).map_err(
            |_| StorageError::LockLost {
                path: self.path.clone(),
            },
        )
    }

    /// Releases the advisory lease without unlinking its stable lock inode.
    ///
    /// # Errors
    ///
    /// Returns an error when final lease validation or OS unlock fails. The
    /// file descriptor is still dropped so an advisory lock is not leaked.
    pub fn release(mut self, directory: &PrivateDirectory) -> Result<()> {
        let validation = self.assert_held_for(directory);
        let unlock = FileExt::unlock(&self.file)
            .map_err(|error| StorageError::io("release writer lock", &self.path, error));
        self.held = false;
        validation?;
        unlock
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        if self.held {
            let _ = FileExt::unlock(&self.file);
            self.held = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn existing_lock_inode_is_reopened_read_only() {
        let scratch = tempfile::tempdir().expect("temporary directory");
        let directory =
            PrivateDirectory::open_or_create(scratch.path().join("store")).expect("private store");

        WriterLock::acquire(&directory, "writer.lock")
            .expect("create writer lock")
            .release(&directory)
            .expect("release created writer lock");

        let mut reopened =
            WriterLock::acquire(&directory, "writer.lock").expect("reopen existing writer lock");
        assert!(
            reopened.file.write_all(b"x").is_err(),
            "an existing lock descriptor must not allow writes"
        );
        reopened
            .assert_held_for(&directory)
            .expect("read-only descriptor keeps the lease valid");
        reopened
            .release(&directory)
            .expect("release reopened writer lock");
    }
}
