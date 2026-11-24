use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Result, StorageError};
use crate::{HARD_MAX_FILE_BYTES, OWNER_DIRECTORY_MODE, OWNER_FILE_MODE};

use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FileIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSnapshot {
    identity: FileIdentity,
    len: u64,
    sha256: [u8; 32],
}

impl FileSnapshot {
    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetSnapshot {
    Missing,
    File(FileSnapshot),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedFile {
    bytes: Vec<u8>,
    snapshot: FileSnapshot,
}

impl BoundedFile {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn snapshot(&self) -> &FileSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, FileSnapshot) {
        (self.bytes, self.snapshot)
    }
}

#[derive(Debug)]
pub struct PrivateDirectory {
    path: PathBuf,
    handle: File,
    identity: FileIdentity,
    owner_uid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StableFileMetadata {
    pub(crate) identity: FileIdentity,
    pub(crate) len: u64,
    pub(crate) mode: u32,
    pub(crate) uid: u32,
    pub(crate) links: u64,
    pub(crate) modified_seconds: i64,
    pub(crate) modified_nanoseconds: i64,
    pub(crate) changed_seconds: i64,
    pub(crate) changed_nanoseconds: i64,
}

pub(crate) struct VerifiedFile {
    pub(crate) file: File,
    pub(crate) metadata: StableFileMetadata,
}

impl PrivateDirectory {
    /// Opens an existing private directory without changing it.
    ///
    /// # Errors
    ///
    /// Returns an error unless the path is an absolute, real, mode-0700
    /// directory whose device and inode remain stable while being opened.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_owner(path.as_ref(), None)
    }

    /// Creates only the final path component when absent, then opens it.
    ///
    /// Parents are intentionally not created recursively: following a symlink
    /// in an unverified missing parent would escape the intended store root.
    ///
    /// # Errors
    ///
    /// Returns an error when the parent is missing, the existing path is
    /// unsafe, or creation, mode enforcement, or parent fsync fails.
    pub fn open_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        validate_directory_path(path)?;
        let mut created = false;
        match create_directory_exact(path) {
            Ok(()) => created = true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StorageError::io("create directory", path, error)),
        }
        // DirBuilder requested the exact mode. Do not chmod by name here: if
        // the path were swapped after mkdir, chmod would follow the replacement.
        // A restrictive unusual umask therefore fails closed below.
        let directory = Self::open_with_owner(path, None)?;
        if created {
            sync_plain_directory(path.parent().ok_or(StorageError::InvalidPath {
                path: path.to_path_buf(),
                reason: "directory must have a parent",
            })?)?;
        }
        Ok(directory)
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    /// The uid that owns this directory, and so every file it may hold.
    pub(crate) const fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    /// Opens a real, private child directory without changing it.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, unsafe child metadata/ownership,
    /// or parent/child identity drift.
    pub fn open_child(&self, name: &str) -> Result<Self> {
        self.assert_unchanged()?;
        let path = self.entry_path(name)?;
        let child = Self::open_with_owner(&path, Some(self.owner_uid))?;
        self.assert_unchanged()?;
        Ok(child)
    }

    /// Creates or opens a private child directory and fsyncs new membership.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, unsafe existing state, ownership
    /// mismatch, or an unproven create/fsync operation.
    pub fn open_or_create_child(&self, name: &str) -> Result<Self> {
        self.assert_unchanged()?;
        let path = self.entry_path(name)?;
        let mut created = false;
        match create_directory_exact(&path) {
            Ok(()) => created = true,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(StorageError::io("create child directory", &path, error)),
        }
        let child = Self::open_with_owner(&path, Some(self.owner_uid))?;
        if created {
            self.sync()?;
        }
        self.assert_unchanged()?;
        Ok(child)
    }

    /// Creates a new private child directory for a staging tree without
    /// fsyncing this directory's new membership.
    ///
    /// The caller owns the child's durability: it must fsync this directory
    /// (or rename the child into place and fsync the destination directory)
    /// before anything depends on the child existing, and remove the child if
    /// it is never published. A tree nothing references yet need not survive
    /// a crash, so the parent fsync [`Self::open_or_create_child`] pays buys
    /// nothing there.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid name, an existing entry, unsafe child
    /// metadata or ownership, or parent/child identity drift.
    pub fn create_unpublished_child(&self, name: &str) -> Result<Self> {
        self.assert_unchanged()?;
        let path = self.entry_path(name)?;
        create_directory_exact(&path)
            .map_err(|error| StorageError::io("create child directory", &path, error))?;
        let child = Self::open_with_owner(&path, Some(self.owner_uid))?;
        self.assert_unchanged()?;
        Ok(child)
    }

    /// Revalidates the named directory against the held descriptor identity.
    ///
    /// # Errors
    ///
    /// Returns an error if the path, inode, device, mode, owner, or file type
    /// is no longer the one accepted at open time.
    pub fn assert_unchanged(&self) -> Result<()> {
        let named = fs::symlink_metadata(&self.path)
            .map_err(|error| StorageError::io("revalidate directory", &self.path, error))?;
        assert_directory_metadata(&self.path, &named, Some(self.owner_uid))?;
        let opened = self
            .handle
            .metadata()
            .map_err(|error| StorageError::io("inspect open directory", &self.path, error))?;
        assert_directory_metadata(&self.path, &opened, Some(self.owner_uid))?;
        if identity(&named) != self.identity || identity(&opened) != self.identity {
            return Err(StorageError::unsafe_at(
                &self.path,
                "directory changed device or inode after it was opened",
            ));
        }
        Ok(())
    }

    /// Fsyncs this directory while checking its identity before and after.
    ///
    /// On Apple platforms std's `sync_all` is `fcntl(F_FULLFSYNC)`: it also
    /// drains the device's write cache, which persists everything a plain
    /// `fsync(2)` handed to the same device before it.
    ///
    /// # Errors
    ///
    /// Returns an error if directory revalidation or fsync fails.
    pub fn sync(&self) -> Result<()> {
        self.assert_unchanged()?;
        self.handle
            .sync_all()
            .map_err(|error| StorageError::io("fsync directory", &self.path, error))?;
        self.assert_unchanged()
    }

    /// Hands this directory's metadata to the device with a plain `fsync(2)`,
    /// checking its identity before and after. On Apple platforms that does
    /// not drain the drive's write cache: only a later [`Self::sync`] (or any
    /// `F_FULLFSYNC` on the device) makes it power-loss durable.
    ///
    /// # Errors
    ///
    /// Returns an error if directory revalidation or fsync fails.
    pub fn hand_off(&self) -> Result<()> {
        self.assert_unchanged()?;
        loop {
            match rustix::fs::fsync(&self.handle) {
                // std's sync_all retries an interrupted call; so does this.
                Err(rustix::io::Errno::INTR) => {}
                result => {
                    result.map_err(|error| {
                        StorageError::io("fsync directory", &self.path, error.into())
                    })?;
                    break;
                }
            }
        }
        self.assert_unchanged()
    }

    pub(crate) fn entry_path(&self, name: &str) -> Result<PathBuf> {
        validate_entry_name(name)?;
        Ok(self.path.join(name))
    }

    pub(crate) fn read_names(&self) -> Result<Vec<String>> {
        self.assert_unchanged()?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.path)
            .map_err(|error| StorageError::io("read directory", &self.path, error))?
        {
            let entry = entry
                .map_err(|error| StorageError::io("read directory entry", &self.path, error))?;
            let name = entry.file_name().into_string().map_err(|_| {
                StorageError::unsafe_at(&self.path, "directory contains a non-UTF-8 entry name")
            })?;
            names.push(name);
        }
        names.sort_unstable();
        self.assert_unchanged()?;
        Ok(names)
    }

    fn open_with_owner(path: &Path, expected_owner: Option<u32>) -> Result<Self> {
        validate_directory_path(path)?;
        let before = fs::symlink_metadata(path)
            .map_err(|error| StorageError::io("inspect directory", path, error))?;
        assert_directory_metadata(path, &before, expected_owner)?;
        let handle = open_directory(path)?;
        let opened = handle
            .metadata()
            .map_err(|error| StorageError::io("inspect open directory", path, error))?;
        assert_directory_metadata(path, &opened, expected_owner)?;
        if identity(&before) != identity(&opened) {
            return Err(StorageError::unsafe_at(
                path,
                "directory changed device or inode while being opened",
            ));
        }
        let directory = Self {
            path: path.to_path_buf(),
            handle,
            identity: identity(&opened),
            owner_uid: opened.uid(),
        };
        directory.assert_unchanged()?;
        Ok(directory)
    }
}

/// The held read-only directory descriptor, for callers that choose their own
/// fsync strength (a plain `fsync(2)` ahead of one [`PrivateDirectory::sync`]
/// barrier). Mutations still go through this type's checked methods.
impl AsFd for PrivateDirectory {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.handle.as_fd()
    }
}

/// Reads at most `max_bytes` from a stable, owner-private regular file.
///
/// A missing path returns `Ok(None)` only after directory and named-path
/// revalidation. The function never creates or repairs filesystem entries.
///
/// # Errors
///
/// Returns an error for invalid bounds/names, links and non-regular files,
/// wrong modes/ownership, hard links, oversize data, or identity drift.
pub fn read_bounded(
    directory: &PrivateDirectory,
    name: &str,
    max_bytes: usize,
) -> Result<Option<BoundedFile>> {
    validate_bound(max_bytes)?;
    directory.assert_unchanged()?;
    let path = directory.entry_path(name)?;
    let Some(mut opened) = open_regular(directory, &path, max_bytes)? else {
        directory.assert_unchanged()?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(StorageError::io("recheck missing file", &path, error)),
            Ok(_) => return Err(StorageError::ExternalDrift { path }),
        }
    };

    let capacity = usize::try_from(opened.metadata.len).unwrap_or(max_bytes);
    let mut bytes = Vec::with_capacity(capacity);
    let read_limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    opened
        .file
        .by_ref()
        .take(read_limit)
        .read_to_end(&mut bytes)
        .map_err(|error| StorageError::io("read bounded file", &path, error))?;
    if bytes.len() > max_bytes {
        return Err(StorageError::BoundsExceeded {
            path,
            max_bytes,
            actual_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        });
    }

    let after = stable_regular_metadata(directory, &path, &opened.file, max_bytes)?;
    if opened.metadata != after || after.len != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
        return Err(StorageError::unsafe_at(
            &path,
            "file changed while it was being read",
        ));
    }
    revalidate_named_file(directory, &path, &after, max_bytes)?;
    directory.assert_unchanged()?;
    let sha256 = Sha256::digest(&bytes).into();
    Ok(Some(BoundedFile {
        bytes,
        snapshot: FileSnapshot {
            identity: after.identity,
            len: after.len,
            sha256,
        },
    }))
}

/// Lowercase hex SHA-256 of `bytes`, the digest form content hashes use on the wire.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Captures either exact absence or a content-and-inode snapshot.
///
/// # Errors
///
/// Returns every safety or bounds error described by [`read_bounded`].
pub fn snapshot_target(
    directory: &PrivateDirectory,
    name: &str,
    max_bytes: usize,
) -> Result<TargetSnapshot> {
    Ok(match read_bounded(directory, name, max_bytes)? {
        Some(file) => TargetSnapshot::File(file.snapshot),
        None => TargetSnapshot::Missing,
    })
}

pub(crate) fn verify_expected_target(
    directory: &PrivateDirectory,
    name: &str,
    expected: &TargetSnapshot,
    max_bytes: usize,
) -> Result<()> {
    let current = snapshot_target(directory, name, max_bytes)?;
    if &current != expected {
        return Err(StorageError::ExternalDrift {
            path: directory.entry_path(name)?,
        });
    }
    Ok(())
}

pub(crate) fn open_regular(
    directory: &PrivateDirectory,
    path: &Path,
    max_bytes: usize,
) -> Result<Option<VerifiedFile>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(StorageError::io("inspect file", path, error)),
    };
    let before = validate_regular_metadata(directory, path, &before, max_bytes)?;
    let mut options = OpenOptions::new();
    options.read(true);
    set_nofollow_flags(&mut options);
    let file = options
        .open(path)
        .map_err(|error| StorageError::io("open file without following links", path, error))?;
    let opened = stable_regular_metadata(directory, path, &file, max_bytes)?;
    if before != opened {
        return Err(StorageError::unsafe_at(
            path,
            "file changed identity or metadata while being opened",
        ));
    }
    revalidate_named_file(directory, path, &opened, max_bytes)?;
    Ok(Some(VerifiedFile {
        file,
        metadata: opened,
    }))
}

pub(crate) fn revalidate_named_file(
    directory: &PrivateDirectory,
    path: &Path,
    expected: &StableFileMetadata,
    max_bytes: usize,
) -> Result<()> {
    let named = fs::symlink_metadata(path)
        .map_err(|error| StorageError::io("revalidate named file", path, error))?;
    let named = validate_regular_metadata(directory, path, &named, max_bytes)?;
    if &named != expected {
        return Err(StorageError::unsafe_at(
            path,
            "named file changed identity or metadata during verification",
        ));
    }
    Ok(())
}

pub(crate) fn stable_regular_metadata(
    directory: &PrivateDirectory,
    path: &Path,
    file: &File,
    max_bytes: usize,
) -> Result<StableFileMetadata> {
    let metadata = file
        .metadata()
        .map_err(|error| StorageError::io("inspect open file", path, error))?;
    validate_regular_metadata(directory, path, &metadata, max_bytes)
}

pub(crate) fn validate_regular_metadata(
    directory: &PrivateDirectory,
    path: &Path,
    metadata: &Metadata,
    max_bytes: usize,
) -> Result<StableFileMetadata> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(StorageError::unsafe_at(
            path,
            "entry must be a regular file and must not be a symlink",
        ));
    }
    if metadata.uid() != directory.owner_uid {
        return Err(StorageError::unsafe_at(
            path,
            "file owner does not match its private directory owner",
        ));
    }
    if permission_mode(metadata) != OWNER_FILE_MODE {
        return Err(StorageError::unsafe_at(
            path,
            "file mode must be exactly 0600",
        ));
    }
    if metadata.nlink() != 1 {
        return Err(StorageError::unsafe_at(
            path,
            "hard-linked files are not authoritative storage",
        ));
    }
    if metadata.len() > u64::try_from(max_bytes).unwrap_or(u64::MAX) {
        return Err(StorageError::BoundsExceeded {
            path: path.to_path_buf(),
            max_bytes,
            actual_bytes: metadata.len(),
        });
    }
    Ok(stable_metadata(metadata))
}

pub(crate) fn validate_bound(max_bytes: usize) -> Result<()> {
    if max_bytes == 0 || max_bytes > HARD_MAX_FILE_BYTES {
        return Err(StorageError::InvalidPath {
            path: PathBuf::from("<byte-limit>"),
            reason: "byte limit must be between 1 byte and HARD_MAX_FILE_BYTES",
        });
    }
    Ok(())
}

pub(crate) fn validate_entry_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(StorageError::InvalidName {
            name: name.to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn set_private_file_mode(file: &File, path: &Path) -> Result<()> {
    file.set_permissions(fs::Permissions::from_mode(OWNER_FILE_MODE))
        .map_err(|error| StorageError::io("set private file mode on", path, error))
}

pub(crate) fn create_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).read(true).create_new(true);
    set_create_mode_and_flags(&mut options, OWNER_FILE_MODE);
    options
        .open(path)
        .map_err(|error| StorageError::io("create private file", path, error))
}

fn validate_directory_path(path: &Path) -> Result<()> {
    if !path.is_absolute() || path.parent().is_none() || path == Path::new("/") {
        return Err(StorageError::InvalidPath {
            path: path.to_path_buf(),
            reason: "directory must be an absolute non-root path",
        });
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(StorageError::InvalidPath {
            path: path.to_path_buf(),
            reason: "directory must not contain parent traversal",
        });
    }
    Ok(())
}

fn assert_directory_metadata(
    path: &Path,
    metadata: &Metadata,
    expected_owner: Option<u32>,
) -> Result<()> {
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(StorageError::unsafe_at(
            path,
            "directory must be real and must not be a symlink",
        ));
    }
    if permission_mode(metadata) != OWNER_DIRECTORY_MODE {
        return Err(StorageError::unsafe_at(
            path,
            "directory mode must be exactly 0700",
        ));
    }
    if expected_owner.is_some_and(|owner| metadata.uid() != owner) {
        return Err(StorageError::unsafe_at(
            path,
            "child directory owner does not match its private parent owner",
        ));
    }
    Ok(())
}

fn stable_metadata(metadata: &Metadata) -> StableFileMetadata {
    StableFileMetadata {
        identity: identity(metadata),
        len: metadata.len(),
        mode: metadata.mode(),
        uid: metadata.uid(),
        links: metadata.nlink(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn sync_plain_directory(path: &Path) -> Result<()> {
    let file = open_directory(path)?;
    file.sync_all()
        .map_err(|error| StorageError::io("fsync parent directory", path, error))
}

fn create_directory_exact(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.mode(OWNER_DIRECTORY_MODE).create(path)
}

fn open_directory(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options
        .open(path)
        .map_err(|error| StorageError::io("open directory without following links", path, error))
}

fn set_nofollow_flags(options: &mut OpenOptions) {
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
}

fn set_create_mode_and_flags(options: &mut OpenOptions, mode: u32) {
    options
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
}

fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn permission_mode(metadata: &Metadata) -> u32 {
    metadata.mode() & 0o7777
}

#[cfg(not(unix))]
compile_error!("interface-storage currently requires Unix O_NOFOLLOW and inode semantics");
