//! Claude's per-process session pointer (`sessions/<pid>.json`) and the
//! owned-directory snapshots that pin the provider store while it is read.

use super::process_probe::normalize_whitespace;
use super::{
    Path, PathBuf, TerminalBackendError, TerminalProcessIdentity, canonical_uuid_v4,
    invalid_registry,
};

pub(super) fn read_current_session_with<F>(
    sessions_root: &Path,
    process: &TerminalProcessIdentity,
    validate_process: F,
) -> Result<Option<String>, TerminalBackendError>
where
    F: Fn(&TerminalProcessIdentity) -> Result<(), TerminalBackendError>,
{
    use std::{fs, io::Read, os::unix::fs::OpenOptionsExt};

    const MAX_SESSION_RECORD_BYTES: u64 = 16 * 1_024;

    let path = sessions_root.join(format!("{}.json", process.process_id));
    let before = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid_registry()),
    };
    validate_metadata(&before, MAX_SESSION_RECORD_BYTES)?;

    // O_NONBLOCK makes a pre-open replacement with a pipe or device fail
    // promptly; O_NOFOLLOW independently closes the final-component symlink
    // race between the named metadata check and open.
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|_| invalid_registry())?;
    let opened = file.metadata().map_err(|_| invalid_registry())?;
    validate_metadata(&opened, MAX_SESSION_RECORD_BYTES)?;
    if !same_stable_metadata(&before, &opened) {
        return Err(invalid_registry());
    }

    let mut contents = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_SESSION_RECORD_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|_| invalid_registry())?;
    let after = file.metadata().map_err(|_| invalid_registry())?;
    validate_metadata(&after, MAX_SESSION_RECORD_BYTES)?;
    if u64::try_from(contents.len()).ok() != Some(after.len())
        || !same_stable_metadata(&opened, &after)
    {
        return Err(invalid_registry());
    }

    // Recheck the name while the descriptor remains open. A replace-by-rename
    // cannot make an old, already-open record look like the current PID file.
    let named = fs::symlink_metadata(&path).map_err(|_| invalid_registry())?;
    validate_metadata(&named, MAX_SESSION_RECORD_BYTES)?;
    if !same_stable_metadata(&after, &named) {
        return Err(invalid_registry());
    }

    let record: ClaudeSessionRecord =
        serde_json::from_slice(&contents).map_err(|_| invalid_registry())?;
    let session_id = validate_record(record, process)?;
    // This is deliberately last. `lstart` has only one-second resolution, so
    // the retained process group and controlling PTY are re-read after the
    // sidecar snapshot to close same-second PID-reuse and stale-file ABA.
    validate_process(process)?;
    // The provider can rewrite this pointer while the process check runs.
    // Revalidate both the open descriptor and its current name afterward so
    // the returned ID describes one stable snapshot on both sides of that
    // potentially slow system call.
    let final_opened = file.metadata().map_err(|_| invalid_registry())?;
    validate_metadata(&final_opened, MAX_SESSION_RECORD_BYTES)?;
    let final_named = fs::symlink_metadata(&path).map_err(|_| invalid_registry())?;
    validate_metadata(&final_named, MAX_SESSION_RECORD_BYTES)?;
    if !same_stable_metadata(&after, &final_opened)
        || !same_stable_metadata(&final_opened, &final_named)
    {
        return Err(invalid_registry());
    }
    Ok(Some(session_id))
}

pub(super) fn validate_metadata(
    metadata: &std::fs::Metadata,
    max_bytes: u64,
) -> Result<(), TerminalBackendError> {
    use std::os::unix::fs::MetadataExt;

    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > max_bytes
        || metadata.uid() != effective_uid()?
        || metadata.mode() & 0o022 != 0
    {
        return Err(invalid_registry());
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DirectoryIdentity {
    device: u64,
    inode: u64,
}

pub(super) fn snapshot_owned_directory(
    path: &Path,
) -> Result<Option<DirectoryIdentity>, TerminalBackendError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid_registry()),
    };
    if !metadata.file_type().is_dir()
        || metadata.uid() != effective_uid()?
        || metadata.mode() & 0o022 != 0
    {
        return Err(invalid_registry());
    }
    Ok(Some(DirectoryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }))
}

pub(super) fn assert_owned_directory(
    path: &Path,
    expected: DirectoryIdentity,
) -> Result<(), TerminalBackendError> {
    let observed = snapshot_owned_directory(path)?;
    if observed != Some(expected) {
        return Err(invalid_registry());
    }
    Ok(())
}

pub(super) fn effective_uid() -> Result<u32, TerminalBackendError> {
    interface_common::unix::effective_uid().map_err(|_| invalid_registry())
}

/// The shared stable-metadata check, with the group pinned as well.
pub(super) fn same_stable_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    interface_common::unix::same_stable_metadata(left, right) && left.gid() == right.gid()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClaudeSessionRecord {
    version: String,
    pid: u32,
    session_id: String,
    cwd: PathBuf,
    proc_start: String,
    kind: String,
}

pub(super) fn validate_record(
    record: ClaudeSessionRecord,
    process: &TerminalProcessIdentity,
) -> Result<String, TerminalBackendError> {
    if record.pid != process.process_id
        || !supported_registry_version(&record.version)
        || record.kind != "interactive"
        || !record.cwd.is_absolute()
        || normalize_whitespace(&record.proc_start).is_empty()
        || normalize_whitespace(&record.proc_start) != normalize_whitespace(&process.process_start)
        || !canonical_uuid_v4(&record.session_id)
    {
        return Err(invalid_registry());
    }
    Ok(record.session_id)
}

pub(super) fn supported_registry_version(value: &str) -> bool {
    let mut parts = value.split('.');
    let Some(major) = canonical_version_component(parts.next()) else {
        return false;
    };
    let Some(minor) = canonical_version_component(parts.next()) else {
        return false;
    };
    let Some(patch) = canonical_version_component(parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    major == 2 && (minor > 1 || (minor == 1 && patch >= 258))
}

pub(super) fn canonical_version_component(value: Option<&str>) -> Option<u64> {
    let value = value?;
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}
