use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::*;

fn private_root() -> (TempDir, PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("checkpoints");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (temp, root)
}

fn draft(root: &Path, id: &str) -> CheckpointDraft {
    CheckpointDraft {
        id: id.into(),
        kind: CheckpointKind::Review,
        created_at: "2026-08-27T12:00:00.000Z".into(),
        repository_root: root.to_path_buf(),
        repository_head: Some("abc".into()),
        source_checkpoint_id: None,
        files: vec![CheckpointInputFile {
            path: "src/lib.rs".into(),
            content: b"fn main() {}\n".to_vec(),
            mode: 0o644,
        }],
        omitted: vec![CheckpointOmission {
            path: "gone.rs".into(),
            reason: CheckpointOmissionReason::Deleted,
        }],
    }
}

#[test]
fn committed_checkpoint_has_golden_schema_private_modes_and_exact_reads() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    fs::create_dir(&repository).unwrap();
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let manifest = store
        .create(store.revision(), draft(&repository, "2026-08-27-id"))
        .unwrap();
    assert_eq!(manifest.files.len(), 1);
    assert_eq!(
        store.read_blob(&manifest, &manifest.files[0]).unwrap(),
        b"fn main() {}\n"
    );
    let checkpoint = root.join(&manifest.id);
    let manifest_path = checkpoint.join("manifest.json");
    let before = fs::read(&manifest_path).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&before).unwrap(),
        json!({
            "version":1,
            "id":"2026-08-27-id",
            "kind":"review",
            "createdAt":"2026-08-27T12:00:00.000Z",
            "repositoryRoot":repository,
            "repositoryHead":"abc",
            "files":[{
                "path":"src/lib.rs", "blob":"000001", "bytes":13,
                "sha256":"536e506bb90914c243a12b397b9a998f85ae2cbd9ba02dfd03a9e155ca5ca0f4",
                "mode":420
            }],
            "omitted":[{"path":"gone.rs","reason":"deleted"}],
            "totalBytes":13
        })
    );
    assert_eq!(fs::metadata(&checkpoint).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&manifest_path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(checkpoint.join("blobs/000001"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
    drop(store);
    let modified = fs::metadata(&manifest_path).unwrap().modified().unwrap();
    thread::sleep(Duration::from_millis(5));
    let inspected =
        inspect_checkpoint_read_only(&root, "2026-08-27-id", CheckpointLimits::default()).unwrap();
    let listed =
        list_checkpoints_read_only(&root, CheckpointLimits::default(), Some(&repository)).unwrap();
    assert_eq!(inspected, manifest);
    assert_eq!(listed.len(), 1);
    assert_eq!(before, fs::read(&manifest_path).unwrap());
    assert_eq!(
        modified,
        fs::metadata(&manifest_path).unwrap().modified().unwrap()
    );
}

#[test]
fn a_blob_reader_refuses_an_entry_outside_its_manifest() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    fs::create_dir(&repository).unwrap();
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let mut wide = draft(&repository, "2026-08-27-wide");
    wide.files.push(CheckpointInputFile {
        path: "src/other.rs".into(),
        content: b"fn other() {}\n".to_vec(),
        mode: 0o644,
    });
    let wide = store.create(store.revision(), wide).unwrap();
    let narrow = store
        .create(store.revision(), draft(&repository, "2026-08-27-narrow"))
        .unwrap();
    drop(store);

    let reader =
        CheckpointBlobReader::open_read_only(&root, &narrow, CheckpointLimits::default()).unwrap();
    assert_eq!(reader.read(&narrow.files[0]).unwrap(), b"fn main() {}\n");
    assert_eq!(wide.files.len(), 2);
    let outside = reader.read(&wide.files[1]).unwrap_err();
    assert!(
        matches!(&outside, RegistryError::Invalid(message)
            if message == "blob entry does not belong to this manifest"),
        "unexpected error: {outside:?}"
    );
}

#[test]
fn a_same_length_tampered_blob_fails_its_recorded_digest() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    fs::create_dir(&repository).unwrap();
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let manifest = store
        .create(store.revision(), draft(&repository, "2026-08-27-tampered"))
        .unwrap();
    let blob = root.join(&manifest.id).join("blobs/000001");
    fs::write(&blob, b"fn mian() {}\n").unwrap();
    fs::set_permissions(&blob, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        store.read_blob(&manifest, &manifest.files[0]),
        Err(RegistryError::Checkpoint { reason, .. }) if reason == "checkpoint-corrupt"
    ));

    for content in [&b""[..], b"fn main() {}\n", &[0xff; 70]] {
        let digest: [u8; 32] = Sha256::digest(content).into();
        let hex = format!("{:x}", Sha256::digest(content));
        assert!(is_lower_hex_of(&hex, &digest));
        assert!(!is_lower_hex_of(&hex.to_uppercase(), &digest));
        assert!(!is_lower_hex_of(&hex[..63], &digest));
        assert!(!is_lower_hex_of(&format!("{hex}0"), &digest));
        let mut flipped = digest;
        flipped[31] ^= 1;
        assert!(!is_lower_hex_of(&hex, &flipped));
    }
}

#[test]
fn unsafe_inputs_are_omitted_without_persisting_credentials_or_binary_bytes() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let mut draft = draft(&repository, "safe-id");
    draft.files.extend([
        CheckpointInputFile {
            path: ".env.local".into(),
            content: b"NEVER=PERSIST".to_vec(),
            mode: 0o600,
        },
        CheckpointInputFile {
            path: "../escape".into(),
            content: b"x".to_vec(),
            mode: 0o644,
        },
        CheckpointInputFile {
            path: "asset.png".into(),
            content: vec![0, 1],
            mode: 0o644,
        },
    ]);
    let manifest = store.create(store.revision(), draft).unwrap();
    assert_eq!(manifest.files.len(), 1);
    assert!(
        manifest
            .omitted
            .iter()
            .any(|item| item.reason == CheckpointOmissionReason::Credential)
    );
    assert!(
        manifest
            .omitted
            .iter()
            .any(|item| item.reason == CheckpointOmissionReason::UnsafePath)
    );
    assert!(
        manifest
            .omitted
            .iter()
            .any(|item| item.reason == CheckpointOmissionReason::Binary)
    );
    let all_bytes = fs::read(root.join("safe-id/manifest.json")).unwrap();
    assert!(
        !all_bytes
            .windows(13)
            .any(|window| window == b"NEVER=PERSIST")
    );
}

#[test]
fn empty_turn_checkpoint_is_reported_without_becoming_durable() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let revision = store.revision();
    let mut empty = draft(&repository, "empty-turn");
    empty.kind = CheckpointKind::Turn;
    empty.files[0].path = ".env".into();
    let manifest = store.create(revision, empty).unwrap();
    assert!(manifest.files.is_empty());
    assert_eq!(
        manifest.omitted[1].reason,
        CheckpointOmissionReason::Credential
    );
    assert_eq!(store.revision(), revision);
    assert!(!root.join("empty-turn").exists());
    assert!(!fs::read_dir(&root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("empty-turn")
    }));
}

#[test]
fn linked_entries_hardlinks_tampering_and_stale_revisions_fail_closed() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    assert!(store.create(0, draft(&repository, "stale")).is_err());
    let manifest = store
        .create(store.revision(), draft(&repository, "valid-id"))
        .unwrap();
    drop(store);

    let outside = root.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.join("linked-id")).unwrap();
    let listed = list_checkpoints_read_only(&root, CheckpointLimits::default(), None).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(inspect_checkpoint_read_only(&root, "linked-id", CheckpointLimits::default()).is_err());

    let manifest_path = root.join("valid-id/manifest.json");
    let extra = root.parent().unwrap().join("manifest-link");
    fs::hard_link(&manifest_path, &extra).unwrap();
    assert_eq!(fs::metadata(&manifest_path).unwrap().nlink(), 2);
    assert!(
        inspect_checkpoint_read_only(&root, &manifest.id, CheckpointLimits::default()).is_err()
    );
}

#[test]
fn turn_checkpoints_below_the_cheap_gate_are_not_pruned() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    fs::create_dir(&repository).unwrap();
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    for index in 0..(TURN_CHECKPOINT_RETENTION + 5) {
        let mut item = draft(&repository, &format!("turn-{index:03}"));
        item.kind = CheckpointKind::Turn;
        item.created_at = format!("2026-08-14T12:00:{index:02}.000Z");
        store.create(store.revision(), item).unwrap();
    }
    let listed = store.list(Some(&repository)).unwrap();
    assert_eq!(listed.len(), TURN_CHECKPOINT_RETENTION + 5);
    assert!(listed.iter().any(|item| item.id == "turn-000"));
}

fn add_turn(store: &mut CheckpointStore, repository: &Path, index: usize) {
    let mut item = draft(repository, &format!("turn-{index:03}"));
    item.kind = CheckpointKind::Turn;
    item.created_at = format!("2026-08-14T12:{:02}:{:02}.000Z", index / 60, index % 60);
    let revision = store.revision();
    store.create(revision, item).unwrap();
}

#[test]
fn turn_checkpoints_past_the_gate_are_cut_back_to_retention() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let other = root.parent().unwrap().join("other");
    fs::create_dir(&repository).unwrap();
    fs::create_dir(&other).unwrap();
    for index in 0..=(TURN_CHECKPOINT_RETENTION * 2) {
        fs::create_dir(root.join(format!("pad-{index:03}"))).unwrap();
    }
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let mut kept_review = draft(&repository, "kept-review");
    kept_review.created_at = "2026-08-14T11:00:00.000Z".into();
    store.create(store.revision(), kept_review).unwrap();
    let mut kept_other = draft(&other, "kept-other");
    kept_other.kind = CheckpointKind::Turn;
    kept_other.created_at = "2026-08-14T11:01:00.000Z".into();
    store.create(store.revision(), kept_other).unwrap();
    let trigger = TURN_CHECKPOINT_RETENTION * 2 - 1;
    for index in 0..trigger {
        add_turn(&mut store, &repository, index);
    }
    // A scan that frees nothing records the total it left behind, so the
    // gate waits for another retention's growth instead of re-scanning the
    // whole store on every send. This root sits above its ceiling until then.
    assert_eq!(
        store
            .list(Some(&repository))
            .unwrap()
            .iter()
            .filter(|item| item.kind == CheckpointKind::Turn)
            .count(),
        trigger
    );

    add_turn(&mut store, &repository, trigger);

    let listed = store.list(Some(&repository)).unwrap();
    let turns = listed
        .iter()
        .filter(|item| item.kind == CheckpointKind::Turn)
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(turns.len(), TURN_CHECKPOINT_RETENTION);
    assert_eq!(turns[0], format!("turn-{trigger:03}"));
    assert!(!turns.contains(&"turn-000"));
    assert!(!turns.contains(&"turn-001"));
    assert!(listed.iter().any(|item| item.id == "kept-review"));
    let other_listed = store.list(Some(&other)).unwrap();
    assert_eq!(other_listed.len(), 1);
    assert_eq!(other_listed[0].id, "kept-other");
}

#[test]
fn a_root_whose_display_cap_is_full_still_prunes_its_turns() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    fs::create_dir(&repository).unwrap();
    // Under the cheap gate while the turns accumulate, so the only scan is
    // the one the final capture triggers.
    for index in 0..(TURN_CHECKPOINT_RETENTION - 2) {
        fs::create_dir(root.join(format!("pad-{index:03}"))).unwrap();
    }
    let limits = CheckpointLimits {
        max_listed_checkpoints: 1,
        ..CheckpointLimits::default()
    };
    let mut store = CheckpointStore::open(&root, limits).unwrap();
    for index in 0..=TURN_CHECKPOINT_RETENTION {
        add_turn(&mut store, &repository, index);
    }
    // A newer non-turn checkpoint fills the display cap: the listing `list`
    // returns stops before it reaches a single turn.
    let mut newest = draft(&repository, "zz-review");
    newest.created_at = "2026-08-14T13:00:00.000Z".into();
    store.create(store.revision(), newest).unwrap();
    assert_eq!(store.list(Some(&repository)).unwrap().len(), 1);

    add_turn(&mut store, &repository, TURN_CHECKPOINT_RETENTION + 1);
    drop(store);

    let listed =
        list_checkpoints_read_only(&root, CheckpointLimits::default(), Some(&repository)).unwrap();
    assert_eq!(
        listed
            .iter()
            .filter(|item| item.kind == CheckpointKind::Turn)
            .count(),
        TURN_CHECKPOINT_RETENTION
    );
    assert!(listed.iter().any(|item| item.id == "zz-review"));
}

#[test]
fn manifest_traversal_and_credential_paths_are_rejected() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let manifest = store
        .create(store.revision(), draft(&repository, "tamper-id"))
        .unwrap();
    drop(store);
    let path = root.join("tamper-id/manifest.json");
    let mut value = serde_json::to_value(&manifest).unwrap();
    value["files"][0]["path"] = Value::String("../escape".into());
    fs::write(&path, json_line(&value).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(inspect_checkpoint_read_only(&root, "tamper-id", CheckpointLimits::default()).is_err());
}

/// Performs each durability step for real, recording it and whether the
/// checkpoint was already published when it ran; the step at `fail_at`
/// fails instead of running.
struct RecordingDurability {
    root: PathBuf,
    published: PathBuf,
    fail_at: Option<usize>,
    steps: Vec<(String, bool)>,
}

impl RecordingDurability {
    fn new(root: &Path, id: &str, fail_at: Option<usize>) -> Self {
        Self {
            root: root.to_path_buf(),
            published: root.join(id),
            fail_at,
            steps: Vec::new(),
        }
    }

    fn step(&mut self, kind: &str, path: &Path) -> Result<()> {
        let mut parts = path
            .strip_prefix(&self.root)
            .unwrap()
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if parts.first().is_some_and(|first| first.starts_with('.')) {
            parts[0] = "staging".into();
        }
        let label = if parts.is_empty() {
            "store".to_owned()
        } else {
            parts.join("/")
        };
        let failing = self.fail_at == Some(self.steps.len());
        self.steps
            .push((format!("{kind} {label}"), self.published.exists()));
        if failing {
            return Err(RegistryError::io(
                "fsync (injected failure)",
                path,
                std::io::Error::other("injected"),
            ));
        }
        Ok(())
    }

    fn labels(&self) -> Vec<(&str, bool)> {
        self.steps
            .iter()
            .map(|(label, published)| (label.as_str(), *published))
            .collect()
    }
}

impl CheckpointDurability for RecordingDurability {
    fn hand_off(&mut self, operation: &'static str, fd: BorrowedFd<'_>, path: &Path) -> Result<()> {
        self.step("hand-off", path)?;
        DeviceDurability.hand_off(operation, fd, path)
    }

    fn flush(&mut self, directory: &PrivateDirectory) -> Result<()> {
        self.step("flush", directory.path())?;
        DeviceDurability.flush(directory)
    }
}

fn root_entries(root: &Path) -> Vec<String> {
    let mut names = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn every_entry_is_handed_off_before_the_one_barrier_that_precedes_publication() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let mut item = draft(&repository, "ordered");
    item.kind = CheckpointKind::Turn;
    item.files.push(CheckpointInputFile {
        path: "src/main.rs".into(),
        content: b"fn run() {}\n".to_vec(),
        mode: 0o644,
    });
    let mut durability = RecordingDurability::new(&root, "ordered", None);
    let revision = store.revision();
    let manifest = store
        .create_durably(revision, item, &mut durability)
        .unwrap();

    // Plain fsyncs for every blob, the manifest, the journal and the blobs
    // directory; then the single drive-cache barrier, still unpublished; then
    // the rename, and the store sync that makes the publication durable.
    assert_eq!(
        durability.labels(),
        [
            ("hand-off staging/blobs/000001", false),
            ("hand-off staging/blobs/000002", false),
            ("hand-off staging/manifest.json", false),
            ("hand-off staging/journal.json", false),
            ("hand-off staging/blobs", false),
            ("flush staging", false),
            ("flush store", true),
        ]
    );
    assert_eq!(
        store.read_blob(&manifest, &manifest.files[1]).unwrap(),
        b"fn run() {}\n"
    );
    assert_eq!(store.revision(), revision + 1);
}

#[test]
fn a_failed_blob_sync_never_publishes_the_manifest() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let revision = store.revision();
    let mut durability = RecordingDurability::new(&root, "unsynced", Some(0));
    let error = store
        .create_durably(revision, draft(&repository, "unsynced"), &mut durability)
        .unwrap_err();

    assert!(error.to_string().contains("injected"), "{error}");
    assert_eq!(
        durability.labels(),
        [("hand-off staging/blobs/000001", false)]
    );
    assert_eq!(store.revision(), revision);
    // Neither the checkpoint nor its staging tree (with the manifest the
    // failed blob would have been published under) is left behind.
    assert_eq!(root_entries(&root), [CHECKPOINT_LOCK]);
    assert!(inspect_checkpoint_read_only(&root, "unsynced", CheckpointLimits::default()).is_err());
    assert!(
        list_checkpoints_read_only(&root, CheckpointLimits::default(), None)
            .unwrap()
            .is_empty()
    );
    // The store is not wedged: the same checkpoint then commits.
    store
        .create(revision, draft(&repository, "unsynced"))
        .unwrap();
    assert_eq!(root_entries(&root), [CHECKPOINT_LOCK, "unsynced"]);
}

#[test]
fn only_a_failure_after_the_rename_can_leave_a_published_checkpoint() {
    let (_temp, root) = private_root();
    let repository = root.parent().unwrap().join("repo");
    let mut store = CheckpointStore::open(&root, CheckpointLimits::default()).unwrap();
    let revision = store.revision();
    // One blob: its hand-off, the manifest, the journal, the blobs directory
    // and the staging barrier all run before the rename.
    for fail_at in 0..5 {
        let mut durability = RecordingDurability::new(&root, "unpublished", Some(fail_at));
        let error = store
            .create_durably(revision, draft(&repository, "unpublished"), &mut durability)
            .unwrap_err();
        assert!(error.to_string().contains("injected"), "{fail_at}: {error}");
        assert_eq!(durability.steps.len(), fail_at + 1);
        assert!(durability.steps.iter().all(|(_, published)| !published));
        assert_eq!(root_entries(&root), [CHECKPOINT_LOCK], "{fail_at}");
        assert_eq!(store.revision(), revision);
    }

    // The store sync after the rename is the only step that can fail with
    // the checkpoint already in place, and it reports that as uncertain.
    let mut durability = RecordingDurability::new(&root, "uncertain", Some(5));
    let error = store
        .create_durably(revision, draft(&repository, "uncertain"), &mut durability)
        .unwrap_err();
    assert!(
        matches!(
            error,
            RegistryError::Storage(StorageError::WriteUncertain { .. })
        ),
        "{error}"
    );
    assert_eq!(durability.labels().last(), Some(&("flush store", true)));
    assert_eq!(root_entries(&root), [CHECKPOINT_LOCK, "uncertain"]);
}
