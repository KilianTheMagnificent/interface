use std::{ffi::OsStr, fs, path::Path};

use interface_collaboration::journal::{
    CollaborationJournal, JournalErrorCode, journal_record_path,
};
use interface_collaboration::state::{
    CollaborationEngineId, CollaborationIntentInput, CollaborationMode, CollaborationOrigin,
    CollaborationPolicyOverrides, CollaborationState, CollaborationTransitionPatch,
    CollaborationTransitionRequest, CreateCollaborationRecordOptions, create_collaboration_record,
    normalize_collaboration_intent, transition_collaboration_record,
};
use sha2::Digest;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};

const AT: &str = "2026-08-27T12:00:00.000Z";

fn record(id: &str, objective: &str) -> interface_collaboration::state::CollaborationRecordV1 {
    let intent = normalize_collaboration_intent(
        &CollaborationIntentInput {
            version: Some(1),
            mode: CollaborationMode::Consult,
            source_session_key: "claude#source-1".to_owned(),
            target_engine: CollaborationEngineId::Codex,
            objective: objective.to_owned(),
            client_request_id: "request-1".to_owned(),
            origin: CollaborationOrigin::Human,
        },
        None,
    )
    .expect("intent");
    create_collaboration_record(CreateCollaborationRecordOptions {
        collaboration_id: id.to_owned(),
        intent,
        created_at: AT.to_owned(),
        depth: 0,
        limits: CollaborationPolicyOverrides::default(),
    })
    .expect("record")
}

fn cancelled(
    current: &interface_collaboration::state::CollaborationRecordV1,
) -> interface_collaboration::state::CollaborationRecordV1 {
    transition_collaboration_record(
        current,
        CollaborationTransitionRequest {
            expected_revision: current.revision,
            expected_state: current.state,
            to: CollaborationState::Cancelled,
            maintenance_effect: None,
            at: AT.to_owned(),
            event_id: Some("event-cancel".to_owned()),
            patch: CollaborationTransitionPatch::default(),
        },
    )
    .expect("transition")
    .record
}

#[test]
fn create_reopen_and_idempotent_replay_preserve_owner_private_state() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    let mut journal = CollaborationJournal::open(&root).expect("open");
    let first = journal
        .create(record("collaboration-1", "inspect code"))
        .expect("create");
    assert!(!first.replayed);
    {
        let listed: Vec<_> = journal.records().expect("records").collect();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].collaboration_id, "collaboration-1");
    }
    let replay = journal
        .create(record("collaboration-2", "inspect code"))
        .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.record.collaboration_id, "collaboration-1");

    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(&root).expect("root mode").permissions().mode() & 0o777,
            0o700
        );
        let file = journal_record_path(&root, "collaboration-1");
        assert_eq!(
            fs::metadata(file).expect("file mode").permissions().mode() & 0o777,
            0o600
        );
        assert!(root.join("collaboration.lock").is_file());
        assert!(root.join("quarantine").is_dir());
        let names = fs::read_dir(root.join("records"))
            .expect("records")
            .map(|entry| {
                entry
                    .expect("record entry")
                    .file_name()
                    .into_string()
                    .expect("utf8")
            })
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 1);
        assert_eq!(Path::new(&names[0]).extension(), Some(OsStr::new("json")));
    }
    drop(journal);

    let reopened = CollaborationJournal::open(&root).expect("reopen");
    assert_eq!(reopened.list().expect("list").len(), 1);
    assert_eq!(
        reopened
            .find_by_dedupe_key(&first.record.dedupe_key)
            .expect("dedupe")
            .expect("record")
            .collaboration_id,
        "collaboration-1"
    );
}

#[test]
fn same_request_identity_with_different_normalized_payload_conflicts() {
    let temp = TempDir::new().expect("temp");
    let mut journal = CollaborationJournal::open(temp.path().join("journal")).expect("open");
    journal
        .create(record("collaboration-1", "first objective"))
        .expect("create");
    let error = journal
        .create(record("collaboration-2", "different objective"))
        .expect_err("conflict");
    assert_eq!(error.code, JournalErrorCode::IdempotencyConflict);
    assert_eq!(error.current, Some((0, CollaborationState::Requested)));
}

#[test]
fn compare_and_set_detects_same_revision_external_byte_drift_and_poisoning() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    let mut journal = CollaborationJournal::open(&root).expect("open");
    let initial = journal
        .create(record("collaboration-1", "inspect"))
        .expect("create")
        .record;
    let path = journal_record_path(&root, &initial.collaboration_id);
    let mut bytes = fs::read(&path).expect("read");
    bytes.extend_from_slice(b" \n");
    fs::write(&path, bytes).expect("external drift");

    let error = journal
        .compare_and_set(
            &initial.collaboration_id,
            0,
            CollaborationState::Requested,
            cancelled(&initial),
        )
        .expect_err("drift");
    assert_eq!(error.code, JournalErrorCode::StateCorrupt);
    assert_eq!(
        journal.list().expect_err("poisoned").code,
        JournalErrorCode::JournalPoisoned
    );
}

#[test]
fn corrupt_duplicate_member_is_preserved_and_refused_on_reopen() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    let mut journal = CollaborationJournal::open(&root).expect("open");
    let initial = journal
        .create(record("collaboration-1", "inspect"))
        .expect("create")
        .record;
    drop(journal);
    let path = journal_record_path(&root, &initial.collaboration_id);
    let original = fs::read_to_string(&path).expect("read");
    let corrupt = original.replacen('{', "{\n  \"version\": 1,", 1);
    fs::write(&path, &corrupt).expect("write corrupt fixture");
    let error = match CollaborationJournal::open(&root) {
        Ok(_) => panic!("duplicate must be refused"),
        Err(error) => error,
    };
    assert_eq!(error.code, JournalErrorCode::StateCorrupt);
    assert_eq!(fs::read_to_string(path).expect("preserved"), corrupt);
}

#[test]
fn unknown_record_field_is_preserved_and_refused_on_reopen() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    let mut journal = CollaborationJournal::open(&root).expect("open");
    let initial = journal
        .create(record("collaboration-1", "inspect"))
        .expect("create")
        .record;
    drop(journal);
    let path = journal_record_path(&root, &initial.collaboration_id);
    let original = fs::read_to_string(&path).expect("read");
    let corrupt = original.replacen(
        '{',
        "{\n  \"originCapability\": \"must-not-be-accepted\",",
        1,
    );
    fs::write(&path, &corrupt).expect("write corrupt fixture");
    let error = match CollaborationJournal::open(&root) {
        Ok(_) => panic!("an unknown member must be refused"),
        Err(error) => error,
    };
    assert_eq!(error.code, JournalErrorCode::StateCorrupt);
    assert!(error.to_string().contains("originCapability"), "{error}");
    assert_eq!(fs::read_to_string(path).expect("preserved"), corrupt);
}

#[test]
fn one_writer_lock_prevents_split_brain() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    let _first = CollaborationJournal::open(&root).expect("first");
    let error = match CollaborationJournal::open(&root) {
        Ok(_) => panic!("second writer must fail"),
        Err(error) => error,
    };
    assert_eq!(error.code, JournalErrorCode::WriterLeaseLost);
}

#[test]
fn stale_atomic_temp_is_quarantined_before_authoritative_load() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    drop(CollaborationJournal::open(&root).expect("open"));
    let records = root.join("records");
    let temp_name = format!(
        ".interface-atomic-0123456789abcdef-{}.tmp",
        uuid::Uuid::new_v4()
    );
    let stale = records.join(&temp_name);
    fs::write(&stale, b"forensic bytes").expect("stale temp");
    #[cfg(unix)]
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o600)).expect("mode");

    let journal = CollaborationJournal::open(&root).expect("reopen");
    assert!(journal.list().expect("list").is_empty());
    assert_eq!(journal.records().expect("records").count(), 0);
    assert!(!stale.exists());
    let artifacts: Vec<_> = fs::read_dir(root.join("quarantine"))
        .expect("quarantine")
        .collect();
    assert_eq!(artifacts.len(), 1);
}

#[test]
fn legacy_journal_temp_is_moved_to_sibling_quarantine_without_byte_changes() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    drop(CollaborationJournal::open(&root).expect("open"));
    let id = "collaboration-ts-temp";
    let digest = format!("{:x}", sha2::Sha256::digest(id.as_bytes()));
    let name = format!(".journal-{digest}-123-{}.tmp", uuid::Uuid::new_v4());
    let source = root.join("records").join(&name);
    let bytes = b"legacy forensic bytes\n";
    fs::write(&source, bytes).expect("legacy temp");
    #[cfg(unix)]
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).expect("file mode");

    drop(CollaborationJournal::open(&root).expect("reopen"));
    assert!(!source.exists());
    let artifacts = fs::read_dir(root.join("quarantine"))
        .expect("quarantine")
        .map(|entry| entry.expect("artifact").path())
        .collect::<Vec<_>>();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(fs::read(&artifacts[0]).expect("preserved bytes"), bytes);
    assert!(
        artifacts[0]
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with(&name) && value.ends_with(".quarantined"))
    );
}

#[cfg(unix)]
#[test]
fn unsafe_sibling_quarantine_is_part_of_the_fail_closed_trust_boundary() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path().join("journal");
    drop(CollaborationJournal::open(&root).expect("open"));
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).expect("outside");
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).expect("outside mode");
    fs::remove_dir(root.join("quarantine")).expect("remove quarantine");
    symlink(&outside, root.join("quarantine")).expect("quarantine symlink");

    let error = match CollaborationJournal::open(&root) {
        Ok(_) => panic!("unsafe quarantine must fail closed"),
        Err(error) => error,
    };
    assert_eq!(error.code, JournalErrorCode::UnsafeStorage);
}

#[cfg(unix)]
#[test]
fn symlink_journal_root_is_refused() {
    let temp = TempDir::new().expect("temp");
    let real = temp.path().join("real");
    fs::create_dir(&real).expect("real");
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).expect("mode");
    let linked = temp.path().join("linked");
    symlink(&real, &linked).expect("symlink");
    let error = match CollaborationJournal::open(&linked) {
        Ok(_) => panic!("symlink must fail"),
        Err(error) => error,
    };
    assert_eq!(error.code, JournalErrorCode::UnsafeStorage);
}
