use std::collections::BTreeSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::Duration;

use interface_common::bounded_read;

use interface_contracts::ipc::FileSnapshot;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

use crate::fsid::{FileIdentity, identity, inside, relative_string};
use crate::{is_likely_binary_path, is_search_excluded_path};

use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub const EDITOR_FILE_BYTE_LIMIT: usize = 5 * 1024 * 1024;
pub const EDITOR_DIRECTORY_ENTRY_LIMIT: usize = 5_000;

/// A safe directory-listing row returned to the native UI.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryEntry {
    pub name: String,
    pub dir: bool,
}

/// Deterministic scheduler-gap hooks used only by adversarial tests.
pub trait EditorHooks: Send + Sync {
    fn after_snapshot_validated(&self, _file: &Path) {}
    fn before_save_temp_create(&self, _file: &Path) {}
    fn before_save_rename(&self, _file: &Path) {}
}

/// Human-safe editor failures. No variant contains a raw path, errno line, or
/// operating-system stack text.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum WorkspaceEditorError {
    #[error("that path is outside every currently authorized workspace")]
    OutsideAuthorizedRoot,
    #[error("that path or one of its folders changed while Interface was using it")]
    PathChanged,
    #[error("credential-shaped files and folders cannot be opened")]
    ProtectedPath,
    #[error("binary and container files cannot be opened in the text editor")]
    BinaryFile,
    #[error("that file is larger than the 5 MB editor limit")]
    FileTooLarge,
    #[error("that folder contains more than 5000 entries")]
    DirectoryTooLarge,
    #[error("that target is a folder, not a text file")]
    IsDirectory,
    #[error("that target is not valid UTF-8 text")]
    InvalidText,
    #[error("the save request is invalid")]
    InvalidSave,
    #[error("the file changed on disk; nothing was overwritten")]
    StaleRevision,
    #[error("the file is not writable")]
    NotWritable,
    #[error("the save failed before replacing the file")]
    WriteFailed,
    #[error("the save may have committed and must be reconciled before retrying")]
    WriteUncertain,
    /// The read did not finish within [`EDITOR_READ_DEADLINE`]: the file is
    /// most likely still downloading from iCloud.
    #[error("that file is still downloading from iCloud; try again once it has downloaded")]
    StillDownloading,
}

/// How long one editor or output-preview read may take. A local file within
/// the 5 MB limit reads in milliseconds; one iCloud has evicted waits for its
/// download, so the read stops here and says so.
pub const EDITOR_READ_DEADLINE: Duration = Duration::from_secs(10);

/// Mutable authorization set for file-tree/editor operations. Session and
/// picked-folder lifecycle code grants and revokes roots; every operation then
/// independently re-proves canonical containment and inode identity.
#[derive(Default)]
pub struct WorkspaceAuthority {
    roots: RwLock<BTreeSet<PathBuf>>,
    files: RwLock<BTreeSet<PathBuf>>,
}

impl WorkspaceAuthority {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            roots: RwLock::new(BTreeSet::new()),
            files: RwLock::new(BTreeSet::new()),
        }
    }

    /// Adds one exact canonical, real directory as an authorized root.
    ///
    /// # Errors
    ///
    /// Refuses relative, missing, non-directory, symlinked, or concurrently
    /// replaced roots.
    pub fn grant_root(&self, root: &Path) -> Result<PathBuf, WorkspaceEditorError> {
        let stable = stable_directory(root)?;
        self.roots
            .write()
            .map_err(|_| WorkspaceEditorError::PathChanged)?
            .insert(stable.path.clone());
        Ok(stable.path)
    }

    /// Adds one exact canonical regular file for read-only preview inspection
    /// without authorizing directory listing or sibling files in its parent.
    ///
    /// # Errors
    ///
    /// Refuses relative, missing, non-regular, symlinked, or concurrently
    /// replaced files.
    pub fn grant_file(&self, file: &Path) -> Result<PathBuf, WorkspaceEditorError> {
        if !file.is_absolute() {
            return Err(WorkspaceEditorError::OutsideAuthorizedRoot);
        }
        let canonical =
            fs::canonicalize(file).map_err(|_| WorkspaceEditorError::OutsideAuthorizedRoot)?;
        if canonical != file {
            return Err(WorkspaceEditorError::PathChanged);
        }
        let _ = stable_regular_file(&canonical)?;
        let parent = canonical
            .parent()
            .ok_or(WorkspaceEditorError::OutsideAuthorizedRoot)?;
        let _ = stable_directory(parent)?;
        self.files
            .write()
            .map_err(|_| WorkspaceEditorError::PathChanged)?
            .insert(canonical.clone());
        Ok(canonical)
    }

    /// Removes an exact lexical/canonical root grant.
    pub fn revoke_root(&self, root: &Path) {
        if let Ok(mut roots) = self.roots.write() {
            roots.remove(root);
            if let Ok(canonical) = fs::canonicalize(root) {
                roots.remove(&canonical);
            }
        }
    }

    pub fn clear(&self) {
        if let Ok(mut roots) = self.roots.write() {
            roots.clear();
        }
        if let Ok(mut files) = self.files.write() {
            files.clear();
        }
    }

    #[must_use]
    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots
            .read()
            .map_or_else(|_| Vec::new(), |roots| roots.iter().cloned().collect())
    }

    /// Reads one authorized text file.
    ///
    /// # Errors
    ///
    /// Returns a sanitized authorization, safety, size, or UTF-8 error.
    pub fn read_text(&self, target: &Path) -> Result<String, WorkspaceEditorError> {
        self.read_text_with_hooks(target, None)
    }

    /// Same as [`Self::read_text`] with a deterministic adversarial test hook.
    ///
    /// # Errors
    ///
    /// Returns every error documented by [`Self::read_text`].
    pub fn read_text_with_hooks(
        &self,
        target: &Path,
        hooks: Option<&dyn EditorHooks>,
    ) -> Result<String, WorkspaceEditorError> {
        let verified = self.verified_file(target)?;
        Ok(read_verified_regular(&verified, hooks)?.text)
    }

    /// Reads text plus its lowercase SHA-256 compare-and-save revision.
    ///
    /// # Errors
    ///
    /// Returns a sanitized authorization, safety, size, or UTF-8 error.
    pub fn read_snapshot(&self, target: &Path) -> Result<FileSnapshot, WorkspaceEditorError> {
        let text = self.read_text(target)?;
        Ok(snapshot(text))
    }

    /// Atomically compare-and-saves one authorized text file.
    ///
    /// # Errors
    ///
    /// Refuses oversized drafts, stale revisions, unsafe paths, unwritable
    /// sources, unproven temp ownership, and uncertain durability.
    pub fn save_text(
        &self,
        target: &Path,
        text: &str,
        expected_revision: &str,
    ) -> Result<FileSnapshot, WorkspaceEditorError> {
        self.save_text_with_hooks(target, text, expected_revision, None)
    }

    /// Same as [`Self::save_text`] with deterministic scheduler-gap hooks.
    ///
    /// # Errors
    ///
    /// Returns every error documented by [`Self::save_text`].
    pub fn save_text_with_hooks(
        &self,
        target: &Path,
        text: &str,
        expected_revision: &str,
        hooks: Option<&dyn EditorHooks>,
    ) -> Result<FileSnapshot, WorkspaceEditorError> {
        if text.len() > EDITOR_FILE_BYTE_LIMIT || !is_revision(expected_revision) {
            return Err(WorkspaceEditorError::InvalidSave);
        }
        let verified = self.verified_file(target)?;
        compare_and_replace(&verified, text, expected_revision, hooks)?;
        Ok(snapshot(text.to_owned()))
    }

    /// Lists a single authorized directory with directories first and the
    /// shared credential/binary/size policy applied.
    ///
    /// # Errors
    ///
    /// Refuses unsafe/unauthorized roots, changing directories, and oversized
    /// listings rather than returning a misleading partial tree.
    pub fn list_directory(
        &self,
        target: &Path,
    ) -> Result<Vec<DirectoryEntry>, WorkspaceEditorError> {
        self.list_directory_policy(target, false)
    }

    pub(super) fn list_preview_directory(
        &self,
        target: &Path,
    ) -> Result<Vec<DirectoryEntry>, WorkspaceEditorError> {
        self.list_directory_policy(target, true)
    }

    fn list_directory_policy(
        &self,
        target: &Path,
        preview: bool,
    ) -> Result<Vec<DirectoryEntry>, WorkspaceEditorError> {
        let (directory, owner) = self.resolve_authorized_path(target)?;
        let before = stable_directory(&directory)?;
        if !inside(&owner.path, &directory) {
            return Err(WorkspaceEditorError::OutsideAuthorizedRoot);
        }
        // A folder iCloud has evicted lists only once it is downloaded, so
        // the listing runs through the bounded project read too.
        let listed = directory.clone();
        let entries = bounded_read::within(EDITOR_READ_DEADLINE, move || {
            fs::read_dir(&listed)
                .map_err(|_| WorkspaceEditorError::PathChanged)?
                .take(EDITOR_DIRECTORY_ENTRY_LIMIT + 1)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| WorkspaceEditorError::PathChanged)
        })
        .map_err(|_| WorkspaceEditorError::StillDownloading)??;
        if entries.len() > EDITOR_DIRECTORY_ENTRY_LIMIT {
            return Err(WorkspaceEditorError::DirectoryTooLarge);
        }
        assert_directory(&before)?;

        let mut visible = Vec::new();
        for entry in entries {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let absolute = directory.join(&name);
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let Some(relative) = relative_string(&owner.path, &absolute) else {
                continue;
            };
            // Full classifier: this is a single-directory listing, so ancestor
            // segments have not already been admitted by a walk enqueue.
            if file_type.is_symlink() || is_search_excluded_path(&relative, file_type.is_dir()) {
                continue;
            }
            if !file_type.is_dir()
                && (!file_type.is_file() || (!preview && is_likely_binary_path(&relative)))
            {
                continue;
            }
            // A listing reads no content and the directory is pinned before
            // and after, so one `lstat` names an entry: a single component of
            // a canonical directory that is not a link is canonical itself.
            // Opening or saving an entry proves the whole path again.
            let Ok(metadata) = fs::symlink_metadata(&absolute) else {
                continue;
            };
            let kind = metadata.file_type();
            if kind.is_symlink() || kind.is_dir() != file_type.is_dir() {
                continue;
            }
            if kind.is_dir() {
                visible.push(DirectoryEntry { name, dir: true });
            } else if kind.is_file()
                && (preview
                    || metadata.len() <= u64::try_from(EDITOR_FILE_BYTE_LIMIT).unwrap_or(u64::MAX))
            {
                visible.push(DirectoryEntry { name, dir: false });
            }
        }
        assert_directory(&before)?;
        visible.sort_by(|left, right| right.dir.cmp(&left.dir).then(left.name.cmp(&right.name)));
        Ok(visible)
    }

    fn verified_file(&self, target: &Path) -> Result<VerifiedFile, WorkspaceEditorError> {
        self.verified_file_kind(target, false)
    }

    pub(super) fn preview_file_size(&self, target: &Path) -> Result<u64, WorkspaceEditorError> {
        let verified = self.verified_file_kind(target, true)?;
        let metadata =
            fs::symlink_metadata(&verified.file).map_err(|_| WorkspaceEditorError::PathChanged)?;
        assert_verified_file_path(&verified)?;
        Ok(metadata.len())
    }

    pub(super) fn preview_file_bytes(
        &self,
        target: &Path,
        hooks: Option<&dyn EditorHooks>,
    ) -> Result<Vec<u8>, WorkspaceEditorError> {
        let verified = self.verified_file_kind(target, true)?;
        Ok(read_verified_bytes(&verified, hooks)?.0)
    }

    fn verified_file_kind(
        &self,
        target: &Path,
        preview: bool,
    ) -> Result<VerifiedFile, WorkspaceEditorError> {
        let (file, root) = self.resolve_authorized_target(target, preview)?;
        let relative = relative_string(&root.path, &file)
            .ok_or(WorkspaceEditorError::OutsideAuthorizedRoot)?;
        if is_search_excluded_path(&relative, false) {
            return Err(WorkspaceEditorError::ProtectedPath);
        }
        if !preview && is_likely_binary_path(&relative) {
            return Err(WorkspaceEditorError::BinaryFile);
        }
        let identity = stable_regular_file(&file)?;
        let parent_path = file.parent().ok_or(WorkspaceEditorError::PathChanged)?;
        let parent = stable_directory(parent_path)?;
        if !inside(&root.path, &parent.path) {
            return Err(WorkspaceEditorError::PathChanged);
        }
        let verified = VerifiedFile {
            file,
            identity,
            parent,
            root,
        };
        assert_verified_file_path(&verified)?;
        Ok(verified)
    }

    fn resolve_authorized_path(
        &self,
        target: &Path,
    ) -> Result<(PathBuf, VerifiedDirectory), WorkspaceEditorError> {
        self.resolve_authorized_target(target, false)
    }

    fn resolve_authorized_target(
        &self,
        target: &Path,
        allow_exact_file: bool,
    ) -> Result<(PathBuf, VerifiedDirectory), WorkspaceEditorError> {
        if !target.is_absolute() {
            return Err(WorkspaceEditorError::OutsideAuthorizedRoot);
        }
        let canonical =
            fs::canonicalize(target).map_err(|_| WorkspaceEditorError::OutsideAuthorizedRoot)?;
        if canonical != target {
            return Err(WorkspaceEditorError::PathChanged);
        }
        let roots = self
            .roots
            .read()
            .map_err(|_| WorkspaceEditorError::PathChanged)?;
        // Granted roots are stored canonical (grant_root), so a lexical
        // prefix test picks the candidates before any per-root syscall.
        if let Some(root) = roots
            .iter()
            .filter(|root| inside(root, &canonical))
            .filter_map(|root| stable_directory(root).ok())
            .max_by_key(|root| root.path.components().count())
        {
            return Ok((canonical, root));
        }
        drop(roots);
        if allow_exact_file {
            let files = self
                .files
                .read()
                .map_err(|_| WorkspaceEditorError::PathChanged)?;
            if files.contains(&canonical) {
                let parent = canonical
                    .parent()
                    .ok_or(WorkspaceEditorError::OutsideAuthorizedRoot)?;
                let root = stable_directory(parent)?;
                return Ok((canonical, root));
            }
        }
        Err(WorkspaceEditorError::OutsideAuthorizedRoot)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableFileState {
    identity: FileIdentity,
    len: u64,
    mode: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

/// A real directory at its own canonical path, pinned by identity.
type VerifiedDirectory = interface_common::fsid::DirectorySnapshot;

#[derive(Clone, Debug)]
struct VerifiedFile {
    file: PathBuf,
    identity: FileIdentity,
    parent: VerifiedDirectory,
    root: VerifiedDirectory,
}

struct OpenedText {
    text: String,
    state: StableFileState,
}

fn stable_state(metadata: &Metadata) -> StableFileState {
    StableFileState {
        identity: identity(metadata),
        len: metadata.len(),
        mode: metadata.mode(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

/// Pins `directory`, which must already be absolute and canonical: a
/// relative path is never its own canonical path, so it is refused too.
fn stable_directory(directory: &Path) -> Result<VerifiedDirectory, WorkspaceEditorError> {
    VerifiedDirectory::capture(directory).map_err(|_| WorkspaceEditorError::PathChanged)
}

fn assert_directory(directory: &VerifiedDirectory) -> Result<(), WorkspaceEditorError> {
    directory
        .assert_unchanged()
        .map_err(|_| WorkspaceEditorError::PathChanged)
}

fn stable_regular_file(file: &Path) -> Result<FileIdentity, WorkspaceEditorError> {
    let before = fs::symlink_metadata(file).map_err(|_| WorkspaceEditorError::PathChanged)?;
    if before.file_type().is_dir() {
        return Err(WorkspaceEditorError::IsDirectory);
    }
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(WorkspaceEditorError::PathChanged);
    }
    let canonical = fs::canonicalize(file).map_err(|_| WorkspaceEditorError::PathChanged)?;
    let after = fs::symlink_metadata(file).map_err(|_| WorkspaceEditorError::PathChanged)?;
    if canonical != file
        || !after.file_type().is_file()
        || after.file_type().is_symlink()
        || identity(&before) != identity(&after)
    {
        return Err(WorkspaceEditorError::PathChanged);
    }
    Ok(identity(&after))
}

fn assert_verified_file_path(verified: &VerifiedFile) -> Result<(), WorkspaceEditorError> {
    assert_directory(&verified.root)?;
    assert_directory(&verified.parent)?;
    if stable_regular_file(&verified.file)? != verified.identity {
        return Err(WorkspaceEditorError::PathChanged);
    }
    assert_directory(&verified.parent)?;
    assert_directory(&verified.root)
}

fn read_verified_regular(
    verified: &VerifiedFile,
    hooks: Option<&dyn EditorHooks>,
) -> Result<OpenedText, WorkspaceEditorError> {
    let (bytes, state) = read_verified_bytes(verified, hooks)?;
    if bytes.contains(&0) {
        return Err(WorkspaceEditorError::InvalidText);
    }
    let text = String::from_utf8(bytes).map_err(|_| WorkspaceEditorError::InvalidText)?;
    Ok(OpenedText { text, state })
}

fn read_verified_bytes(
    verified: &VerifiedFile,
    hooks: Option<&dyn EditorHooks>,
) -> Result<(Vec<u8>, StableFileState), WorkspaceEditorError> {
    read_verified_bytes_with(verified, hooks, open_for_read, EDITOR_READ_DEADLINE)
}

/// A no-follow, non-blocking, close-on-exec open for reading.
fn open_for_read(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    options.open(path)
}

/// The open and the read are the only steps that touch the file's contents,
/// so they are the ones that wait for a file iCloud has evicted: they run
/// through the bounded project read. Every identity check still runs on what
/// they answer, and a mismatch discards the bytes.
fn read_verified_bytes_with(
    verified: &VerifiedFile,
    hooks: Option<&dyn EditorHooks>,
    open: fn(&Path) -> std::io::Result<File>,
    deadline: Duration,
) -> Result<(Vec<u8>, StableFileState), WorkspaceEditorError> {
    assert_verified_file_path(verified)?;
    if let Some(hooks) = hooks {
        hooks.after_snapshot_validated(&verified.file);
    }
    let path = verified.file.clone();
    let (file, opened, bytes) = bounded_read::within(deadline, move || {
        let mut file = open(&path).map_err(|_| WorkspaceEditorError::PathChanged)?;
        let opened = file
            .metadata()
            .map_err(|_| WorkspaceEditorError::PathChanged)?;
        let mut bytes = Vec::with_capacity(
            usize::try_from(opened.len())
                .unwrap_or(0)
                .min(EDITOR_FILE_BYTE_LIMIT),
        );
        Read::by_ref(&mut file)
            .take(
                u64::try_from(EDITOR_FILE_BYTE_LIMIT)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)
            .map_err(|_| WorkspaceEditorError::PathChanged)?;
        Ok::<_, WorkspaceEditorError>((file, opened, bytes))
    })
    .map_err(|_| WorkspaceEditorError::StillDownloading)??;
    if !opened.file_type().is_file() || identity(&opened) != verified.identity {
        return Err(WorkspaceEditorError::PathChanged);
    }
    assert_verified_file_path(verified)?;
    if opened.len() > u64::try_from(EDITOR_FILE_BYTE_LIMIT).unwrap_or(u64::MAX) {
        return Err(WorkspaceEditorError::FileTooLarge);
    }
    if bytes.len() > EDITOR_FILE_BYTE_LIMIT {
        return Err(WorkspaceEditorError::FileTooLarge);
    }
    let after = file
        .metadata()
        .map_err(|_| WorkspaceEditorError::PathChanged)?;
    if stable_state(&opened) != stable_state(&after) {
        return Err(WorkspaceEditorError::PathChanged);
    }
    assert_verified_file_path(verified)?;
    Ok((bytes, stable_state(&opened)))
}

fn compare_and_replace(
    verified: &VerifiedFile,
    text: &str,
    expected_revision: &str,
    hooks: Option<&dyn EditorHooks>,
) -> Result<(), WorkspaceEditorError> {
    assert_writable(verified)?;
    let before = read_verified_regular(verified, None)?;
    if sha256_hex(&before.text) != expected_revision {
        return Err(WorkspaceEditorError::StaleRevision);
    }

    let file_name = verified
        .file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(WorkspaceEditorError::InvalidSave)?;
    let temporary = verified
        .parent
        .path
        .join(format!(".{file_name}.interface-{}.tmp", Uuid::new_v4()));
    let mut temporary_identity = None;
    let mut renamed = false;
    let operation = (|| -> Result<(), WorkspaceEditorError> {
        assert_verified_file_path(verified)?;
        if let Some(hooks) = hooks {
            hooks.before_save_temp_create(&verified.file);
        }
        assert_expected_target(verified, expected_revision, before.state.identity)?;

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        options
            .mode(before.state.mode & 0o777)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut staged = options
            .open(&temporary)
            .map_err(|_| WorkspaceEditorError::WriteFailed)?;
        let staged_metadata = staged
            .metadata()
            .map_err(|_| WorkspaceEditorError::WriteFailed)?;
        if !staged_metadata.file_type().is_file() {
            return Err(WorkspaceEditorError::WriteFailed);
        }
        let staged_identity = identity(&staged_metadata);
        temporary_identity = Some(staged_identity);
        assert_owned_temp(verified, &temporary, staged_identity)?;
        staged
            .write_all(text.as_bytes())
            .map_err(|_| WorkspaceEditorError::WriteFailed)?;
        staged
            .set_permissions(fs::Permissions::from_mode(before.state.mode & 0o777))
            .map_err(|_| WorkspaceEditorError::WriteFailed)?;
        staged
            .sync_all()
            .map_err(|_| WorkspaceEditorError::WriteFailed)?;
        assert_expected_target(verified, expected_revision, before.state.identity)?;
        assert_owned_temp(verified, &temporary, staged_identity)?;
        drop(staged);

        if let Some(hooks) = hooks {
            hooks.before_save_rename(&verified.file);
        }
        assert_expected_target(verified, expected_revision, before.state.identity)?;
        assert_owned_temp(verified, &temporary, staged_identity)?;
        fs::rename(&temporary, &verified.file).map_err(|_| WorkspaceEditorError::WriteFailed)?;
        renamed = true;
        sync_directory(&verified.parent.path).map_err(|_| WorkspaceEditorError::WriteUncertain)?;
        let installed =
            reverify_installed(verified, text).map_err(|_| WorkspaceEditorError::WriteUncertain)?;
        if installed != sha256_hex(text) {
            return Err(WorkspaceEditorError::WriteUncertain);
        }
        Ok(())
    })();

    if !renamed {
        remove_owned_temp(verified, &temporary, temporary_identity);
    }
    match operation {
        Err(WorkspaceEditorError::WriteFailed) if renamed => {
            Err(WorkspaceEditorError::WriteUncertain)
        }
        result => result,
    }
}

fn assert_writable(verified: &VerifiedFile) -> Result<(), WorkspaceEditorError> {
    assert_verified_file_path(verified)?;
    let mut options = OpenOptions::new();
    options.write(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let file = options
        .open(&verified.file)
        .map_err(|_| WorkspaceEditorError::NotWritable)?;
    if identity(
        &file
            .metadata()
            .map_err(|_| WorkspaceEditorError::PathChanged)?,
    ) != verified.identity
    {
        return Err(WorkspaceEditorError::PathChanged);
    }
    assert_verified_file_path(verified)
}

fn assert_expected_target(
    verified: &VerifiedFile,
    expected_revision: &str,
    expected_identity: FileIdentity,
) -> Result<(), WorkspaceEditorError> {
    let current = read_verified_regular(verified, None)?;
    if current.state.identity != expected_identity || sha256_hex(&current.text) != expected_revision
    {
        return Err(WorkspaceEditorError::StaleRevision);
    }
    Ok(())
}

fn assert_owned_temp(
    verified: &VerifiedFile,
    temporary: &Path,
    expected_identity: FileIdentity,
) -> Result<(), WorkspaceEditorError> {
    assert_directory(&verified.root)?;
    assert_directory(&verified.parent)?;
    if stable_regular_file(temporary)? != expected_identity {
        return Err(WorkspaceEditorError::PathChanged);
    }
    assert_directory(&verified.parent)?;
    assert_directory(&verified.root)
}

fn remove_owned_temp(
    verified: &VerifiedFile,
    temporary: &Path,
    expected_identity: Option<FileIdentity>,
) {
    let Some(identity) = expected_identity else {
        return;
    };
    if assert_owned_temp(verified, temporary, identity).is_ok() {
        let _ = fs::remove_file(temporary);
    }
}

fn reverify_installed(
    previous: &VerifiedFile,
    expected_text: &str,
) -> Result<String, WorkspaceEditorError> {
    assert_directory(&previous.root)?;
    assert_directory(&previous.parent)?;
    let identity = stable_regular_file(&previous.file)?;
    let installed = VerifiedFile {
        file: previous.file.clone(),
        identity,
        parent: previous.parent.clone(),
        root: previous.root.clone(),
    };
    let opened = read_verified_regular(&installed, None)?;
    if opened.text != expected_text {
        return Err(WorkspaceEditorError::WriteUncertain);
    }
    Ok(sha256_hex(&opened.text))
}

fn sync_directory(directory: &Path) -> Result<(), WorkspaceEditorError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let handle = options
        .open(directory)
        .map_err(|_| WorkspaceEditorError::WriteUncertain)?;
    handle
        .sync_all()
        .map_err(|_| WorkspaceEditorError::WriteUncertain)
}

fn snapshot(text: String) -> FileSnapshot {
    let revision = sha256_hex(&text);
    FileSnapshot { text, revision }
}

/// Lowercase hex SHA-256 of `text`: the editor revision and the delegation
/// packet digest.
pub(crate) fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn is_revision(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    use std::os::unix::fs::symlink;

    struct TestProject {
        _temporary: tempfile::TempDir,
        path: PathBuf,
    }

    impl TestProject {
        fn new() -> Self {
            let temporary = tempfile::tempdir().expect("project");
            let path = fs::canonicalize(temporary.path()).expect("canonical project");
            Self {
                _temporary: temporary,
                path,
            }
        }
    }

    fn authority(root: &Path) -> WorkspaceAuthority {
        let authority = WorkspaceAuthority::new();
        authority.grant_root(root).expect("grant root");
        authority
    }

    #[test]
    fn reads_lists_and_filters_one_authorized_root() {
        let project = TestProject::new();
        fs::create_dir(project.path.join("zdir")).expect("dir");
        fs::write(project.path.join("a.txt"), "safe").expect("text");
        fs::write(project.path.join(".env.local"), "secret").expect("secret");
        fs::write(project.path.join("image.png"), "not image").expect("binary");
        let authority = authority(&project.path);

        assert_eq!(
            authority.list_directory(&project.path).expect("directory"),
            [
                DirectoryEntry {
                    name: "zdir".into(),
                    dir: true
                },
                DirectoryEntry {
                    name: "a.txt".into(),
                    dir: false
                }
            ]
        );
        assert_eq!(
            authority.read_text(&project.path.join("a.txt")),
            Ok("safe".into())
        );
        assert_eq!(
            authority.read_text(&project.path.join(".env.local")),
            Err(WorkspaceEditorError::ProtectedPath)
        );
        assert_eq!(
            authority.read_text(&project.path.join("image.png")),
            Err(WorkspaceEditorError::BinaryFile)
        );
    }

    #[test]
    fn list_directory_names_entries_without_following_links() {
        let project = TestProject::new();
        fs::create_dir(project.path.join("dir")).expect("dir");
        fs::write(project.path.join("file.txt"), "text").expect("file");
        fs::write(
            project.path.join("large.txt"),
            vec![b'x'; EDITOR_FILE_BYTE_LIMIT + 1],
        )
        .expect("large");
        fs::write(project.path.join("locked.txt"), "locked").expect("locked");
        fs::set_permissions(
            project.path.join("locked.txt"),
            std::os::unix::fs::PermissionsExt::from_mode(0o000),
        )
        .expect("lock");
        symlink(
            project.path.join("file.txt"),
            project.path.join("file-link.txt"),
        )
        .expect("link");
        symlink(project.path.join("dir"), project.path.join("dir-link")).expect("dir link");
        let authority = authority(&project.path);
        let names = |entries: Vec<DirectoryEntry>| {
            entries
                .into_iter()
                .map(|entry| (entry.name, entry.dir))
                .collect::<Vec<_>>()
        };

        // Links are never listed; an unreadable file is, as before (opening
        // it is what refuses it); only the editor refuses a large file.
        assert_eq!(
            names(authority.list_directory(&project.path).expect("listing")),
            [
                ("dir".to_owned(), true),
                ("file.txt".to_owned(), false),
                ("locked.txt".to_owned(), false),
            ]
        );
        assert_eq!(
            names(
                authority
                    .list_preview_directory(&project.path)
                    .expect("preview listing")
            ),
            [
                ("dir".to_owned(), true),
                ("file.txt".to_owned(), false),
                ("large.txt".to_owned(), false),
                ("locked.txt".to_owned(), false),
            ]
        );
    }

    #[test]
    fn list_directory_hides_children_of_sensitive_ancestors() {
        let project = TestProject::new();
        fs::create_dir(project.path.join(".ssh")).expect("ssh");
        fs::write(project.path.join(".ssh/config"), "secret").expect("config");
        fs::create_dir(project.path.join("src")).expect("src");
        fs::write(project.path.join("src/app.ts"), "ok").expect("app");
        let authority = authority(&project.path);
        assert!(
            authority
                .list_directory(&project.path.join(".ssh"))
                .expect("ssh listing")
                .is_empty()
        );
        assert_eq!(
            authority
                .list_directory(&project.path.join("src"))
                .expect("src listing"),
            [DirectoryEntry {
                name: "app.ts".into(),
                dir: false
            }]
        );
    }

    #[test]
    fn deepest_authorized_root_is_selected() {
        let project = TestProject::new();
        let nested = project.path.join("inner");
        fs::create_dir(&nested).expect("inner");
        fs::write(nested.join("note.txt"), "inner").expect("note");
        fs::write(project.path.join("outer.txt"), "outer").expect("outer");
        let authority = WorkspaceAuthority::new();
        authority.grant_root(&project.path).expect("grant outer");
        authority.grant_root(&nested).expect("grant inner");
        assert_eq!(
            authority.read_text(&nested.join("note.txt")),
            Ok("inner".into())
        );
        assert_eq!(
            authority.list_directory(&nested).expect("nested listing"),
            [DirectoryEntry {
                name: "note.txt".into(),
                dir: false
            }]
        );
        assert_eq!(
            authority.read_text(&project.path.join("outer.txt")),
            Ok("outer".into())
        );
    }

    #[test]
    fn compare_and_save_never_overwrites_a_stale_revision() {
        let project = TestProject::new();
        let file = project.path.join("note.txt");
        fs::write(&file, "one").expect("one");
        let authority = authority(&project.path);
        let first = authority.read_snapshot(&file).expect("snapshot");
        let saved = authority
            .save_text(&file, "two", &first.revision)
            .expect("save");
        assert_eq!(fs::read_to_string(&file).expect("saved bytes"), "two");
        assert_ne!(saved.revision, first.revision);

        fs::write(&file, "changed elsewhere").expect("external edit");
        assert_eq!(
            authority.save_text(&file, "stale draft", &saved.revision),
            Err(WorkspaceEditorError::StaleRevision)
        );
        assert_eq!(
            fs::read_to_string(&file).expect("external bytes"),
            "changed elsewhere"
        );
    }

    #[test]
    fn refuses_invalid_utf8_nul_and_oversized_text() {
        let project = TestProject::new();
        let invalid = project.path.join("invalid.txt");
        let nul = project.path.join("nul.txt");
        let large = project.path.join("large.txt");
        fs::write(&invalid, [0xff, 0xfe]).expect("invalid");
        fs::write(&nul, b"hello\0world").expect("nul");
        fs::write(&large, vec![b'x'; EDITOR_FILE_BYTE_LIMIT + 1]).expect("large");
        let authority = authority(&project.path);
        assert_eq!(
            authority.read_text(&invalid),
            Err(WorkspaceEditorError::InvalidText)
        );
        assert_eq!(
            authority.read_text(&nul),
            Err(WorkspaceEditorError::InvalidText)
        );
        assert_eq!(
            authority.read_text(&large),
            Err(WorkspaceEditorError::FileTooLarge)
        );
    }

    #[test]
    fn ancestor_swap_is_refused_before_reading_or_saving_outside() {
        let project = TestProject::new();
        let outside = TestProject::new();
        let source = project.path.join("src");
        let parked = project.path.join("src-parked");
        fs::create_dir(&source).expect("source");
        let file = source.join("note.txt");
        fs::write(&file, "inside").expect("inside");
        fs::write(outside.path.join("note.txt"), "outside").expect("outside");
        let authority = authority(&project.path);
        let hook = SwapHook {
            source: source.clone(),
            parked,
            outside: outside.path.clone(),
            target: file.clone(),
            swapped: AtomicBool::new(false),
        };
        assert_eq!(
            authority.read_text_with_hooks(&file, Some(&hook)),
            Err(WorkspaceEditorError::PathChanged)
        );
        assert!(hook.swapped.load(Ordering::SeqCst));
        assert_eq!(
            fs::read_to_string(outside.path.join("note.txt")).expect("outside unchanged"),
            "outside"
        );
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum SaveGap {
        BeforeTempCreate,
        BeforeRename,
    }

    /// Runs `action` once, in the chosen scheduler gap of the save path.
    struct SaveRaceHook<F> {
        gap: SaveGap,
        action: F,
        fired: AtomicBool,
    }

    impl<F: Fn() + Send + Sync> SaveRaceHook<F> {
        fn new(gap: SaveGap, action: F) -> Self {
            Self {
                gap,
                action,
                fired: AtomicBool::new(false),
            }
        }

        fn fire(&self, gap: SaveGap) {
            if gap == self.gap && !self.fired.swap(true, Ordering::SeqCst) {
                (self.action)();
            }
        }
    }

    impl<F: Fn() + Send + Sync> EditorHooks for SaveRaceHook<F> {
        fn before_save_temp_create(&self, _file: &Path) {
            self.fire(SaveGap::BeforeTempCreate);
        }

        fn before_save_rename(&self, _file: &Path) {
            self.fire(SaveGap::BeforeRename);
        }
    }

    fn entry_names(directory: &Path) -> Vec<String> {
        let mut names = fs::read_dir(directory)
            .expect("directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("utf-8 name")
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn save_rechecks_the_target_revision_before_creating_a_temp() {
        let project = TestProject::new();
        let source = project.path.join("src");
        fs::create_dir(&source).expect("source");
        let file = source.join("note.txt");
        fs::write(&file, "one").expect("one");
        let authority = authority(&project.path);
        let first = authority.read_snapshot(&file).expect("snapshot");
        let hook = SaveRaceHook::new(SaveGap::BeforeTempCreate, || {
            fs::write(&file, "agent edit").expect("agent edit");
        });
        assert_eq!(
            authority.save_text_with_hooks(&file, "draft", &first.revision, Some(&hook)),
            Err(WorkspaceEditorError::StaleRevision)
        );
        assert!(hook.fired.load(Ordering::SeqCst));
        assert_eq!(
            fs::read_to_string(&file).expect("agent bytes"),
            "agent edit"
        );
        assert_eq!(entry_names(&source), ["note.txt"]);
    }

    /// Swaps the file's parent for a symlink to an outside directory in `gap`.
    /// A temp staged before the swap may stay in the parked directory: the
    /// cleanup deliberately refuses to delete through a swapped parent.
    fn assert_parent_swap_is_refused_without_writing_outside(gap: SaveGap) {
        let project = TestProject::new();
        let outside = TestProject::new();
        let source = project.path.join("src");
        let parked = project.path.join("src-before-swap");
        fs::create_dir(&source).expect("source");
        let file = source.join("note.txt");
        fs::write(&file, "one").expect("one");
        fs::write(outside.path.join("note.txt"), "outside original").expect("outside");
        let authority = authority(&project.path);
        let first = authority.read_snapshot(&file).expect("snapshot");
        let hook = SaveRaceHook::new(gap, || {
            fs::rename(&source, &parked).expect("park source");
            symlink(&outside.path, &source).expect("swap source");
        });
        assert_eq!(
            authority.save_text_with_hooks(&file, "draft", &first.revision, Some(&hook)),
            Err(WorkspaceEditorError::PathChanged)
        );
        assert!(hook.fired.load(Ordering::SeqCst));
        assert_eq!(
            fs::read_to_string(parked.join("note.txt")).expect("parked original"),
            "one"
        );
        assert_eq!(
            fs::read_to_string(outside.path.join("note.txt")).expect("outside unchanged"),
            "outside original"
        );
        assert_eq!(entry_names(&outside.path), ["note.txt"]);
    }

    #[test]
    fn parent_swap_before_save_temp_creation_is_refused_without_writing_outside() {
        assert_parent_swap_is_refused_without_writing_outside(SaveGap::BeforeTempCreate);
    }

    #[test]
    fn parent_swap_before_save_rename_is_refused_without_writing_outside() {
        assert_parent_swap_is_refused_without_writing_outside(SaveGap::BeforeRename);
    }

    struct SwapHook {
        source: PathBuf,
        parked: PathBuf,
        outside: PathBuf,
        target: PathBuf,
        swapped: AtomicBool,
    }

    impl EditorHooks for SwapHook {
        fn after_snapshot_validated(&self, file: &Path) {
            if file != self.target || self.swapped.swap(true, Ordering::SeqCst) {
                return;
            }
            fs::rename(&self.source, &self.parked).expect("park source");
            symlink(&self.outside, &self.source).expect("swap source");
        }
    }

    /// Turns the file into a FIFO once it is validated: opened for reading
    /// without `O_NONBLOCK`, a FIFO blocks forever, as a file iCloud has
    /// evicted waits for its download.
    struct EvictHook;

    impl EditorHooks for EvictHook {
        fn after_snapshot_validated(&self, file: &Path) {
            fs::remove_file(file).expect("remove the downloaded copy");
            assert!(
                std::process::Command::new("/usr/bin/mkfifo")
                    .arg(file)
                    .status()
                    .expect("mkfifo")
                    .success()
            );
        }
    }

    #[test]
    fn a_preview_read_still_downloading_from_icloud_says_so_at_its_deadline() {
        let project = TestProject::new();
        let file = project.path.join("report.pdf");
        fs::write(&file, b"%PDF-1.7").expect("file");
        let authority = authority(&project.path);
        let verified = authority.verified_file_kind(&file, true).expect("verified");
        let blocking: fn(&Path) -> std::io::Result<File> = |path| File::open(path);

        let started = std::time::Instant::now();
        let refused = read_verified_bytes_with(
            &verified,
            Some(&EvictHook),
            blocking,
            Duration::from_millis(300),
        )
        .expect_err("a read that never finishes is not waited for");
        assert_eq!(refused, WorkspaceEditorError::StillDownloading);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            refused
                .to_string()
                .contains("still downloading from iCloud")
        );
        let _ = fs::OpenOptions::new().write(true).open(&file);
        assert_eq!(EDITOR_READ_DEADLINE, Duration::from_secs(10));

        // The production read of a local file answers as before.
        fs::remove_file(&file).expect("remove fifo");
        fs::write(&file, b"%PDF-1.7").expect("file again");
        assert_eq!(
            authority
                .preview_file_bytes(&file, None)
                .expect("local read"),
            b"%PDF-1.7"
        );
    }
}
