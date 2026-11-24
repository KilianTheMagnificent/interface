//! Owner-private create_new-temp + fsync + rename writes.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use interface_storage::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE};

static WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` over `path` with `create_new` + chmod 0600 + fsync + rename.
///
/// The parent directory is created at 0700 and refused if it is a symbolic link,
/// the temporary file is opened with `O_NOFOLLOW` and chmodded through its
/// descriptor before the fsync (rename keeps that inode and mode), and both the
/// file and parent directory are fsynced.
pub(crate) fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "private state path has no parent".to_owned())?;
    ensure_private_directory(parent)?;
    let sequence = WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let temporary = parent.join(format!(
        ".{file_name}.{}.{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = open_owner_new(&temporary)?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.set_permissions(owner_file_permissions())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Creates `path` at 0700 if needed and re-asserts owner-private mode on it.
///
/// The single owner-private directory guard for the CLI stores: a symlinked or
/// non-directory path is refused rather than followed, and an existing 0700
/// directory skips the chmod.
///
/// # Errors
///
/// Returns an error when the path is a symbolic link or not a directory, or
/// when it cannot be created or made owner-private.
pub(crate) fn ensure_private_directory(path: &Path) -> Result<(), String> {
    if let Ok(metadata) = path.symlink_metadata() {
        if metadata.file_type().is_symlink() {
            return Err("Private state directory must not be a symbolic link".into());
        }
        if !metadata.is_dir() {
            return Err("Private state path is not a directory".into());
        }
        if already_owner_private(&metadata) {
            return Ok(());
        }
        return fs::set_permissions(path, owner_directory_permissions())
            .map_err(|error| error.to_string());
    }
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    let metadata = path.symlink_metadata().map_err(|error| error.to_string())?;
    if !metadata.is_dir() {
        return Err("Private state path is not a directory".into());
    }
    fs::set_permissions(path, owner_directory_permissions()).map_err(|error| error.to_string())
}

/// Opens `path` for owner-private appending, refusing to follow a symlink.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or made owner-private.
pub(crate) fn open_owner_append(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .append(true)
        .write(true)
        .mode(OWNER_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|error| error.to_string())?;
    file.set_permissions(owner_file_permissions())
        .map_err(|error| error.to_string())?;
    Ok(file)
}

fn already_owner_private(metadata: &fs::Metadata) -> bool {
    metadata.permissions().mode() & 0o777 == OWNER_DIRECTORY_MODE
}

fn open_owner_new(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options
        .create_new(true)
        .write(true)
        .mode(OWNER_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
    options.open(path).map_err(|error| error.to_string())
}

fn owner_directory_permissions() -> fs::Permissions {
    fs::Permissions::from_mode(OWNER_DIRECTORY_MODE)
}

fn owner_file_permissions() -> fs::Permissions {
    fs::Permissions::from_mode(OWNER_FILE_MODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_a_symlinked_parent_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real");
        let linked = temp.path().join("linked");
        fs::create_dir(&real).expect("real");
        std::os::unix::fs::symlink(&real, &linked).expect("symlink");
        let error =
            write_private_bytes(&linked.join("state.json"), b"{}\n").expect_err("symlink refused");
        assert!(error.contains("symbolic link"), "{error}");
        assert!(!real.join("state.json").exists());
    }

    #[test]
    fn owner_private_modes_are_the_frozen_store_modes() {
        assert_eq!(OWNER_DIRECTORY_MODE, 0o700);
        assert_eq!(OWNER_FILE_MODE, 0o600);
    }

    #[test]
    fn writes_owner_private_bytes() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("state.json");
        write_private_bytes(&path, b"{}\n").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"{}\n");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, OWNER_FILE_MODE);
    }
}
