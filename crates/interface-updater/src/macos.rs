use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha512};
use tempfile::{Builder, TempDir};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::timeout;

use crate::local_feed::same_file_state;
use crate::signature::{
    CODESIGN_PATH, DITTO_PATH, is_enforceable_requirement, parse_designated_requirement,
    requirement_arguments,
};
use crate::state::is_newer_version;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt, OpenOptionsExt, PermissionsExt};

const REQUIREMENT_TIMEOUT: Duration = Duration::from_secs(30);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);
const VERIFY_MAX_OUTPUT_BYTES: usize = 1 << 20;
const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const PLUTIL_PATH: &str = "/usr/bin/plutil";

/// A fail-closed macOS artifact-verification failure.
///
/// The variants intentionally contain no provider output or private paths.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum VerificationError {
    #[error("the update archive path is invalid")]
    InvalidArchivePath,
    #[error("the update archive is not a stable private regular file")]
    UnsafeArchive,
    #[error("the update archive exceeds the verification size limit")]
    ArchiveTooLarge,
    #[error("the running app has no enforceable signing requirement")]
    MissingRequirement,
    #[error("the update verifier could not create private working storage")]
    PrivateWorkspace,
    #[error("the update verifier command could not be started")]
    CommandUnavailable,
    #[error("the update verifier command exceeded its time limit")]
    CommandTimeout,
    #[error("the update verifier command exceeded its output limit")]
    CommandOutputTooLarge,
    #[error("the update verifier command rejected the artifact")]
    CommandRejected,
    #[error("the update artifact did not contain exactly one real top-level app bundle")]
    InvalidBundleLayout,
    #[error("the update artifact version is older than or does not match the expected release")]
    RollbackOrVersionMismatch,
}

/// Owner-private retained copy of a verified update archive, pinned by `(dev, ino)`
/// and `SHA-512` so `request_install` can re-verify before allowing restart.
#[derive(Debug)]
pub struct RetainedVerifiedArchive {
    workspace: TempDir,
    archive_path: PathBuf,
    retained_dev: u64,
    retained_ino: u64,
    source_path: PathBuf,
    source_dev: u64,
    source_ino: u64,
    size: u64,
    sha512: [u8; 64],
}

impl RetainedVerifiedArchive {
    #[must_use]
    pub fn archive_path(&self) -> &Path {
        &self.archive_path
    }

    /// Re-verifies the retained `0600` archive inside its owner-private `0700`
    /// directory and re-checks the pinned `(dev, ino)` + `SHA-512` of the
    /// downloaded source archive before install.
    ///
    /// # Errors
    ///
    /// Fails closed if either directory privacy, file identity, permissions, or
    /// `SHA-512` bytes changed after verification.
    pub fn reverify(&self) -> Result<(), VerificationError> {
        assert_private_workspace(self.workspace.path())?;
        verify_pinned_file(
            &self.archive_path,
            self.retained_dev,
            self.retained_ino,
            self.size,
            &self.sha512,
            true,
        )?;
        verify_pinned_file(
            &self.source_path,
            self.source_dev,
            self.source_ino,
            self.size,
            &self.sha512,
            false,
        )
    }
}

/// Reads and self-evaluates the running bundle's designated requirement.
///
/// Printed signing metadata is never trusted on its own. The parsed source is
/// returned only after `codesign` successfully evaluates that exact source
/// against the same bundle.
///
/// # Errors
///
/// Returns a sanitized verifier error for invalid paths, missing tooling,
/// bounded-output overflow, timeout, or a failed self-evaluation.
pub async fn read_mac_app_requirement(app_bundle: &Path) -> Result<String, VerificationError> {
    if !app_bundle.is_absolute() {
        return Err(VerificationError::InvalidArchivePath);
    }
    let display = run_bounded(
        CODESIGN_PATH,
        [
            OsString::from("-d"),
            OsString::from("-r-"),
            OsString::from("--verbose=0"),
            app_bundle.as_os_str().to_owned(),
        ],
        REQUIREMENT_TIMEOUT,
    )
    .await?;

    let mut claimed = display.stdout;
    claimed.push(b'\n');
    claimed.extend_from_slice(&display.stderr);
    let source = String::from_utf8(claimed).map_err(|_| VerificationError::MissingRequirement)?;
    let requirement =
        parse_designated_requirement(&source).ok_or(VerificationError::MissingRequirement)?;

    run_bounded(
        CODESIGN_PATH,
        requirement_arguments(&requirement, app_bundle),
        REQUIREMENT_TIMEOUT,
    )
    .await?;
    Ok(requirement)
}

/// Verifies the exact downloaded archive against the running build's signing
/// requirement before the archive can become installable.
///
/// The source archive is copied through an open, no-follow descriptor into a
/// fresh mode-0700 directory. Extraction and signature checks use absolute
/// Apple tool paths and argv, never a shell or caller-controlled `PATH`.
///
/// # Errors
///
/// Fails closed for unsafe or changing archive bytes, ambiguous bundle layout,
/// weak requirements, unavailable tools, timeout, output overflow, or a
/// signature that does not satisfy the supplied requirement.
pub async fn verify_mac_update_artifact(
    downloaded_archive: &Path,
    requirement: &str,
) -> Result<(), VerificationError> {
    if !is_enforceable_requirement(requirement) {
        return Err(VerificationError::MissingRequirement);
    }
    if !downloaded_archive.is_absolute() {
        return Err(VerificationError::InvalidArchivePath);
    }

    let retained = retain_verified_archive_file(downloaded_archive, None).await?;
    let extraction = create_private_workspace(None)?;
    run_bounded(
        DITTO_PATH,
        [
            OsString::from("-x"),
            OsString::from("-k"),
            retained.archive_path().as_os_str().to_owned(),
            extraction.path().as_os_str().to_owned(),
        ],
        VERIFY_TIMEOUT,
    )
    .await?;
    let app = extracted_bundle_path(extraction.path(), None)?;
    run_bounded(
        CODESIGN_PATH,
        requirement_arguments(requirement, &app),
        VERIFY_TIMEOUT,
    )
    .await?;
    Ok(())
}

/// Verifies and retains a `0600` copy of `downloaded_archive` inside an
/// owner-private `0700` directory, enforcing `CFBundleShortVersionString`
/// anti-rollback (`bundle_version > current_version`) and codesign requirement.
///
/// # Errors
///
/// Fails closed on unsafe storage, non-monotonic versions, bundle layout errors,
/// or signature rejection.
pub async fn verify_and_retain_mac_update_artifact(
    downloaded_archive: &Path,
    requirement: &str,
    current_version: &str,
    expected_version: Option<&str>,
    staging_root: Option<&Path>,
) -> Result<RetainedVerifiedArchive, VerificationError> {
    if !is_enforceable_requirement(requirement) {
        return Err(VerificationError::MissingRequirement);
    }
    if !downloaded_archive.is_absolute() {
        return Err(VerificationError::InvalidArchivePath);
    }

    let retained = retain_verified_archive_file(downloaded_archive, staging_root).await?;
    let extraction = create_private_workspace(None)?;
    run_bounded(
        DITTO_PATH,
        [
            OsString::from("-x"),
            OsString::from("-k"),
            retained.archive_path().as_os_str().to_owned(),
            extraction.path().as_os_str().to_owned(),
        ],
        VERIFY_TIMEOUT,
    )
    .await?;
    let app = extracted_bundle_path(extraction.path(), None)?;
    let bundle_version = read_bundle_short_version(&app).await?;
    if !is_newer_version(&bundle_version, current_version)
        || expected_version.is_some_and(|expected| bundle_version != expected)
    {
        return Err(VerificationError::RollbackOrVersionMismatch);
    }
    run_bounded(
        CODESIGN_PATH,
        requirement_arguments(requirement, &app),
        VERIFY_TIMEOUT,
    )
    .await?;
    Ok(retained)
}

/// Copies `downloaded_archive` into an owner-private `0700` directory as a
/// `0600` regular file, recording `(dev, ino)` and `SHA-512` for install-time
/// re-verification.
///
/// # Errors
///
/// Fails closed if `downloaded_archive` is not a single-link regular file or
/// if the owner-private directory cannot be created and verified.
pub async fn retain_verified_archive_file(
    downloaded_archive: &Path,
    staging_root: Option<&Path>,
) -> Result<RetainedVerifiedArchive, VerificationError> {
    if !downloaded_archive.is_absolute() {
        return Err(VerificationError::InvalidArchivePath);
    }
    let workspace = create_private_workspace(staging_root)?;
    let archive_path = workspace.path().join("download.zip");
    let source = downloaded_archive.to_path_buf();
    let destination = archive_path.clone();
    let copied = tokio::task::spawn_blocking(move || copy_stable_archive(&source, &destination))
        .await
        .map_err(|_| VerificationError::UnsafeArchive)??;
    Ok(RetainedVerifiedArchive {
        workspace,
        archive_path,
        retained_dev: copied.retained_dev,
        retained_ino: copied.retained_ino,
        source_path: downloaded_archive.to_path_buf(),
        source_dev: copied.source_dev,
        source_ino: copied.source_ino,
        size: copied.size,
        sha512: copied.sha512,
    })
}

async fn read_bundle_short_version(app_bundle: &Path) -> Result<String, VerificationError> {
    let info_plist = app_bundle.join("Contents").join("Info.plist");
    let meta =
        fs::symlink_metadata(&info_plist).map_err(|_| VerificationError::InvalidBundleLayout)?;
    if !meta.file_type().is_file() || meta.file_type().is_symlink() {
        return Err(VerificationError::InvalidBundleLayout);
    }
    let output = run_bounded(
        PLUTIL_PATH,
        [
            OsString::from("-extract"),
            OsString::from("CFBundleShortVersionString"),
            OsString::from("raw"),
            OsString::from("-o"),
            OsString::from("-"),
            OsString::from("--"),
            info_plist.as_os_str().to_owned(),
        ],
        REQUIREMENT_TIMEOUT,
    )
    .await?;
    let version = String::from_utf8(output.stdout)
        .map_err(|_| VerificationError::RollbackOrVersionMismatch)?;
    let trimmed = version.trim();
    if trimmed.is_empty() {
        return Err(VerificationError::RollbackOrVersionMismatch);
    }
    Ok(trimmed.to_owned())
}

fn ensure_private_parent(path: &Path) -> Result<(), VerificationError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
                return Err(VerificationError::PrivateWorkspace);
            }
            #[cfg(unix)]
            if meta.permissions().mode() & 0o077 != 0 {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| VerificationError::PrivateWorkspace)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|_| VerificationError::PrivateWorkspace)?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|_| VerificationError::PrivateWorkspace)?;
        }
        Err(_) => return Err(VerificationError::PrivateWorkspace),
    }
    assert_private_workspace(path)
}

fn create_private_workspace(parent: Option<&Path>) -> Result<TempDir, VerificationError> {
    let mut builder = Builder::new();
    builder.prefix("interface-update-verify-");
    #[cfg(unix)]
    builder.permissions(fs::Permissions::from_mode(0o700));
    let workspace = if let Some(root) = parent {
        ensure_private_parent(root)?;
        builder
            .tempdir_in(root)
            .map_err(|_| VerificationError::PrivateWorkspace)?
    } else {
        builder
            .tempdir()
            .map_err(|_| VerificationError::PrivateWorkspace)?
    };
    #[cfg(unix)]
    fs::set_permissions(workspace.path(), fs::Permissions::from_mode(0o700))
        .map_err(|_| VerificationError::PrivateWorkspace)?;
    assert_private_workspace(workspace.path())?;
    Ok(workspace)
}

#[derive(Debug)]
struct BoundedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug)]
struct DrainedOutput {
    retained: Vec<u8>,
    overflowed: bool,
}

async fn run_bounded<I>(
    program: &str,
    arguments: I,
    duration: Duration,
) -> Result<BoundedOutput, VerificationError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut command = Command::new(program);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| VerificationError::CommandUnavailable)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(VerificationError::CommandUnavailable)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(VerificationError::CommandUnavailable)?;
    let process_group = child.id();
    // Pipe EOF belongs to the command deadline too: an exited leader can leave
    // descendants holding stdout/stderr open. No detached reader outlives this
    // future; cancellation drops both pipe descriptors immediately.
    let result = timeout(duration, async {
        tokio::try_join!(child.wait(), drain_bounded(stdout), drain_bounded(stderr))
    })
    .await;
    let (status, stdout, stderr) = match result {
        Ok(Ok(output)) => output,
        failure => {
            #[cfg(unix)]
            if let Some(group) = process_group {
                let mut kill = Command::new("/bin/kill");
                kill.args(["-KILL", "--", &format!("-{group}")])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                let _ = timeout(Duration::from_millis(100), kill.status()).await;
            }
            let _ = child.start_kill();
            let _ = timeout(Duration::from_millis(100), child.wait()).await;
            return Err(if failure.is_err() {
                VerificationError::CommandTimeout
            } else {
                VerificationError::CommandRejected
            });
        }
    };
    if stdout.overflowed || stderr.overflowed {
        return Err(VerificationError::CommandOutputTooLarge);
    }
    if !status.success() {
        return Err(VerificationError::CommandRejected);
    }
    Ok(BoundedOutput {
        stdout: stdout.retained,
        stderr: stderr.retained,
    })
}

async fn drain_bounded(mut reader: impl AsyncRead + Unpin) -> io::Result<DrainedOutput> {
    let mut retained = Vec::new();
    let mut overflowed = false;
    let mut chunk = vec![0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = VERIFY_MAX_OUTPUT_BYTES.saturating_sub(retained.len());
        let keep = remaining.min(read);
        retained.extend_from_slice(&chunk[..keep]);
        overflowed |= keep != read;
    }
    Ok(DrainedOutput {
        retained,
        overflowed,
    })
}

fn assert_private_workspace(path: &Path) -> Result<(), VerificationError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| VerificationError::PrivateWorkspace)?;
    #[cfg(unix)]
    if !metadata.file_type().is_dir()
        || metadata.file_type().is_symlink()
        || interface_common::unix::effective_uid().ok() != Some(metadata.uid())
        || (metadata.permissions().mode() & 0o022) != 0
        || metadata.permissions().mode() & 0o7777 != 0o700
    {
        return Err(VerificationError::PrivateWorkspace);
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct CopiedArchive {
    source_dev: u64,
    source_ino: u64,
    retained_dev: u64,
    retained_ino: u64,
    size: u64,
    sha512: [u8; 64],
}

fn copy_stable_archive(
    source: &Path,
    destination: &Path,
) -> Result<CopiedArchive, VerificationError> {
    let before = fs::symlink_metadata(source).map_err(|_| VerificationError::UnsafeArchive)?;
    validate_archive_metadata(&before)?;
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    source_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut input = source_options
        .open(source)
        .map_err(|_| VerificationError::UnsafeArchive)?;
    let opened = input
        .metadata()
        .map_err(|_| VerificationError::UnsafeArchive)?;
    validate_archive_metadata(&opened)?;
    if !same_file_state(&before, &opened) {
        return Err(VerificationError::UnsafeArchive);
    }

    let mut destination_options = OpenOptions::new();
    destination_options.write(true).create_new(true);
    #[cfg(unix)]
    destination_options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut output = destination_options
        .open(destination)
        .map_err(|_| VerificationError::PrivateWorkspace)?;

    let mut hasher = Sha512::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut copied = 0_u64;
    loop {
        let count = input
            .read(&mut buffer)
            .map_err(|_| VerificationError::UnsafeArchive)?;
        if count == 0 {
            break;
        }
        copied = copied.saturating_add(count as u64);
        if copied > MAX_ARCHIVE_BYTES {
            return Err(VerificationError::ArchiveTooLarge);
        }
        hasher.update(&buffer[..count]);
        output
            .write_all(&buffer[..count])
            .map_err(|_| VerificationError::PrivateWorkspace)?;
    }
    if copied != opened.len() {
        return Err(VerificationError::UnsafeArchive);
    }
    let retained_meta = output
        .metadata()
        .map_err(|_| VerificationError::PrivateWorkspace)?;
    #[cfg(unix)]
    if interface_common::unix::effective_uid().ok() != Some(retained_meta.uid())
        || (retained_meta.permissions().mode() & 0o777) != 0o600
    {
        return Err(VerificationError::PrivateWorkspace);
    }
    let after = input
        .metadata()
        .map_err(|_| VerificationError::UnsafeArchive)?;
    if !same_file_state(&opened, &after) {
        return Err(VerificationError::UnsafeArchive);
    }
    let digest = hasher.finalize();
    let mut sha512 = [0_u8; 64];
    sha512.copy_from_slice(&digest);
    #[cfg(unix)]
    let (source_dev, source_ino, retained_dev, retained_ino) = (
        opened.dev(),
        opened.ino(),
        retained_meta.dev(),
        retained_meta.ino(),
    );
    #[cfg(not(unix))]
    let (source_dev, source_ino, retained_dev, retained_ino) = (0, 0, 0, 0);
    Ok(CopiedArchive {
        source_dev,
        source_ino,
        retained_dev,
        retained_ino,
        size: copied,
        sha512,
    })
}

fn verify_pinned_file(
    path: &Path,
    expected_dev: u64,
    expected_ino: u64,
    expected_size: u64,
    expected_sha512: &[u8; 64],
    require_owner_private_mode: bool,
) -> Result<(), VerificationError> {
    let before = fs::symlink_metadata(path).map_err(|_| VerificationError::UnsafeArchive)?;
    validate_archive_metadata(&before)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(path)
        .map_err(|_| VerificationError::UnsafeArchive)?;
    let meta = file
        .metadata()
        .map_err(|_| VerificationError::UnsafeArchive)?;
    validate_archive_metadata(&meta)?;
    if !same_file_state(&before, &meta) || meta.len() != expected_size {
        return Err(VerificationError::UnsafeArchive);
    }
    #[cfg(unix)]
    {
        if meta.dev() != expected_dev || meta.ino() != expected_ino {
            return Err(VerificationError::UnsafeArchive);
        }
        if require_owner_private_mode
            && (interface_common::unix::effective_uid().ok() != Some(meta.uid())
                || (meta.permissions().mode() & 0o777) != 0o600)
        {
            return Err(VerificationError::PrivateWorkspace);
        }
    }
    #[cfg(not(unix))]
    let _ = (expected_dev, expected_ino, require_owner_private_mode);

    let mut hasher = Sha512::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| VerificationError::UnsafeArchive)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > expected_size {
            return Err(VerificationError::UnsafeArchive);
        }
        hasher.update(&buffer[..count]);
    }
    if total != expected_size {
        return Err(VerificationError::UnsafeArchive);
    }
    let digest = hasher.finalize();
    if digest.as_slice() != expected_sha512 {
        return Err(VerificationError::UnsafeArchive);
    }
    Ok(())
}

fn validate_archive_metadata(metadata: &fs::Metadata) -> Result<(), VerificationError> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(VerificationError::UnsafeArchive);
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err(VerificationError::UnsafeArchive);
    }
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(VerificationError::ArchiveTooLarge);
    }
    Ok(())
}

fn extracted_bundle_path(
    root: &Path,
    ignored_archive: Option<&Path>,
) -> Result<PathBuf, VerificationError> {
    let canonical_root =
        fs::canonicalize(root).map_err(|_| VerificationError::InvalidBundleLayout)?;
    let ignored_name = ignored_archive.and_then(Path::file_name);
    let mut bundles = Vec::new();
    for entry in
        fs::read_dir(&canonical_root).map_err(|_| VerificationError::InvalidBundleLayout)?
    {
        let entry = entry.map_err(|_| VerificationError::InvalidBundleLayout)?;
        let path = entry.path();
        if ignored_name.is_some_and(|name| path.file_name() == Some(name)) {
            continue;
        }
        if path.extension().is_some_and(|extension| extension == "app") {
            bundles.push(path);
        }
    }
    if bundles.len() != 1 {
        return Err(VerificationError::InvalidBundleLayout);
    }
    let app = bundles.pop().expect("length checked");
    let metadata =
        fs::symlink_metadata(&app).map_err(|_| VerificationError::InvalidBundleLayout)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(VerificationError::InvalidBundleLayout);
    }
    let canonical_app =
        fs::canonicalize(&app).map_err(|_| VerificationError::InvalidBundleLayout)?;
    if canonical_app != app || canonical_app.parent() != Some(canonical_root.as_path()) {
        return Err(VerificationError::InvalidBundleLayout);
    }
    Ok(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    #[cfg(unix)]
    #[tokio::test]
    async fn command_deadline_includes_descendants_holding_either_pipe() {
        for script in [
            "printf ready; sleep 2 & exit 0",
            "sleep 2 >&2 & exit 0",
            "sleep 2 & wait",
        ] {
            let started = std::time::Instant::now();
            let result = run_bounded(
                "/bin/sh",
                ["-c", script].map(OsString::from),
                Duration::from_millis(50),
            )
            .await;
            assert_eq!(result.unwrap_err(), VerificationError::CommandTimeout);
            assert!(started.elapsed() < Duration::from_secs(1), "{script}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_collects_both_pipes_and_rejects_failed_status() {
        let output = run_bounded(
            "/bin/sh",
            ["-c", "printf out; printf err >&2"].map(OsString::from),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
        let failure = run_bounded(
            "/bin/sh",
            ["-c", "exit 1"].map(OsString::from),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert_eq!(failure, VerificationError::CommandRejected);
    }

    #[tokio::test]
    async fn drain_caps_retention_and_propagates_read_errors() {
        struct Broken;
        impl AsyncRead for Broken {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<io::Result<()>> {
                std::task::Poll::Ready(Err(io::Error::other("read failed")))
            }
        }
        let bytes = vec![42; VERIFY_MAX_OUTPUT_BYTES + 1];
        let output = drain_bounded(bytes.as_slice()).await.unwrap();
        assert!(output.overflowed);
        assert_eq!(output.retained.len(), VERIFY_MAX_OUTPUT_BYTES);
        assert!(drain_bounded(Broken).await.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn verifier_workspace_is_owner_private() {
        let workspace = create_private_workspace(None).expect("private verifier workspace");
        assert_eq!(
            fs::metadata(workspace.path())
                .expect("workspace metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(assert_private_workspace(workspace.path()), Ok(()));
    }

    #[test]
    fn extracted_bundle_must_be_one_real_top_level_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let app = root.path().join("Interface.app");
        fs::create_dir(&app).expect("app");
        assert_eq!(
            extracted_bundle_path(root.path(), None),
            Ok(fs::canonicalize(&app).expect("canonical app"))
        );

        fs::create_dir(root.path().join("Other.app")).expect("other app");
        assert_eq!(
            extracted_bundle_path(root.path(), None),
            Err(VerificationError::InvalidBundleLayout)
        );
    }

    #[cfg(unix)]
    #[test]
    fn extracted_bundle_refuses_symlinks() {
        let root = tempfile::tempdir().expect("tempdir");
        let genuine = root.path().join("genuine");
        fs::create_dir(&genuine).expect("genuine");
        symlink(&genuine, root.path().join("Interface.app")).expect("symlink");
        assert_eq!(
            extracted_bundle_path(root.path(), None),
            Err(VerificationError::InvalidBundleLayout)
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_snapshot_refuses_links_and_preserves_exact_bytes() {
        let root = tempfile::tempdir().expect("tempdir");
        let source = root.path().join("source.zip");
        let destination = root.path().join("copy.zip");
        fs::write(&source, b"exact archive bytes").expect("source");
        copy_stable_archive(&source, &destination).expect("copy");
        assert_eq!(
            fs::read(&destination).expect("read copy"),
            b"exact archive bytes"
        );
        assert_eq!(
            fs::metadata(&destination)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let alias = root.path().join("alias.zip");
        fs::hard_link(&source, &alias).expect("hard link");
        assert_eq!(
            copy_stable_archive(&source, &root.path().join("refused.zip")),
            Err(VerificationError::UnsafeArchive)
        );
    }
}
