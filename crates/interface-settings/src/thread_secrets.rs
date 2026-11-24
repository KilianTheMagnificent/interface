//! Private secret files (protocol 67, T3 section 12): where a secret the
//! owner gives an agent lives, and the only place it lives.
//!
//! Layout, under the private data root: `thread-secrets/` (0700), one
//! folder per thread named by a digest of its key (0700), holding a
//! `.thread` file with the key (so Settings can list secrets by thread) and
//! one file per secret, named by the secret (0600), holding exactly the
//! value. A value is written to a fresh 0600 temporary file and renamed into
//! place, so a reader never sees half of one. Nothing here logs, formats or
//! returns a value.
//!
//! A secret expires [`THREAD_SECRET_TTL_MS`] after it was given (its file's
//! modification time): [`sweep`] removes expired ones (the host runs it at
//! start and on a timer), and [`forget_thread`] removes a thread's folder
//! when the thread goes.

use std::{
    fmt::Write as _,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use interface_contracts::thread_secrets::{
    HeldSecret, THREAD_SECRET_TTL_MS, THREAD_SECRET_VALUE_BYTES, valid_secret_name,
};
use interface_storage::{
    PrivateDirectory, StorageError, WriterLock, atomic_write, is_atomic_temp_name, read_bounded,
    snapshot_target,
};
use sha2::{Digest, Sha256};

/// The folder under the data root.
pub const THREAD_SECRETS_DIR: &str = "thread-secrets";
/// A temporary file this old is a write that never finished.
const STALE_TEMPORARY_MS: u64 = 60_000;
/// The file in a thread's folder that names its thread.
const THREAD_FILE: &str = ".thread";
/// Persistent advisory writer lock inside `thread-secrets/`.
const WRITER_LOCK_FILE: &str = ".writer.lock";
const MAX_THREAD_KEY_BYTES: usize = 4 * 1024;

/// Why a secret could not be written, read or removed. Never carries a value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretFileError(pub String);

impl std::fmt::Display for SecretFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SecretFileError {}

fn failed(what: &str, error: &std::io::Error) -> SecretFileError {
    SecretFileError(format!("{what} ({})", error.kind()))
}

fn storage_failed(what: &str, error: &StorageError) -> SecretFileError {
    let detail = match error {
        StorageError::Io { source, .. } => source.kind().to_string(),
        StorageError::LockHeld { .. } => "lock held".to_owned(),
        StorageError::WriteUncertain { .. } => "write uncertain".to_owned(),
        _ => "storage check failed".to_owned(),
    };
    SecretFileError(format!("{what} ({detail})"))
}

/// The folder of the thread `key` (its head's key) under `root`.
#[must_use]
pub fn thread_folder(root: &Path, key: &str) -> PathBuf {
    let digest = Sha256::digest(key.as_bytes());
    let name = digest
        .iter()
        .take(16)
        .fold(String::new(), |mut name, byte| {
            let _ = write!(name, "{byte:02x}");
            name
        });
    root.join(THREAD_SECRETS_DIR).join(name)
}

/// Opens or creates an owner-private `0700` directory via [`PrivateDirectory`],
/// tightening loose permissions on an existing non-symlink directory first.
fn open_private_folder(path: &Path) -> Result<PrivateDirectory, SecretFileError> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(SecretFileError(
                "the secrets folder is not a plain folder".into(),
            ));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .map_err(|error| failed("the secrets folder could not be made private", &error))?;
        }
    }
    PrivateDirectory::open_or_create(path)
        .map_err(|error| storage_failed("the secrets folder could not be opened safely", &error))
}

fn atomic_write_entry(
    directory: &PrivateDirectory,
    lock: &WriterLock,
    name: &str,
    bytes: &[u8],
    max_bytes: usize,
) -> Result<(), SecretFileError> {
    let expected = snapshot_target(directory, name, max_bytes)
        .map_err(|error| storage_failed("the secret target could not be inspected", &error))?;
    atomic_write(directory, lock, name, bytes, &expected, max_bytes)
        .map(|_| ())
        .map_err(|error| storage_failed("the secret could not be saved", &error))
}

/// Writes thread `key`'s secret `name` with `value` under the data root
/// `root`, replacing one of that name. Answers the file's path.
///
/// # Errors
/// A name outside the rules, an empty or too-long value, or the file system
/// refusing; nothing is left half-written.
pub fn write_secret(
    root: &Path,
    key: &str,
    name: &str,
    value: &str,
) -> Result<PathBuf, SecretFileError> {
    if !valid_secret_name(name) {
        return Err(SecretFileError(
            "a secret's name is 1 to 64 letters, digits, `_`, `.` or `-`".into(),
        ));
    }
    if key.is_empty() || key.len() > MAX_THREAD_KEY_BYTES {
        return Err(SecretFileError(
            "a thread key must be non-empty and bounded".into(),
        ));
    }
    if value.is_empty() || value.len() > THREAD_SECRET_VALUE_BYTES {
        return Err(SecretFileError(format!(
            "a secret is 1 byte to {} KiB",
            THREAD_SECRET_VALUE_BYTES / 1024
        )));
    }
    let secrets_root = open_private_folder(&root.join(THREAD_SECRETS_DIR))?;
    let folder_path = thread_folder(root, key);
    let thread_dir = open_private_folder(&folder_path)?;
    let lock = WriterLock::acquire_for(&secrets_root, WRITER_LOCK_FILE, &thread_dir)
        .map_err(|error| storage_failed("the secrets lock could not be acquired", &error))?;

    let existing_key = read_bounded(&thread_dir, THREAD_FILE, MAX_THREAD_KEY_BYTES)
        .ok()
        .flatten()
        .and_then(|file| String::from_utf8(file.bytes().to_vec()).ok());
    if existing_key.as_deref() != Some(key) {
        atomic_write_entry(
            &thread_dir,
            &lock,
            THREAD_FILE,
            key.as_bytes(),
            MAX_THREAD_KEY_BYTES,
        )?;
    }

    let target = folder_path.join(name);
    if fs::symlink_metadata(&target)
        .is_ok_and(|meta| meta.file_type().is_symlink() || !meta.is_file())
    {
        return Err(SecretFileError(
            "the secret's file is not a plain file".into(),
        ));
    }
    atomic_write_entry(
        &thread_dir,
        &lock,
        name,
        value.as_bytes(),
        THREAD_SECRET_VALUE_BYTES,
    )?;
    Ok(target)
}

/// When `path` was written, in ms since the epoch.
fn written_at(path: &Path) -> Option<u64> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok()?;
    u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Every secret under `root`, oldest expiry first. Unreadable entries are
/// left out.
#[must_use]
pub fn list(root: &Path) -> Vec<HeldSecret> {
    let mut held = Vec::new();
    let Ok(secrets_root) = PrivateDirectory::open(root.join(THREAD_SECRETS_DIR)) else {
        return held;
    };
    let Ok(folders) = fs::read_dir(secrets_root.path()) else {
        return held;
    };
    for folder in folders.flatten() {
        let path = folder.path();
        if !folder
            .file_type()
            .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        {
            continue;
        }
        let Ok(thread_dir) = PrivateDirectory::open(&path) else {
            continue;
        };
        let Ok(Some(marker)) = read_bounded(&thread_dir, THREAD_FILE, MAX_THREAD_KEY_BYTES) else {
            continue;
        };
        let Ok(key) = String::from_utf8(marker.bytes().to_vec()) else {
            continue;
        };
        let Ok(files) = fs::read_dir(thread_dir.path()) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if !valid_secret_name(&name)
                || !file
                    .file_type()
                    .is_ok_and(|kind| kind.is_file() && !kind.is_symlink())
            {
                continue;
            }
            if read_bounded(&thread_dir, &name, THREAD_SECRET_VALUE_BYTES)
                .ok()
                .flatten()
                .is_none()
            {
                continue;
            }
            if let Some(at) = written_at(&file.path()) {
                held.push(HeldSecret {
                    key: key.clone(),
                    name,
                    expires_at: at.saturating_add(THREAD_SECRET_TTL_MS),
                });
            }
        }
    }
    held.sort_by(|left, right| {
        (left.expires_at, &left.key, &left.name).cmp(&(right.expires_at, &right.key, &right.name))
    });
    held
}

/// Removes thread `key`'s secret `name`. Answers whether there was one.
///
/// # Errors
/// The file could not be removed.
pub fn forget(root: &Path, key: &str, name: &str) -> Result<bool, SecretFileError> {
    if !valid_secret_name(name) {
        return Ok(false);
    }
    let folder = thread_folder(root, key);
    let Ok(thread_dir) = PrivateDirectory::open(&folder) else {
        if !folder.exists() {
            return Ok(false);
        }
        return Err(SecretFileError(
            "the secrets folder is not a safe private folder".into(),
        ));
    };
    match fs::remove_file(thread_dir.path().join(name)) {
        Ok(()) => {
            remove_if_empty(thread_dir.path());
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failed("the secret could not be removed", &error)),
    }
}

/// Removes every secret thread `key` holds, and its folder.
///
/// # Errors
/// The folder could not be removed.
pub fn forget_thread(root: &Path, key: &str) -> Result<(), SecretFileError> {
    let folder = thread_folder(root, key);
    let meta = match fs::symlink_metadata(&folder) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(failed("the thread's secrets could not be read", &error)),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(SecretFileError(
            "the secrets folder is not a plain folder".into(),
        ));
    }
    match fs::remove_dir_all(&folder) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failed("the thread's secrets could not be removed", &error)),
    }
}

/// A thread's folder that holds nothing but its `.thread` file goes.
fn remove_if_empty(folder: &Path) {
    let only_marker = fs::read_dir(folder).is_ok_and(|entries| {
        entries
            .flatten()
            .all(|entry| entry.file_name() == THREAD_FILE)
    });
    if only_marker {
        let _ = fs::remove_dir_all(folder);
    }
}

/// Removes every secret given at least [`THREAD_SECRET_TTL_MS`] before
/// `now` (ms since the epoch), and every folder left empty. Answers how many
/// secrets went.
#[must_use]
pub fn sweep(root: &Path, now: u64) -> usize {
    let mut removed = 0;
    for secret in list(root) {
        if secret.expires_at <= now && forget(root, &secret.key, &secret.name).unwrap_or(false) {
            removed += 1;
        }
    }
    // Stray temporary files and folders whose secrets all went.
    if let Ok(secrets_root) = PrivateDirectory::open(root.join(THREAD_SECRETS_DIR))
        && let Ok(folders) = fs::read_dir(secrets_root.path())
    {
        for folder in folders.flatten() {
            let path = folder.path();
            if !folder
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                continue;
            }
            if let Ok(files) = fs::read_dir(&path) {
                for file in files.flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    let stale = (name.starts_with(".tmp-") || is_atomic_temp_name(&name))
                        && written_at(&file.path())
                            .is_some_and(|at| now.saturating_sub(at) > STALE_TEMPORARY_MS);
                    if stale {
                        let _ = fs::remove_file(file.path());
                    }
                }
            }
            remove_if_empty(&path);
        }
    }
    removed
}

#[cfg(test)]
#[path = "thread_secrets_tests.rs"]
mod tests;
