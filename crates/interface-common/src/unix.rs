//! Unix identity checks the owner-private stores run before trusting a file.
//!
//! `unsafe_code` is forbidden workspace-wide; the effective uid comes from
//! rustix's safe `geteuid` wrapper. Each caller maps a failure to its own
//! refusal; nothing here decides what a mismatch means.

use std::fmt;
use std::fs::Metadata;
use std::io;
use std::os::unix::fs::MetadataExt;

/// Why [`effective_uid`] could not name the current user. `geteuid` cannot
/// fail, so no variant is produced today; callers keep mapping all of them,
/// with the messages the earlier `/usr/bin/id -u` lookup printed.
#[derive(Debug)]
pub enum EffectiveUidError {
    /// `/usr/bin/id` could not be run.
    Spawn(io::Error),
    /// `/usr/bin/id -u` exited unsuccessfully.
    Failed,
    /// Its output was not UTF-8.
    NotText(std::str::Utf8Error),
    /// Its output was not a decimal uid.
    NotUid(std::num::ParseIntError),
}

impl fmt::Display for EffectiveUidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => error.fmt(formatter),
            Self::Failed => formatter.write_str("Unable to determine the current user id."),
            Self::NotText(error) => error.fmt(formatter),
            Self::NotUid(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for EffectiveUidError {}

/// The effective uid of this process.
///
/// # Errors
///
/// Never in practice: `geteuid` always succeeds. The `Result` keeps every
/// caller's refusal path for an unknown user.
pub fn effective_uid() -> Result<u32, EffectiveUidError> {
    Ok(rustix::process::geteuid().as_raw())
}

/// `left` and `right` describe the same unchanged file: device and inode, size,
/// mode, owner uid, link count, and ctime and mtime to the nanosecond.
///
/// The group is deliberately not compared; a caller that also pins it adds
/// `left.gid() == right.gid()`.
#[must_use]
pub fn same_stable_metadata(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mode() == right.mode()
        && left.uid() == right.uid()
        && left.nlink() == right.nlink()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    use super::{EffectiveUidError, effective_uid, same_stable_metadata};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "interface-common-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::write(&path, b"first").unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn effective_uid_owns_the_files_this_process_creates() {
        let scratch = Scratch::new("uid");
        let owner = fs::symlink_metadata(&scratch.0).unwrap().uid();
        assert_eq!(effective_uid().unwrap(), owner);
        // The cached answer is the same answer.
        assert_eq!(effective_uid().unwrap(), owner);
    }

    #[test]
    fn effective_uid_errors_keep_the_messages_the_lock_file_check_printed() {
        assert_eq!(
            EffectiveUidError::Failed.to_string(),
            "Unable to determine the current user id."
        );
        let not_uid = "x".parse::<u32>().unwrap_err();
        assert_eq!(
            EffectiveUidError::NotUid(not_uid.clone()).to_string(),
            not_uid.to_string()
        );
        let spawn = std::io::Error::other("no id");
        assert_eq!(EffectiveUidError::Spawn(spawn).to_string(), "no id");
    }

    #[test]
    fn stable_metadata_sees_the_same_file_and_refuses_a_rewrite_or_another_file() {
        let scratch = Scratch::new("stable");
        let other = Scratch::new("other");
        let before = fs::symlink_metadata(&scratch.0).unwrap();
        assert!(same_stable_metadata(
            &before,
            &fs::symlink_metadata(&scratch.0).unwrap()
        ));
        assert!(!same_stable_metadata(
            &before,
            &fs::symlink_metadata(&other.0).unwrap()
        ));
        fs::write(&scratch.0, b"a longer second body").unwrap();
        assert!(!same_stable_metadata(
            &before,
            &fs::symlink_metadata(&scratch.0).unwrap()
        ));
    }
}
