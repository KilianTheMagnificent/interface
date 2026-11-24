use std::fs::{self, OpenOptions};
use std::path::Path;

use interface_storage::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE, PrivateDirectory, StorageError};
use thiserror::Error;

use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("settings path must be an absolute file path")]
    InvalidPath,
    #[error("settings storage is unsafe: {0}")]
    UnsafeStorage(&'static str),
    #[error("invalid agent workspace: {0}")]
    InvalidAgent(String),
    #[error("invalid ACP profile configuration: {0}")]
    InvalidAcp(String),
    #[error("invalid saved terminal workspace: {0}")]
    InvalidTerminalWorkspace(String),
    #[error("Interface could not resolve the user's home directory")]
    InvalidHome,
    #[error("settings storage operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl SettingsError {
    /// Whether a failed write may still have reached the disk: only a
    /// failure after the atomic rename is uncertain.
    #[must_use]
    pub const fn write_may_have_landed(&self) -> bool {
        matches!(self, Self::Storage(StorageError::WriteUncertain { .. }))
    }
}

pub(crate) struct StoreLocation {
    pub(crate) directory: PrivateDirectory,
    pub(crate) name: String,
}

pub(crate) fn prepare_store(file: &Path) -> Result<StoreLocation, SettingsError> {
    if !file.is_absolute() || file.file_name().is_none() {
        return Err(SettingsError::InvalidPath);
    }
    let parent = file.parent().ok_or(SettingsError::InvalidPath)?;
    prepare_private_directory(parent)?;
    if fs::symlink_metadata(file).is_ok() {
        harden_existing_file(file)?;
    }
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .ok_or(SettingsError::InvalidPath)?
        .to_owned();
    Ok(StoreLocation {
        directory: PrivateDirectory::open(parent)?,
        name,
    })
}

fn prepare_private_directory(directory: &Path) -> Result<(), SettingsError> {
    if !directory.is_absolute() {
        return Err(SettingsError::InvalidPath);
    }
    match fs::symlink_metadata(directory) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(OWNER_DIRECTORY_MODE);
            builder.create(directory)?;
        }
        Err(error) => return Err(error.into()),
    }

    let before = fs::symlink_metadata(directory)?;
    if !before.file_type().is_dir() || before.file_type().is_symlink() {
        return Err(SettingsError::UnsafeStorage(
            "the private parent must be a real directory, not a link",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let opened = options.open(directory)?;
    let opened_before = opened.metadata()?;
    if !opened_before.is_dir() || !same_node(&before, &opened_before) {
        return Err(SettingsError::UnsafeStorage(
            "the private parent changed while it was opened",
        ));
    }
    // Harden (and flush the new mode) only when it differs, so reads and
    // saves of an already private store never mutate metadata.
    if full_mode(&opened_before) != OWNER_DIRECTORY_MODE {
        opened.set_permissions(fs::Permissions::from_mode(OWNER_DIRECTORY_MODE))?;
        opened.sync_all()?;
    }
    let opened_after = opened.metadata()?;
    let named_after = fs::symlink_metadata(directory)?;
    if !opened_after.is_dir()
        || !same_node(&opened_before, &opened_after)
        || !same_node(&opened_after, &named_after)
        || permission_mode(&opened_after) != OWNER_DIRECTORY_MODE
    {
        return Err(SettingsError::UnsafeStorage(
            "the private parent changed while permissions were hardened",
        ));
    }
    Ok(())
}

fn harden_existing_file(file: &Path) -> Result<(), SettingsError> {
    let before = fs::symlink_metadata(file)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() || link_count(&before) != 1
    {
        return Err(SettingsError::UnsafeStorage(
            "the settings target must be a regular, non-linked file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let opened = options.open(file)?;
    let opened_before = opened.metadata()?;
    if !opened_before.is_file()
        || link_count(&opened_before) != 1
        || !same_node(&before, &opened_before)
    {
        return Err(SettingsError::UnsafeStorage(
            "the settings target changed while it was opened",
        ));
    }
    if full_mode(&opened_before) != OWNER_FILE_MODE {
        opened.set_permissions(fs::Permissions::from_mode(OWNER_FILE_MODE))?;
        opened.sync_all()?;
    }
    let opened_after = opened.metadata()?;
    let named_after = fs::symlink_metadata(file)?;
    if !same_node(&opened_before, &opened_after)
        || !same_node(&opened_after, &named_after)
        || link_count(&opened_after) != 1
        || permission_mode(&opened_after) != OWNER_FILE_MODE
    {
        return Err(SettingsError::UnsafeStorage(
            "the settings target changed while permissions were hardened",
        ));
    }
    Ok(())
}

fn same_node(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

fn permission_mode(metadata: &fs::Metadata) -> u32 {
    metadata.mode() & 0o777
}

/// Permission bits including setuid, setgid and sticky, which a harden clears.
fn full_mode(metadata: &fs::Metadata) -> u32 {
    metadata.mode() & 0o7777
}

fn link_count(metadata: &fs::Metadata) -> u64 {
    metadata.nlink()
}

pub(crate) fn map_document_write(
    error: StorageError,
    invalid: fn(String) -> SettingsError,
) -> SettingsError {
    match error {
        StorageError::BoundsExceeded { max_bytes, .. } => {
            invalid(format!("document exceeds {max_bytes} bytes"))
        }
        other => SettingsError::Storage(other),
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsError;

    #[test]
    fn a_storage_failure_names_its_cause() {
        let cause = std::io::Error::from(std::io::ErrorKind::StorageFull);
        let expected = format!("settings storage operation failed: {cause}");
        assert_eq!(SettingsError::from(cause).to_string(), expected);
    }
}
