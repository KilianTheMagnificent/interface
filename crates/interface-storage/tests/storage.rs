use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::Path;

use interface_storage::{
    JsonLimits, PrivateDirectory, QuarantineReason, QuarantinedArtifact, Revisioned, StorageError,
    TargetSnapshot, WriterLock, atomic_temp_digest_prefix, atomic_write_json, compare_and_set_json,
    is_atomic_temp_name, is_atomic_temp_name_for, quarantine_file,
    quarantine_stale_atomic_writes_for, read_bounded, read_json, snapshot_target,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use uuid::Uuid;

const FILE_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Record {
    revision: u64,
    value: String,
}

impl Revisioned for Record {
    fn revision(&self) -> u64 {
        self.revision
    }
}

fn fixture() -> (TempDir, PrivateDirectory) {
    let scratch = tempfile::tempdir().expect("tempdir");
    let directory =
        PrivateDirectory::open_or_create(scratch.path().join("store")).expect("private store");
    (scratch, directory)
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut options = fs::OpenOptions::new();
    let mut file = options
        .write(true)
        .create_new(true)
        .open(path)
        .expect("create private fixture");
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .expect("private fixture mode");
    file.write_all(bytes).expect("write private fixture");
    file.sync_all().expect("sync private fixture");
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).expect("metadata").mode() & 0o7777
}

#[test]
fn read_is_exact_and_never_normalizes_or_creates_recovery_state() {
    let (_scratch, directory) = fixture();
    let path = directory.path().join("state.json");
    let source = br#"{"revision":0,  "value":"kept exactly"}"#;
    write_private(&path, source);
    let before = fs::symlink_metadata(&path).expect("metadata before");
    let entries_before = fs::read_dir(directory.path())
        .expect("entries before")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();

    let loaded = read_json::<Record>(&directory, "state.json", JsonLimits::default())
        .expect("read")
        .expect("present");

    assert_eq!(loaded.value().value, "kept exactly");
    assert_eq!(fs::read(&path).expect("bytes after"), source);
    let after = fs::symlink_metadata(&path).expect("metadata after");
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(
        before.modified().expect("mtime"),
        after.modified().expect("mtime")
    );
    let entries_after = fs::read_dir(directory.path())
        .expect("entries after")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries_before, entries_after);
    assert!(!directory.path().join(".quarantine").exists());
}

#[test]
fn unsafe_files_and_directories_are_refused_without_repair() {
    let (scratch, directory) = fixture();

    let outside = scratch.path().join("outside.json");
    write_private(&outside, br#"{"revision":0,"value":"outside"}"#);
    symlink(&outside, directory.path().join("linked.json")).expect("file symlink");
    assert!(matches!(
        read_bounded(&directory, "linked.json", FILE_LIMIT),
        Err(StorageError::UnsafeStorage { .. })
    ));

    fs::create_dir(directory.path().join("directory.json")).expect("non-file directory");
    fs::set_permissions(
        directory.path().join("directory.json"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("directory mode");
    assert!(matches!(
        read_bounded(&directory, "directory.json", FILE_LIMIT),
        Err(StorageError::UnsafeStorage { .. })
    ));

    let hard = directory.path().join("hard.json");
    write_private(&hard, br#"{"revision":0,"value":"hard"}"#);
    fs::hard_link(&hard, scratch.path().join("hard-link-copy")).expect("hard link");
    assert!(matches!(
        read_bounded(&directory, "hard.json", FILE_LIMIT),
        Err(StorageError::UnsafeStorage { .. })
    ));

    let broad = directory.path().join("broad.json");
    write_private(&broad, br#"{"revision":0,"value":"broad"}"#);
    fs::set_permissions(&broad, fs::Permissions::from_mode(0o644)).expect("broad file mode");
    assert!(matches!(
        read_bounded(&directory, "broad.json", FILE_LIMIT),
        Err(StorageError::UnsafeStorage { .. })
    ));
    assert_eq!(
        mode(&broad),
        0o644,
        "read refusal must not repair permissions"
    );

    let broad_dir = scratch.path().join("broad-dir");
    fs::create_dir(&broad_dir).expect("broad directory");
    fs::set_permissions(&broad_dir, fs::Permissions::from_mode(0o755))
        .expect("broad directory mode");
    assert!(matches!(
        PrivateDirectory::open(&broad_dir),
        Err(StorageError::UnsafeStorage { .. })
    ));
    assert_eq!(mode(&broad_dir), 0o755);

    let directory_link = scratch.path().join("directory-link");
    symlink(directory.path(), &directory_link).expect("directory symlink");
    assert!(matches!(
        PrivateDirectory::open(&directory_link),
        Err(StorageError::UnsafeStorage { .. })
    ));
}

#[test]
fn created_state_lock_and_quarantine_are_owner_private() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");
    let record = Record {
        revision: 0,
        value: "private".to_owned(),
    };
    let loaded = atomic_write_json(
        &directory,
        &lock,
        "state.json",
        &record,
        &TargetSnapshot::Missing,
        JsonLimits::default(),
    )
    .expect("write state");

    assert_eq!(mode(directory.path()), 0o700);
    assert_eq!(mode(lock.path()), 0o600);
    assert_eq!(mode(&directory.path().join("state.json")), 0o600);

    let artifact = quarantine_file(
        &directory,
        &lock,
        "state.json",
        loaded.snapshot(),
        QuarantineReason::Corrupt,
        FILE_LIMIT,
    )
    .expect("quarantine state");
    assert_eq!(mode(&directory.path().join(".quarantine")), 0o700);
    assert_eq!(
        mode(&directory.path().join(".quarantine").join(artifact.name())),
        0o600
    );
}

#[test]
fn stale_revision_conflicts_without_rewriting_the_winner() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");
    let initial = Record {
        revision: 0,
        value: "initial".to_owned(),
    };
    let stale = atomic_write_json(
        &directory,
        &lock,
        "state.json",
        &initial,
        &TargetSnapshot::Missing,
        JsonLimits::default(),
    )
    .expect("initial state");
    let winner = Record {
        revision: 1,
        value: "winner".to_owned(),
    };
    compare_and_set_json(
        &directory,
        &lock,
        "state.json",
        &stale,
        0,
        &winner,
        JsonLimits::default(),
    )
    .expect("winning CAS");
    let path = directory.path().join("state.json");
    let winning_bytes = fs::read(&path).expect("winning bytes");
    let winning_identity = fs::symlink_metadata(&path).expect("winning metadata").ino();

    let loser = Record {
        revision: 1,
        value: "loser".to_owned(),
    };
    let error = compare_and_set_json(
        &directory,
        &lock,
        "state.json",
        &stale,
        0,
        &loser,
        JsonLimits::default(),
    )
    .expect_err("stale CAS must fail");
    assert!(matches!(
        error,
        StorageError::CasConflict {
            expected: 0,
            current: 1
        }
    ));
    assert_eq!(
        fs::read(&path).expect("bytes after conflict"),
        winning_bytes
    );
    assert_eq!(
        fs::symlink_metadata(&path)
            .expect("metadata after conflict")
            .ino(),
        winning_identity
    );
}

#[test]
fn same_revision_external_drift_is_not_silently_overwritten() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");
    let initial = Record {
        revision: 0,
        value: "initial".to_owned(),
    };
    let loaded = atomic_write_json(
        &directory,
        &lock,
        "state.json",
        &initial,
        &TargetSnapshot::Missing,
        JsonLimits::default(),
    )
    .expect("initial state");
    let path = directory.path().join("state.json");
    let drift = br#"{"revision":0,"value":"external"}"#;
    fs::write(&path, drift).expect("external write");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("restore exact mode");

    let replacement = Record {
        revision: 1,
        value: "ours".to_owned(),
    };
    assert!(matches!(
        compare_and_set_json(
            &directory,
            &lock,
            "state.json",
            &loaded,
            0,
            &replacement,
            JsonLimits::default(),
        ),
        Err(StorageError::ExternalDrift { .. })
    ));
    assert_eq!(fs::read(path).expect("drift bytes preserved"), drift);
}

#[test]
fn bounded_and_duplicate_json_fail_closed_without_byte_loss() {
    let (_scratch, directory) = fixture();
    let oversized = directory.path().join("oversized.json");
    write_private(&oversized, b"12345");
    assert!(matches!(
        read_bounded(&directory, "oversized.json", 4),
        Err(StorageError::BoundsExceeded { .. })
    ));

    let duplicate = directory.path().join("duplicate.json");
    let bytes = br#"{"revision":0,"value":"first","val\u0075e":"second"}"#;
    write_private(&duplicate, bytes);
    assert!(matches!(
        read_json::<Record>(&directory, "duplicate.json", JsonLimits::default()),
        Err(StorageError::Corrupt { .. })
    ));
    assert_eq!(
        fs::read(duplicate).expect("duplicate bytes preserved"),
        bytes
    );
}

#[test]
fn advisory_lock_allows_exactly_one_writer_and_keeps_a_stable_inode() {
    let (_scratch, directory) = fixture();
    let first = WriterLock::acquire(&directory, "writer.lock").expect("first writer");
    let identity = fs::symlink_metadata(first.path())
        .expect("lock metadata")
        .ino();
    assert!(matches!(
        WriterLock::acquire(&directory, "writer.lock"),
        Err(StorageError::LockHeld { .. })
    ));
    first.release(&directory).expect("release first writer");

    let second = WriterLock::acquire(&directory, "writer.lock").expect("second writer");
    assert_eq!(
        fs::symlink_metadata(second.path())
            .expect("second lock metadata")
            .ino(),
        identity,
        "release must not unlink and split the lock inode"
    );
}

#[test]
fn parent_lock_can_protect_a_record_only_child_directory() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = PrivateDirectory::open_or_create(temp.path().join("store")).expect("root");
    let records = root
        .open_or_create_child("records")
        .expect("records directory");
    let lock =
        WriterLock::acquire_for(&root, "writer.lock", &records).expect("parent-scoped writer lock");

    atomic_write_json(
        &records,
        &lock,
        "record.json",
        &Record {
            revision: 0,
            value: "legacy-compatible".to_owned(),
        },
        &TargetSnapshot::Missing,
        JsonLimits::default(),
    )
    .expect("write protected child record");

    assert_eq!(
        fs::read_dir(records.path())
            .expect("record entries")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>(),
        ["record.json"]
    );
    assert!(root.path().join("writer.lock").is_file());
    lock.release(&records).expect("release scoped lease");
}

#[test]
fn replaced_lock_inode_revokes_the_writer_lease() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");
    let displaced = directory.path().join("displaced.lock");
    fs::rename(lock.path(), &displaced).expect("displace lock inode");
    write_private(lock.path(), b"");

    assert!(matches!(
        lock.assert_held_for(&directory),
        Err(StorageError::LockLost { .. })
    ));
    assert!(matches!(
        atomic_write_json(
            &directory,
            &lock,
            "state.json",
            &Record {
                revision: 0,
                value: "must not write".to_owned(),
            },
            &TargetSnapshot::Missing,
            JsonLimits::default(),
        ),
        Err(StorageError::LockLost { .. })
    ));
    assert!(!directory.path().join("state.json").exists());
}

/// Names in `.quarantine`, sorted, read straight from the directory.
fn quarantined_names(directory: &PrivateDirectory) -> Vec<String> {
    let mut names = fs::read_dir(directory.path().join(".quarantine"))
        .expect("quarantine directory")
        .map(|entry| {
            entry
                .expect("quarantine entry")
                .file_name()
                .into_string()
                .expect("utf-8 name")
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn artifact_names(artifacts: &[QuarantinedArtifact]) -> Vec<String> {
    let mut names = artifacts
        .iter()
        .map(|artifact| artifact.name().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn stale_temp_recovery_preserves_old_commit_and_keeps_the_temp_bytes() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");
    let initial = Record {
        revision: 0,
        value: "committed".to_owned(),
    };
    atomic_write_json(
        &directory,
        &lock,
        "state.json",
        &initial,
        &TargetSnapshot::Missing,
        JsonLimits::default(),
    )
    .expect("initial state");

    let stale_name = format!(
        ".interface-atomic-{}-{}.tmp",
        atomic_temp_digest_prefix("state.json"),
        Uuid::new_v4()
    );
    assert!(is_atomic_temp_name(&stale_name));
    let stale_bytes = br#"{"revision":1,"value":"uncommitted temp"}"#;
    write_private(&directory.path().join(&stale_name), stale_bytes);

    let artifacts = quarantine_stale_atomic_writes_for(&directory, &lock, "state.json", FILE_LIMIT)
        .expect("stale temp quarantine");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(
        read_json::<Record>(&directory, "state.json", JsonLimits::default())
            .expect("read committed")
            .expect("committed present")
            .value(),
        &initial
    );
    assert!(!directory.path().join(&stale_name).exists());
    assert_eq!(quarantined_names(&directory), artifact_names(&artifacts));
    let quarantined = directory
        .path()
        .join(".quarantine")
        .join(artifacts[0].name());
    assert_eq!(
        fs::read(&quarantined).expect("quarantined bytes"),
        stale_bytes
    );
    assert_eq!(mode(&quarantined), 0o600);
}

#[test]
fn target_scoped_stale_temp_quarantine_leaves_other_stores_alone() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");

    let registry_temp = format!(
        ".interface-atomic-{}-{}.tmp",
        atomic_temp_digest_prefix("registry.json"),
        Uuid::new_v4()
    );
    let appearance_temp = format!(
        ".interface-atomic-{}-{}.tmp",
        atomic_temp_digest_prefix("appearance.json"),
        Uuid::new_v4()
    );
    assert!(is_atomic_temp_name(&registry_temp));
    assert!(is_atomic_temp_name_for(&registry_temp, "registry.json"));
    assert!(!is_atomic_temp_name_for(&appearance_temp, "registry.json"));
    assert!(is_atomic_temp_name_for(&appearance_temp, "appearance.json"));

    write_private(&directory.path().join(&registry_temp), b"registry-temp");
    write_private(&directory.path().join(&appearance_temp), b"appearance-temp");

    let artifacts =
        quarantine_stale_atomic_writes_for(&directory, &lock, "registry.json", FILE_LIMIT)
            .expect("target-scoped quarantine");
    assert_eq!(artifacts.len(), 1);
    assert!(!directory.path().join(&registry_temp).exists());
    assert!(directory.path().join(&appearance_temp).exists());
    assert_eq!(
        fs::read(directory.path().join(&appearance_temp)).expect("foreign bytes"),
        b"appearance-temp"
    );
    assert_eq!(quarantined_names(&directory), artifact_names(&artifacts));
}

#[test]
fn corrupt_state_moves_only_after_explicit_quarantine() {
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").expect("writer lock");
    let path = directory.path().join("state.json");
    let corrupt = b"{not-json";
    write_private(&path, corrupt);

    assert!(matches!(
        read_json::<Record>(&directory, "state.json", JsonLimits::default()),
        Err(StorageError::Corrupt { .. })
    ));
    assert_eq!(fs::read(&path).expect("corrupt bytes after read"), corrupt);
    assert!(!directory.path().join(".quarantine").exists());

    let snapshot = read_bounded(&directory, "state.json", FILE_LIMIT)
        .expect("bounded corrupt read")
        .expect("corrupt state present")
        .snapshot()
        .clone();
    let artifact = quarantine_file(
        &directory,
        &lock,
        "state.json",
        &snapshot,
        QuarantineReason::Corrupt,
        FILE_LIMIT,
    )
    .expect("explicit quarantine");
    assert_eq!(
        snapshot_target(&directory, "state.json", FILE_LIMIT).expect("missing target"),
        TargetSnapshot::Missing
    );
    assert_eq!(
        fs::read(directory.path().join(".quarantine").join(artifact.name()))
            .expect("quarantined corrupt bytes"),
        corrupt
    );
}

#[test]
fn directory_device_inode_revalidation_rejects_path_replacement() {
    let (scratch, directory) = fixture();
    let moved = scratch.path().join("moved-store");
    fs::rename(directory.path(), &moved).expect("move original store");
    fs::create_dir(directory.path()).expect("replacement store");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("replacement mode");
    assert!(matches!(
        directory.assert_unchanged(),
        Err(StorageError::UnsafeStorage { .. })
    ));
}

#[test]
fn prepared_json_keeps_strict_validation_and_rechecks_the_write_target() {
    use interface_storage::prepare_json;
    let (_scratch, directory) = fixture();
    let lock = WriterLock::acquire(&directory, "writer.lock").unwrap();
    let path = directory.path().join("prepared.json");
    let value = Record {
        revision: 1,
        value: "prepared once".into(),
    };
    let limits = JsonLimits::default();
    let prepared = prepare_json(&value, limits, &path).unwrap();
    assert!(!path.exists());
    let written = prepared
        .write(&directory, &lock, "prepared.json", &TargetSnapshot::Missing)
        .unwrap();
    assert_eq!(written.value(), &value);
    assert_eq!(
        fs::read(&path).unwrap(),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()).as_bytes()
    );
    let prepared = prepare_json(&value, limits, &path).unwrap();
    // Preparation is not authority: a later target replacement is still refused.
    fs::write(&path, b"{}\n").unwrap();
    assert!(
        prepared
            .write(
                &directory,
                &lock,
                "prepared.json",
                &TargetSnapshot::File(written.snapshot().clone())
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), b"{}\n");
    assert!(matches!(
        prepare_json(&value, JsonLimits::new(8, 32, 100).unwrap(), &path),
        Err(StorageError::BoundsExceeded { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), b"{}\n");
}

#[test]
fn an_unpublished_child_is_new_private_and_never_reuses_an_entry() {
    let (_scratch, directory) = fixture();
    let staging = directory
        .create_unpublished_child(".staging.tmp")
        .expect("new staging child");
    let metadata = fs::symlink_metadata(staging.path()).expect("staging metadata");
    assert!(metadata.is_dir());
    assert_eq!(metadata.mode() & 0o7777, 0o700);
    assert!(staging.assert_unchanged().is_ok());

    // A staging tree is only ever freshly created: an existing directory,
    // file or symlink under the name is refused, never adopted.
    assert!(directory.create_unpublished_child(".staging.tmp").is_err());
    write_private(&directory.path().join("file"), b"x");
    assert!(directory.create_unpublished_child("file").is_err());
    symlink(staging.path(), directory.path().join("link")).expect("symlink");
    assert!(directory.create_unpublished_child("link").is_err());
    assert!(directory.create_unpublished_child("../escape").is_err());
    assert!(!directory.path().parent().unwrap().join("escape").exists());
}
