//! Pinning a directory by device and inode: the capture-then-recheck that the
//! review runner and Review checkpoints share before trusting a path.
//!
//! Each caller absolutizes the path its own way and maps [`DirectoryChanged`]
//! to its own error; the proof itself is identical.

use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// A file's device and inode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    #[must_use]
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
}

/// The directory is missing, is not a real directory, is reached through a
/// link, or is no longer the directory first seen at that path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryChanged;

/// A real directory at its own canonical path, pinned by identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectorySnapshot {
    pub path: PathBuf,
    identity: FileIdentity,
}

impl DirectorySnapshot {
    /// Pins `requested`, an absolute path, which must name a directory (not a
    /// link) that is its own canonical path, with the same identity before and
    /// after canonicalizing.
    ///
    /// # Errors
    ///
    /// Returns [`DirectoryChanged`] when any of that does not hold.
    pub fn capture(requested: &Path) -> Result<Self, DirectoryChanged> {
        let before = fs::symlink_metadata(requested).map_err(|_| DirectoryChanged)?;
        if !before.is_dir() || before.file_type().is_symlink() {
            return Err(DirectoryChanged);
        }
        let canonical = fs::canonicalize(requested).map_err(|_| DirectoryChanged)?;
        let after = fs::symlink_metadata(&canonical).map_err(|_| DirectoryChanged)?;
        if canonical != requested
            || !after.is_dir()
            || after.file_type().is_symlink()
            || FileIdentity::of(&before) != FileIdentity::of(&after)
        {
            return Err(DirectoryChanged);
        }
        Ok(Self {
            path: canonical,
            identity: FileIdentity::of(&after),
        })
    }

    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    /// The path still names this exact directory.
    ///
    /// # Errors
    ///
    /// Returns [`DirectoryChanged`] when it no longer does.
    pub fn assert_unchanged(&self) -> Result<(), DirectoryChanged> {
        if Self::capture(&self.path)?.identity == self.identity {
            Ok(())
        } else {
            Err(DirectoryChanged)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_is_pinned_until_it_is_replaced_or_linked() {
        let root = std::env::temp_dir().join(format!(
            "interface-common-fsid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let directory = root.join("pinned");
        fs::create_dir_all(&directory).unwrap();
        let canonical = fs::canonicalize(&directory).unwrap();
        let snapshot = DirectorySnapshot::capture(&canonical).unwrap();
        assert_eq!(snapshot.path, canonical);
        snapshot.assert_unchanged().unwrap();

        // A link to it, a non-canonical spelling and a file are all refused.
        let link = canonical.with_file_name("link");
        std::os::unix::fs::symlink(&canonical, &link).unwrap();
        assert_eq!(DirectorySnapshot::capture(&link), Err(DirectoryChanged));
        assert_eq!(
            DirectorySnapshot::capture(&canonical.join("..").join("pinned")),
            Err(DirectoryChanged)
        );
        let file = canonical.with_file_name("file");
        fs::write(&file, b"x").unwrap();
        assert_eq!(DirectorySnapshot::capture(&file), Err(DirectoryChanged));

        // A replacement at the same path is a different directory.
        fs::rename(&canonical, canonical.with_file_name("parked")).unwrap();
        fs::create_dir(&canonical).unwrap();
        assert_eq!(snapshot.assert_unchanged(), Err(DirectoryChanged));
        fs::remove_dir_all(&root).unwrap();
    }
}
