use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use interface_common::unix::{effective_uid, same_stable_metadata};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const OWNER_FILE_MODE: u32 = 0o600;

const MAX_ACQUIRE_RACES: usize = 3;
const MAX_LOCK_BYTES: u64 = 4_096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LockOwner {
    pid: u32,
    token: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LockIdentity {
    dev: u64,
    ino: u64,
    ctime: i64,
    ctime_nsec: i64,
}

#[derive(Clone, Debug)]
struct LockSnapshot {
    owner: LockOwner,
    identity: LockIdentity,
}

#[derive(Debug)]
pub struct CliInstanceLock {
    path: PathBuf,
    owner: LockOwner,
    identity: LockIdentity,
    released: bool,
}

impl CliInstanceLock {
    /// Revalidates that the stable named lock still belongs to this process.
    ///
    /// # Errors
    ///
    /// Returns an error when the path, inode, metadata, PID, or random owner
    /// token no longer matches the acquired lock.
    pub fn assert_held(&self) -> Result<(), String> {
        if self.released {
            return Err(no_longer_owned(&self.path));
        }
        let current = read_owner(&self.path)?.ok_or_else(|| no_longer_owned(&self.path))?;
        if current.owner != self.owner || current.identity != self.identity {
            return Err(no_longer_owned(&self.path));
        }
        Ok(())
    }

    /// Releases the exact lock inode owned by this process.
    ///
    /// # Errors
    ///
    /// Returns an error instead of unlinking if ownership or inode identity
    /// changed after acquisition.
    pub fn release(mut self) -> Result<(), String> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> Result<(), String> {
        if self.released {
            return Ok(());
        }
        remove_owned(&self.path, &self.owner, Some(self.identity))?;
        self.released = true;
        Ok(())
    }
}

impl Drop for CliInstanceLock {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.release_inner();
        }
    }
}

/// Acquires the sole 0.1.169-compatible writer lock for one CLI data tree.
///
/// # Errors
///
/// Returns an error for an active owner, unsafe or unreadable state, uncertain
/// liveness, or any race that prevents exact ownership proof.
pub fn acquire_cli_instance_lock(path: &Path) -> Result<CliInstanceLock, String> {
    acquire_with(
        path,
        std::process::id(),
        Uuid::new_v4().to_string(),
        process_is_alive,
    )
}

fn acquire_with<F>(
    path: &Path,
    pid: u32,
    token: String,
    alive: F,
) -> Result<CliInstanceLock, String>
where
    F: Fn(u32) -> Result<bool, String> + Copy,
{
    if !path.is_absolute() {
        return Err("The iface instance lock path must be absolute.".into());
    }
    let owner = LockOwner { pid, token };
    if !valid_owner(&owner) {
        return Err("The iface instance lock owner is invalid.".into());
    }
    for _ in 0..MAX_ACQUIRE_RACES {
        if let Some(identity) = try_create(path, &owner)? {
            return Ok(handle(path, owner, identity));
        }
        let Some(current) = read_owner(path)? else {
            continue;
        };
        if owner_is_alive(&current.owner, path, alive)? {
            return Err(active_owner(path, current.owner.pid));
        }
        return recover_stale(path, &owner, alive);
    }
    Err(format!(
        "The iface instance lock at {} changed repeatedly; refusing to continue.",
        path.display()
    ))
}

fn handle(path: &Path, owner: LockOwner, identity: LockIdentity) -> CliInstanceLock {
    CliInstanceLock {
        path: path.to_owned(),
        owner,
        identity,
        released: false,
    }
}

fn recover_stale<F>(path: &Path, owner: &LockOwner, alive: F) -> Result<CliInstanceLock, String>
where
    F: Fn(u32) -> Result<bool, String> + Copy,
{
    let recovery = PathBuf::from(format!("{}.recovery", path.display()));
    let recovery_identity = if let Some(identity) = try_create(&recovery, owner)? {
        identity
    } else {
        let current = read_owner(&recovery)?.ok_or_else(|| {
            format!(
                "The stale-lock recovery guard at {} changed; refusing to continue.",
                recovery.display()
            )
        })?;
        if owner_is_alive(&current.owner, &recovery, alive)? {
            return Err(format!(
                "Another iface process is recovering the stale lock at {} (PID {}); refusing concurrent recovery.",
                path.display(),
                current.owner.pid
            ));
        }
        remove_owned(&recovery, &current.owner, Some(current.identity))?;
        try_create(&recovery, owner)?.ok_or_else(|| {
            format!(
                "The stale-lock recovery guard at {} changed during recovery; refusing to continue.",
                recovery.display()
            )
        })?
    };

    let acquired = (|| {
        if let Some(current) = read_owner(path)? {
            if owner_is_alive(&current.owner, path, alive)? {
                return Err(active_owner(path, current.owner.pid));
            }
            remove_owned(path, &current.owner, Some(current.identity))?;
        }
        let identity = try_create(path, owner)?.ok_or_else(|| {
            format!(
                "The iface instance lock at {} changed during stale recovery; refusing to continue.",
                path.display()
            )
        })?;
        Ok(handle(path, owner.clone(), identity))
    })();

    if let Err(error) = remove_owned(&recovery, owner, Some(recovery_identity)) {
        if let Ok(acquired) = acquired {
            drop(acquired);
        }
        return Err(format!(
            "Unable to release the stale-lock recovery guard at {}; iface startup was refused: {error}",
            recovery.display()
        ));
    }
    acquired
}

fn try_create(path: &Path, owner: &LockOwner) -> Result<Option<LockIdentity>, String> {
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(OWNER_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Unable to create the iface instance lock at {}: {error}",
                path.display()
            ));
        }
    };
    // Keep the descriptor open while cleanup is decided. An unlinked inode
    // cannot be reused while this descriptor still refers to it, so a named
    // file with a different device/inode pair is provably a replacement and
    // must never be removed as part of this failed initialization.
    let opened = file.metadata().map_err(|error| {
        format!(
            "Unable to inspect the newly created iface instance lock at {}: {error}",
            path.display()
        )
    })?;
    let initialized: Result<LockIdentity, String> = (|| {
        let mut payload = serde_json::to_vec(owner).map_err(|error| error.to_string())?;
        payload.push(b'\n');
        file.write_all(&payload)
            .map_err(|error| error.to_string())?;
        file.set_permissions(fs::Permissions::from_mode(OWNER_FILE_MODE))
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        assert_owner_only_regular(&metadata, path)?;
        Ok(identity(&metadata))
    })();
    let created = match initialized {
        Ok(identity) => identity,
        Err(error) => {
            let cleanup = remove_created_inode(path, &opened);
            drop(file);
            return cleanup.map_or_else(
                |cleanup_error| {
                    Err(format!(
                        "Unable to write the iface instance lock at {}: {error}; {cleanup_error}",
                        path.display()
                    ))
                },
                |()| {
                    Err(format!(
                        "Unable to write the iface instance lock at {}: {error}",
                        path.display()
                    ))
                },
            );
        }
    };
    drop(file);
    let verified = read_owner(path).map_err(|error| {
        format!(
            "Unable to verify the newly created iface instance lock at {}: {error}",
            path.display()
        )
    })?;
    if verified
        .as_ref()
        .is_none_or(|current| current.owner != *owner || current.identity != created)
    {
        return Err(format!(
            "The newly created iface instance lock at {} changed before ownership could be verified.",
            path.display()
        ));
    }
    Ok(Some(created))
}

fn remove_created_inode(path: &Path, opened: &fs::Metadata) -> Result<(), String> {
    let named = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "Unable to verify failed instance-lock cleanup at {}: {error}",
            path.display()
        )
    })?;
    if named.dev() != opened.dev() || named.ino() != opened.ino() {
        return Err(format!(
            "The failed iface instance lock at {} was replaced; refusing to remove it",
            path.display()
        ));
    }
    fs::remove_file(path).map_err(|error| {
        format!(
            "Unable to remove the failed iface instance lock at {}: {error}",
            path.display()
        )
    })
}

fn read_owner(path: &Path) -> Result<Option<LockSnapshot>, String> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(invalid_lock(path, Some(&error))),
    };
    assert_owner_only_regular(&before, path)?;
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| invalid_lock(path, Some(&error)))?;
    let opened = file
        .metadata()
        .map_err(|error| invalid_lock(path, Some(&error)))?;
    assert_owner_only_regular(&opened, path)?;
    if !same_stable_metadata(&before, &opened) {
        return Err(invalid_lock(path, None));
    }
    let mut contents = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(MAX_LOCK_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| invalid_lock(path, Some(&error)))?;
    let after = file
        .metadata()
        .map_err(|error| invalid_lock(path, Some(&error)))?;
    assert_owner_only_regular(&after, path)?;
    if contents.len() as u64 != after.len() || !same_stable_metadata(&opened, &after) {
        return Err(invalid_lock(path, None));
    }
    let owner: LockOwner =
        serde_json::from_slice(&contents).map_err(|_| invalid_lock(path, None))?;
    if !valid_owner(&owner) {
        return Err(invalid_lock(path, None));
    }
    drop(file);
    let named = fs::symlink_metadata(path).map_err(|error| invalid_lock(path, Some(&error)))?;
    assert_owner_only_regular(&named, path)?;
    if !same_stable_metadata(&after, &named) {
        return Err(invalid_lock(path, None));
    }
    Ok(Some(LockSnapshot {
        owner,
        identity: identity(&after),
    }))
}

fn remove_owned(
    path: &Path,
    owner: &LockOwner,
    expected: Option<LockIdentity>,
) -> Result<(), String> {
    let current = read_owner(path)?;
    if current.as_ref().is_none_or(|current| {
        current.owner != *owner || expected.is_some_and(|identity| current.identity != identity)
    }) {
        return Err(format!(
            "The iface instance lock at {} is no longer owned by this process; refusing to remove it.",
            path.display()
        ));
    }
    fs::remove_file(path).map_err(|error| {
        format!(
            "Unable to release the iface instance lock at {}: {error}",
            path.display()
        )
    })
}

fn owner_is_alive<F>(owner: &LockOwner, path: &Path, alive: F) -> Result<bool, String>
where
    F: Fn(u32) -> Result<bool, String>,
{
    alive(owner.pid).map_err(|error| {
        format!(
            "Unable to verify whether iface lock owner PID {} is alive; refusing to alter {}: {error}",
            owner.pid,
            path.display()
        )
    })
}

fn process_is_alive(pid: u32) -> Result<bool, String> {
    let status = Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        return Ok(true);
    }
    let output = interface_terminal::PsProbe {
        columns: &["pid"],
        select: &["-p", &pid.to_string()],
        utc: false,
        stderr: false,
    }
    .output()
    .map_err(|error| error.to_string())?;
    Ok(output.status.success() && !output.stdout.iter().all(u8::is_ascii_whitespace))
}

fn valid_owner(owner: &LockOwner) -> bool {
    owner.pid > 0
        && (16..=128).contains(&owner.token.len())
        && owner
            .token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn assert_owner_only_regular(metadata: &fs::Metadata, path: &Path) -> Result<(), String> {
    // `EffectiveUidError` displays the same text this check always returned.
    let expected_uid = effective_uid().map_err(|error| error.to_string())?;
    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != OWNER_FILE_MODE
        || metadata.len() == 0
        || metadata.len() > MAX_LOCK_BYTES
        || metadata.uid() != expected_uid
    {
        return Err(invalid_lock(path, None));
    }
    Ok(())
}

fn identity(metadata: &fs::Metadata) -> LockIdentity {
    LockIdentity {
        dev: metadata.dev(),
        ino: metadata.ino(),
        ctime: metadata.ctime(),
        ctime_nsec: metadata.ctime_nsec(),
    }
}

fn invalid_lock(path: &Path, cause: Option<&std::io::Error>) -> String {
    let suffix = cause.map_or_else(String::new, |error| format!(" ({error})"));
    format!(
        "The iface instance lock at {} is invalid or unreadable; refusing to risk shared registry corruption.{suffix}",
        path.display()
    )
}

fn active_owner(path: &Path, pid: u32) -> String {
    format!(
        "Another iface process is already using {} (PID {pid}). One iface TUI can manage multiple sessions; close the running instance first.",
        path.parent().unwrap_or(path).display()
    )
}

fn no_longer_owned(path: &Path) -> String {
    format!(
        "The iface instance lock at {} is no longer owned by this process; refusing to continue.",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temp");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("mode");
        root
    }

    #[test]
    fn exact_owner_record_refuses_a_second_live_writer_and_releases() {
        let root = private_root();
        let path = root.path().join("instance.lock");
        let first =
            acquire_with(&path, 41, "0123456789abcdef".into(), |pid| Ok(pid == 41)).expect("first");
        first.assert_held().expect("held");
        let error = acquire_with(&path, 42, "fedcba9876543210".into(), |pid| Ok(pid == 41))
            .expect_err("second refused");
        assert!(error.contains("Another iface process"));
        first.release().expect("release");
        assert!(!path.exists());
    }

    #[test]
    fn dead_owner_is_recovered_under_a_private_guard() {
        let root = private_root();
        let path = root.path().join("instance.lock");
        let stale =
            acquire_with(&path, 41, "0123456789abcdef".into(), |_| Ok(true)).expect("stale");
        std::mem::forget(stale);
        let replacement =
            acquire_with(&path, 42, "fedcba9876543210".into(), |_| Ok(false)).expect("replacement");
        replacement.assert_held().expect("held");
        assert!(!PathBuf::from(format!("{}.recovery", path.display())).exists());
        replacement.release().expect("release");
    }

    #[test]
    fn malformed_symlink_and_replaced_inode_fail_closed() {
        let root = private_root();
        let path = root.path().join("instance.lock");
        fs::write(&path, "bad").expect("bad");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("mode");
        assert!(
            acquire_with(&path, 42, "fedcba9876543210".into(), |_| Ok(false))
                .expect_err("malformed")
                .contains("invalid or unreadable")
        );
        fs::remove_file(&path).expect("remove bad");
        let held = acquire_with(&path, 42, "fedcba9876543210".into(), |_| Ok(false)).expect("held");
        fs::remove_file(&path).expect("replace remove");
        let replacement = LockOwner {
            pid: 42,
            token: "fedcba9876543210".into(),
        };
        let _ = try_create(&path, &replacement).expect("replacement");
        assert!(held.assert_held().is_err());
        std::mem::forget(held);
        remove_owned(&path, &replacement, None).expect("cleanup");
    }

    #[test]
    fn failed_creation_cleanup_never_unlinks_a_replacement_inode() {
        let root = private_root();
        let path = root.path().join("instance.lock");
        let opened = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(OWNER_FILE_MODE)
            .open(&path)
            .expect("created lock");
        let metadata = opened.metadata().expect("opened metadata");
        fs::remove_file(&path).expect("unlink opened lock");
        fs::write(&path, b"replacement\n").expect("replacement");
        fs::set_permissions(&path, fs::Permissions::from_mode(OWNER_FILE_MODE)).expect("mode");

        let error = remove_created_inode(&path, &metadata).expect_err("replacement refused");

        assert!(error.contains("was replaced"));
        assert_eq!(
            fs::read(&path).expect("replacement preserved"),
            b"replacement\n"
        );
        drop(opened);
    }
}
