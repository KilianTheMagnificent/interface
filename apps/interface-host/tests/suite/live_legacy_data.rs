#![cfg(unix)]

use std::{
    collections::BTreeMap,
    env, fs, io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use interface_host::bootstrap::open_host_services_at;
use sha2::{Digest, Sha256};

const ROOT_FILES: [&str; 4] = [
    "acp-profiles.json",
    "appearance.json",
    "registry.json",
    "worktrees.json",
];
const STATE_DIRECTORIES: [&str; 3] = ["state", "collaboration", "review-checkpoints"];

/// Opt-in acceptance against a byte-for-byte disposable snapshot of the current
/// Interface data. The source is never opened by a Rust writer and no decoded
/// user data is printed by this test.
///
/// Run only after the legacy Interface process has stopped:
///
/// `INTERFACE_LEGACY_DATA_ROOT=... cargo test -p interface-host --test suite -- --ignored`
#[test]
#[ignore = "requires an explicit stopped legacy Interface data root"]
fn current_legacy_data_opens_without_rewriting_canonical_bytes() {
    let source = PathBuf::from(
        env::var_os("INTERFACE_LEGACY_DATA_ROOT")
            .expect("INTERFACE_LEGACY_DATA_ROOT must name the stopped legacy data root"),
    );
    assert!(source.is_absolute(), "legacy data root must be absolute");

    let temporary = tempfile::tempdir().expect("temporary acceptance root");
    let snapshot = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    make_private_directory(&snapshot).expect("private snapshot root");
    make_private_directory(&home).expect("private test home");
    make_private_directory(&home.join("Desktop")).expect("private Desktop");
    make_private_directory(&home.join("Desktop/Projects")).expect("private default project root");

    copy_selected_state(&source, &snapshot).expect("stable legacy snapshot");
    let before = canonical_digests(&snapshot).expect("snapshot digests before open");
    assert!(
        !before.is_empty(),
        "legacy snapshot did not contain canonical state"
    );

    let opened = open_host_services_at(&snapshot, &home).expect("Rust host opens legacy snapshot");
    let registry = opened.runtime.services().session_registry();
    let _session_count = registry.lock().expect("session registry lock").all().len();
    let orchestration = opened
        .runtime
        .services()
        .orchestration()
        .expect("orchestration service");
    let _snapshot = orchestration
        .lock()
        .expect("orchestration lock")
        .snapshot()
        .expect("legacy orchestration snapshot");
    drop(opened);

    let after = canonical_digests(&snapshot).expect("snapshot digests after open");
    assert_eq!(
        before, after,
        "opening legacy state changed a canonical byte before any operation"
    );
}

fn copy_selected_state(source: &Path, destination: &Path) -> io::Result<()> {
    for name in ROOT_FILES {
        copy_if_regular(&source.join(name), &destination.join(name))?;
    }

    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            continue;
        };
        if matches!(
            name_text.rsplit_once('.'),
            Some((stem, "tmp")) if stem.starts_with("registry.json.")
        ) {
            copy_if_regular(&entry.path(), &destination.join(name))?;
        }
    }

    for name in STATE_DIRECTORIES {
        let source_directory = source.join(name);
        if source_directory.exists() {
            copy_tree(&source_directory, &destination.join(name))?;
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::other("legacy state contains a symbolic link"));
    }
    if metadata.is_file() {
        return copy_regular_stable(source, destination);
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    if source.file_name().is_some_and(|name| name == "sockets") {
        return Ok(());
    }
    make_private_directory(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text.ends_with(".lock") || name_text == "host.lock" {
            continue;
        }
        copy_tree(&entry.path(), &destination.join(name))?;
    }
    Ok(())
}

fn copy_if_regular(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::other(
            "legacy canonical state is a symbolic link",
        )),
        Ok(metadata) if metadata.is_file() => copy_regular_stable(source, destination),
        Ok(_) => Err(io::Error::other(
            "legacy canonical state is not a regular file",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn copy_regular_stable(source: &Path, destination: &Path) -> io::Result<()> {
    let before = fs::metadata(source)?;
    if !before.is_file() || before.file_type().is_socket() {
        return Err(io::Error::other("legacy state is not a regular file"));
    }
    let bytes = fs::read(source)?;
    let after = fs::metadata(source)?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        return Err(io::Error::other(
            "legacy state changed while the disposable snapshot was copied",
        ));
    }
    if let Some(parent) = destination.parent() {
        make_private_directory(parent)?;
    }
    fs::write(destination, bytes)?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o600))
}

fn canonical_digests(root: &Path) -> io::Result<BTreeMap<PathBuf, [u8; 32]>> {
    let mut digests = BTreeMap::new();
    digest_tree(root, root, &mut digests)?;
    Ok(digests)
}

fn digest_tree(
    root: &Path,
    current: &Path,
    digests: &mut BTreeMap<PathBuf, [u8; 32]>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            digest_tree(root, &path, digests)?;
            continue;
        }
        if !metadata.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| io::Error::other("snapshot path escaped its root"))?;
        if is_canonical_snapshot_file(relative) {
            let bytes = fs::read(&path)?;
            digests.insert(relative.to_path_buf(), Sha256::digest(bytes).into());
        }
    }
    Ok(())
}

fn is_canonical_snapshot_file(relative: &Path) -> bool {
    if relative.components().count() == 1 {
        let name = relative.to_string_lossy();
        return ROOT_FILES.contains(&name.as_ref())
            || (name.starts_with("registry.json.") && name.ends_with(".tmp"));
    }
    let mut components = relative.components();
    components.next().is_some_and(|first| {
        STATE_DIRECTORIES
            .iter()
            .any(|directory| first.as_os_str() == *directory)
    })
}

fn make_private_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}
