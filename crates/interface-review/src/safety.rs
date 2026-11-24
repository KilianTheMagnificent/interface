use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path};
use std::time::Duration;

pub(crate) use interface_common::fsid::DirectorySnapshot;
use interface_common::{fsid::FileIdentity, paths::lexically_absolute};
use thiserror::Error;

use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileSnapshot {
    identity: FileIdentity,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    links: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedFile {
    pub content: Vec<u8>,
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum SafetyError {
    #[error("the authorized root changed")]
    PathChanged,
    #[error("the candidate is a symbolic link")]
    Symlink,
    #[error("the candidate is not a regular file")]
    NotRegular,
    #[error("the candidate has another hard link")]
    HardLink,
    #[error("the candidate exceeded its byte limit")]
    OutputLimit,
    #[error("the candidate could not be read")]
    Unreadable,
    /// The read did not finish within [`VERIFIED_READ_LIMIT`]: most likely
    /// the file is still downloading from iCloud.
    #[error("the candidate is still downloading from iCloud")]
    Downloading,
}

type ParentValidatedHook<'a> = Option<&'a dyn Fn(&Path, &Path)>;
/// Test seam: runs on the candidate after it is validated, right before the
/// bounded open.
pub(crate) type BeforeOpenHook<'a> = Option<&'a dyn Fn(&Path)>;

/// How long one verified project file read may take. A local file of the
/// sizes reviewed reads in milliseconds; one iCloud has evicted (dataless)
/// waits for its download, so the read is bounded rather than the review.
pub(crate) const VERIFIED_READ_LIMIT: Duration = Duration::from_secs(5);

fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity::of(metadata)
}

fn snapshot(metadata: &Metadata) -> FileSnapshot {
    FileSnapshot {
        identity: identity(metadata),
        size: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
        links: metadata.nlink(),
    }
}

fn link_count(metadata: &Metadata) -> u64 {
    metadata.nlink()
}

fn executable(metadata: &Metadata) -> bool {
    metadata.mode() & 0o111 != 0
}

pub(crate) fn canonical_directory(path: &Path) -> Result<DirectorySnapshot, SafetyError> {
    let requested = lexically_absolute(path).map_err(|_| SafetyError::PathChanged)?;
    DirectorySnapshot::capture(&requested).map_err(|_| SafetyError::PathChanged)
}

pub(crate) fn assert_directory(snapshot: &DirectorySnapshot) -> Result<(), SafetyError> {
    snapshot
        .assert_unchanged()
        .map_err(|_| SafetyError::PathChanged)
}

fn stable_child_directory(
    root: &DirectorySnapshot,
    path: &Path,
) -> Result<DirectorySnapshot, SafetyError> {
    let child = canonical_directory(path)?;
    if !child.path.starts_with(&root.path) {
        return Err(SafetyError::PathChanged);
    }
    assert_directory(root)?;
    Ok(child)
}

fn open_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW);
    options.open(path)
}

/// Opens `path` without following a link and reads at most `maximum` bytes
/// (one more proves it grew past the limit), keeping the descriptor for the
/// caller's identity checks.
fn read_opened(path: &Path, maximum: usize) -> Result<(File, Metadata, Vec<u8>), SafetyError> {
    let mut file = open_no_follow(path).map_err(|_| SafetyError::Unreadable)?;
    let opened = file.metadata().map_err(|_| SafetyError::Unreadable)?;
    if opened.len() > u64::try_from(maximum).unwrap_or(u64::MAX) {
        return Err(SafetyError::OutputLimit);
    }
    let mut content = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|_| SafetyError::Unreadable)?;
    Ok((file, opened, content))
}

pub(crate) fn read_verified_file(
    root: &DirectorySnapshot,
    relative: &Path,
    maximum: usize,
    reject_hard_links: bool,
    after_parent: ParentValidatedHook<'_>,
    before_open: BeforeOpenHook<'_>,
) -> Result<VerifiedFile, SafetyError> {
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir
                    | Component::CurDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
    {
        return Err(SafetyError::PathChanged);
    }
    let target = root.path.join(relative);
    if !target.starts_with(&root.path) {
        return Err(SafetyError::PathChanged);
    }
    assert_directory(root)?;
    let parent = stable_child_directory(root, target.parent().ok_or(SafetyError::PathChanged)?)?;
    if let Some(hook) = after_parent {
        hook(&parent.path, &target);
    }
    assert_directory(root)?;
    assert_directory(&parent)?;

    let before = fs::symlink_metadata(&target).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            SafetyError::NotRegular
        } else {
            SafetyError::Unreadable
        }
    })?;
    if before.file_type().is_symlink() {
        return Err(SafetyError::Symlink);
    }
    if !before.is_file() {
        return Err(SafetyError::NotRegular);
    }
    if reject_hard_links && link_count(&before) != 1 {
        return Err(SafetyError::HardLink);
    }
    if before.len() > u64::try_from(maximum).unwrap_or(u64::MAX) {
        return Err(SafetyError::OutputLimit);
    }
    let before_snapshot = snapshot(&before);

    assert_directory(root)?;
    assert_directory(&parent)?;
    if let Some(hook) = before_open {
        hook(&target);
    }
    // The open and the read are the only steps that touch the file's
    // contents, so they are the ones that wait for an evicted file. They run
    // through the bounded project read; every identity check below still
    // runs on what they answer, and a mismatch discards the bytes.
    let reading = target.clone();
    let (file, opened, content) =
        interface_common::bounded_read::within(VERIFIED_READ_LIMIT, move || {
            read_opened(&reading, maximum)
        })
        .map_err(|_| SafetyError::Downloading)??;
    let after = fs::symlink_metadata(&target).map_err(|_| SafetyError::PathChanged)?;
    if !opened.is_file()
        || opened.file_type().is_symlink()
        || (reject_hard_links && link_count(&opened) != 1)
        || snapshot(&opened) != before_snapshot
        || snapshot(&after) != before_snapshot
    {
        return Err(SafetyError::PathChanged);
    }
    assert_directory(&parent)?;
    assert_directory(root)?;

    let size = usize::try_from(opened.len()).map_err(|_| SafetyError::OutputLimit)?;
    if size > maximum {
        return Err(SafetyError::OutputLimit);
    }
    if content.len() != size {
        return Err(SafetyError::PathChanged);
    }
    let final_opened = file.metadata().map_err(|_| SafetyError::Unreadable)?;
    let final_path = fs::symlink_metadata(&target).map_err(|_| SafetyError::PathChanged)?;
    if final_path.file_type().is_symlink()
        || snapshot(&final_opened) != before_snapshot
        || snapshot(&final_path) != before_snapshot
    {
        return Err(SafetyError::PathChanged);
    }
    assert_directory(&parent)?;
    assert_directory(root)?;
    Ok(VerifiedFile {
        content,
        executable: executable(&opened),
    })
}
