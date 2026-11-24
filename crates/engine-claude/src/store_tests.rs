use interface_contracts::engine::SessionOrigin;
use std::{fmt::Write, fs};

use interface_contracts::engine::EngineEventKind;
use tempfile::tempdir;

use super::*;

const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";
const USER_ID: &str = "22222222-2222-4222-8222-222222222222";
const ASSISTANT_ID: &str = "33333333-3333-4333-8333-333333333333";

fn fixture(root: &Path) -> PathBuf {
    let project = root.join(project_dir_name("/work/app"));
    fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION_ID}.jsonl"));
    fs::write(
        &path,
        format!(
            "{{\"type\":\"user\",\"uuid\":\"{USER_ID}\",\"parentUuid\":null,\"sessionId\":\"{SESSION_ID}\",\"cwd\":\"/work/app\",\"message\":{{\"role\":\"user\",\"content\":\"hello\"}}}}\n\
                 {{\"type\":\"assistant\",\"uuid\":\"{ASSISTANT_ID}\",\"parentUuid\":\"{USER_ID}\",\"sessionId\":\"{SESSION_ID}\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"world\"}}]}}}}\n\
                 {{\"type\":\"custom-title\",\"customTitle\":\"Fixture\",\"sessionId\":\"{SESSION_ID}\"}}\n\
                 {{\"type\":\"tag\",\"tag\":\"interface\",\"sessionId\":\"{SESSION_ID}\"}}\n"
        ),
    )
    .unwrap();
    path
}

#[test]
fn session_presence_is_metadata_only_exact_and_fail_closed() {
    let missing_root = tempdir().unwrap().path().join("missing-projects");
    assert!(!session_exists(&missing_root, SESSION_ID).unwrap());

    let root = tempdir().unwrap();
    assert!(!session_exists(root.path(), SESSION_ID).unwrap());
    let transcript = fixture(root.path());
    fs::write(&transcript, b"not JSON and deliberately never parsed\n").unwrap();
    assert!(session_exists(root.path(), SESSION_ID).unwrap());
    assert!(!session_exists(root.path(), USER_ID).unwrap());
    assert_eq!(
        session_exists(root.path(), "not-a-provider-uuid")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );

    let unsafe_path = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    fs::create_dir(&unsafe_path).unwrap();
    assert!(session_exists(root.path(), USER_ID).is_err());
}

#[test]
fn transcript_path_recovers_only_an_exact_safe_canonical_v4() {
    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    assert_eq!(
        session_id_from_transcript_path(root.path(), &transcript).unwrap(),
        SESSION_ID
    );

    let missing = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    assert_eq!(
        session_id_from_transcript_path(root.path(), &missing)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );

    fs::write(&missing, []).unwrap();
    assert!(session_id_from_transcript_path(root.path(), &missing).is_err());
}

#[test]
fn transcript_path_rejects_outside_wrongly_nested_and_invalid_names() {
    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let outside = tempdir().unwrap();
    let outside_transcript = fixture(outside.path());
    assert_eq!(
        session_id_from_transcript_path(root.path(), &outside_transcript)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );

    let directly_nested = root.path().join(format!("{USER_ID}.jsonl"));
    fs::write(&directly_nested, b"provider transcript\n").unwrap();
    assert_eq!(
        session_id_from_transcript_path(root.path(), &directly_nested)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );

    let project = transcript.parent().unwrap();
    let deeply_nested = project.join("nested").join(format!("{USER_ID}.jsonl"));
    fs::create_dir(project.join("nested")).unwrap();
    fs::write(&deeply_nested, b"provider transcript\n").unwrap();
    assert_eq!(
        session_id_from_transcript_path(root.path(), &deeply_nested)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );

    for invalid_name in [
        "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA.jsonl".to_owned(),
        "11111111-1111-1111-8111-111111111111.jsonl".to_owned(),
        "11111111-1111-4111-7111-111111111111.jsonl".to_owned(),
        format!("{USER_ID}.txt"),
    ] {
        let invalid = project.join(invalid_name);
        fs::write(&invalid, b"provider transcript\n").unwrap();
        assert_eq!(
            session_id_from_transcript_path(root.path(), &invalid)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}

#[cfg(unix)]
#[test]
fn transcript_path_rejects_symlinked_store_components() {
    use std::os::unix::fs::symlink;

    let actual_root = tempdir().unwrap();
    let transcript = fixture(actual_root.path());
    let parent = tempdir().unwrap();
    let linked_root = parent.path().join("projects");
    symlink(actual_root.path(), &linked_root).unwrap();
    let linked_transcript = linked_root
        .join(transcript.parent().unwrap().file_name().unwrap())
        .join(transcript.file_name().unwrap());
    assert!(session_id_from_transcript_path(&linked_root, &linked_transcript).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let linked_transcript = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    symlink(&transcript, &linked_transcript).unwrap();
    assert!(session_id_from_transcript_path(root.path(), &linked_transcript).is_err());
}

#[cfg(unix)]
#[test]
fn transcript_path_recovers_through_a_moved_project_alias() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    // Claude reports the new cwd's project name even when its transcript is
    // still stored in the old project directory beside it.
    let alias = root.path().join(project_dir_name("/moved/app"));
    for target in [
        transcript.parent().unwrap().file_name().unwrap().into(),
        transcript.parent().unwrap().to_path_buf(),
    ] {
        symlink(target, &alias).unwrap();
        assert_eq!(
            session_id_from_transcript_path(
                root.path(),
                &alias.join(transcript.file_name().unwrap()),
            )
            .unwrap(),
            SESSION_ID
        );
        fs::remove_file(&alias).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn transcript_project_alias_replacement_is_refused_during_verification() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let target = fs::canonicalize(transcript.parent().unwrap()).unwrap();
    let alias = root.path().join("moved-project");
    symlink(&target, &alias).unwrap();
    let observed = fs::symlink_metadata(&alias).unwrap();
    assert_session_probe_alias(root.path(), &alias, &observed, &target).unwrap();
    let replacement = root.path().join("replacement-project");
    fs::create_dir(&replacement).unwrap();
    // Both are valid siblings, but changing which one the hook named during
    // the metadata probe must still invalidate the observation.
    fs::remove_file(&alias).unwrap();
    symlink(replacement, &alias).unwrap();
    assert!(assert_session_probe_alias(root.path(), &alias, &observed, &target).is_err());
}

#[cfg(unix)]
#[test]
fn transcript_project_alias_cannot_escape_the_store_or_hide_unsafe_metadata() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let transcript = fixture(outside.path());
    let alias = root.path().join(project_dir_name("/moved/app"));
    symlink(transcript.parent().unwrap(), &alias).unwrap();
    let claimed = alias.join(format!("{SESSION_ID}.jsonl"));
    assert!(session_id_from_transcript_path(root.path(), &claimed).is_err());
    fs::remove_file(&alias).unwrap();

    let transcript = fixture(root.path());
    symlink(transcript.parent().unwrap(), &alias).unwrap();
    fs::set_permissions(
        transcript.parent().unwrap(),
        fs::Permissions::from_mode(0o777),
    )
    .unwrap();
    assert!(session_id_from_transcript_path(root.path(), &claimed).is_err());
    fs::set_permissions(
        transcript.parent().unwrap(),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    fs::set_permissions(&transcript, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(session_id_from_transcript_path(root.path(), &claimed).is_err());
    fs::remove_file(&transcript).unwrap();
    symlink(outside.path().join("missing.jsonl"), &transcript).unwrap();
    assert!(session_id_from_transcript_path(root.path(), &claimed).is_err());
}

#[cfg(unix)]
#[test]
fn session_presence_passes_over_an_alias_of_another_project_in_the_store() {
    use std::os::unix::fs::symlink;

    // A project folder that moved leaves Claude's store holding a link from the
    // new path's name to the old project directory beside it. The link adds no
    // transcript the scan would not reach through the real directory, so it
    // must not abort every other lookup that happens to list after it.
    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let project = transcript.parent().unwrap().file_name().unwrap().to_owned();
    symlink(&project, root.path().join(project_dir_name("/moved/app"))).unwrap();

    assert!(session_exists(root.path(), SESSION_ID).unwrap());
    // A missing session walks every entry, the alias included.
    assert!(!session_exists(root.path(), USER_ID).unwrap());
}

#[cfg(unix)]
#[test]
fn session_presence_rejects_unsafe_directories() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    let actual_root = tempdir().unwrap();
    fixture(actual_root.path());
    let parent = tempdir().unwrap();
    let linked_root = parent.path().join("projects");
    symlink(actual_root.path(), &linked_root).unwrap();
    assert!(session_exists(&linked_root, SESSION_ID).is_err());

    let root = tempdir().unwrap();
    let external = tempdir().unwrap();
    let external_project = external.path().join("external-project");
    fs::create_dir(&external_project).unwrap();
    fs::write(
        external_project.join(format!("{SESSION_ID}.jsonl")),
        b"provider transcript\n",
    )
    .unwrap();
    symlink(&external_project, root.path().join("linked-project")).unwrap();
    assert!(session_exists(root.path(), SESSION_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let project = transcript.parent().unwrap();
    let mut permissions = fs::metadata(project).unwrap().permissions();
    permissions.set_mode(0o777);
    fs::set_permissions(project, permissions).unwrap();
    assert!(session_exists(root.path(), SESSION_ID).is_err());

    let root = tempdir().unwrap();
    fixture(root.path());
    let mut permissions = fs::metadata(root.path()).unwrap().permissions();
    permissions.set_mode(0o777);
    fs::set_permissions(root.path(), permissions).unwrap();
    assert!(session_exists(root.path(), SESSION_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let project = transcript.parent().unwrap();
    let root_metadata = fs::metadata(root.path()).unwrap();
    let project_metadata = fs::metadata(project).unwrap();
    let wrong_uid = root_metadata.uid().wrapping_add(1);
    assert!(validate_session_probe_unix_metadata(&root_metadata, false, wrong_uid).is_err());
    assert!(validate_session_probe_unix_metadata(&project_metadata, false, wrong_uid).is_err());
}

#[cfg(unix)]
#[test]
fn session_presence_rejects_unsafe_transcript_metadata() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let candidate = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    symlink(&transcript, &candidate).unwrap();
    assert!(session_exists(root.path(), USER_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let candidate = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    fs::hard_link(&transcript, &candidate).unwrap();
    assert!(session_exists(root.path(), USER_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let candidate = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    fs::write(&candidate, []).unwrap();
    assert!(session_exists(root.path(), USER_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let candidate = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    File::create(&candidate)
        .unwrap()
        .set_len(MAX_SESSION_PROBE_BYTES + 1)
        .unwrap();
    assert!(session_exists(root.path(), USER_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let candidate = transcript.with_file_name(format!("{USER_ID}.jsonl"));
    fs::write(&candidate, b"provider transcript\n").unwrap();
    let mut permissions = fs::metadata(&candidate).unwrap().permissions();
    permissions.set_mode(0o666);
    fs::set_permissions(&candidate, permissions).unwrap();
    assert!(session_exists(root.path(), USER_ID).is_err());

    let root = tempdir().unwrap();
    let transcript = fixture(root.path());
    let metadata = fs::metadata(transcript).unwrap();
    assert!(
        validate_session_probe_unix_metadata(&metadata, true, metadata.uid().wrapping_add(1))
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn listing_cache_skips_unchanged_transcripts_and_re_reads_changed_ones() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempdir().unwrap();
    let path = fixture(root.path());
    let engine = EngineId::new("claude").unwrap();
    let mut cache = ListingCache::default();
    assert_eq!(
        list_sessions(root.path(), &engine, &mut cache)
            .unwrap()
            .sessions[0]
            .title,
        "Fixture"
    );

    // Permissions/ctime are part of the file version. A changed access policy
    // invalidates the memo even when length and mtime are untouched.
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o000);
    fs::set_permissions(&path, permissions).unwrap();
    let cached = list_sessions(root.path(), &engine, &mut cache).unwrap();
    if fs::File::open(&path).is_err() {
        assert!(!cached.complete);
        assert!(cached.sessions.is_empty());
    } else {
        assert_eq!(cached.sessions[0].title, "Fixture");
    }

    // A changed transcript is read again, so the title moves with it.
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(&path, permissions).unwrap();
    fs::write(
        &path,
        format!("{{\"type\":\"custom-title\",\"customTitle\":\"Renamed\",\"sessionId\":\"{SESSION_ID}\"}}\n"),
    )
    .unwrap();
    let refreshed = list_sessions(root.path(), &engine, &mut cache).unwrap();
    assert_eq!(refreshed.sessions[0].title, "Renamed");

    // A transcript that leaves the store leaves the memo with it.
    fs::remove_file(&path).unwrap();
    assert!(
        list_sessions(root.path(), &engine, &mut cache)
            .unwrap()
            .sessions
            .is_empty()
    );
    assert!(cache.entries.is_empty());
}

/// Appends one assistant record to `path` before each of the next `times`
/// unchanged checks, as a live Claude session does while its transcript is read.
fn append_during_reads(path: &Path, times: usize) {
    let path = path.to_path_buf();
    let mut left = times;
    cache_file::BEFORE_VERIFY.set(Some(Box::new(move || {
        if left > 0 {
            left -= 1;
            append_assistant_record(&path);
        }
    })));
}

fn append_assistant_record(path: &Path) {
    append_line(
        path,
        "{\"type\":\"assistant\",\"message\":{\"content\":\"more\"}}\n",
    );
}

fn append_line(path: &Path, line: &str) {
    use std::io::Write as _;

    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(line.as_bytes())
        .unwrap();
}

#[cfg(unix)]
#[test]
fn listing_reads_a_transcript_again_when_claude_appends_during_its_summary_read() {
    let root = tempdir().unwrap();
    let path = fixture(root.path());
    let padding = format!(
        "{{\"type\":\"assistant\",\"message\":{{\"content\":\"{}\"}}}}\n",
        "x".repeat(64 * 1024)
    );
    let mut transcript = fs::read_to_string(&path).unwrap();
    transcript.push_str(&padding);
    fs::write(&path, transcript).unwrap();
    let engine = EngineId::new("claude").unwrap();
    let mut cache = ListingCache::default();
    assert!(
        list_sessions(root.path(), &engine, &mut cache)
            .unwrap()
            .complete
    );

    // One append races the read: the read is tried again and the listing
    // stays complete.
    append_assistant_record(&path);
    append_during_reads(&path, 1);
    let raced = list_sessions(root.path(), &engine, &mut cache);
    cache_file::BEFORE_VERIFY.set(None);
    let raced = raced.unwrap();
    assert!(raced.complete);
    assert_eq!(raced.sessions[0].title, "Fixture");

    // A transcript that never holds still keeps the row the last listing
    // read, and the listing stays complete. Its entry survives: the next
    // listing resumes from the verified prefix instead of reparsing the whole
    // file.
    append_line(
        &path,
        &format!(
            "{{\"type\":\"custom-title\",\"customTitle\":\"Renamed\",\"sessionId\":\"{SESSION_ID}\"}}\n"
        ),
    );
    append_during_reads(&path, usize::MAX);
    let moving = list_sessions(root.path(), &engine, &mut cache);
    cache_file::BEFORE_VERIFY.set(None);
    let moving = moving.unwrap();
    assert!(moving.complete);
    assert_eq!(moving.sessions[0].title, "Fixture");
    let settled = list_sessions(root.path(), &engine, &mut cache).unwrap();
    assert!(settled.complete);
    assert_eq!(settled.sessions[0].title, "Renamed");
    assert!(cache.entries[&path].parsed_bytes() < 4096);

    // A transcript no listing has read yet has no row to stand in for it, so
    // one that never holds still leaves the listing incomplete.
    append_during_reads(&path, usize::MAX);
    let first = list_sessions(root.path(), &engine, &mut ListingCache::default());
    cache_file::BEFORE_VERIFY.set(None);
    let first = first.unwrap();
    assert!(!first.complete);
    assert!(first.sessions.is_empty());

    // An unsafe transcript still evicts its entry.
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(root.path().join("elsewhere"), &path).unwrap();
    assert!(
        !list_sessions(root.path(), &engine, &mut cache)
            .unwrap()
            .complete
    );
    assert!(cache.entries.is_empty());
}

#[test]
fn sanitized_fixture_lists_and_replays_both_sides() {
    let root = tempdir().unwrap();
    fixture(root.path());
    let engine = EngineId::new("claude").unwrap();
    let listed = list_sessions(root.path(), &engine, &mut ListingCache::default()).unwrap();
    assert!(listed.complete);
    assert_eq!(listed.sessions[0].title, "Fixture");
    assert_eq!(listed.sessions[0].origin, Some(SessionOrigin::App));
    let page = history(root.path(), SESSION_ID, "/work/app", 500);
    assert_eq!(page.messages, Some(2));
    assert!(matches!(
        page.events.first().map(|event| &event.event),
        Some(EngineEventKind::UserMessage { text, .. }) if text == "hello"
    ));
    assert!(page.events.iter().any(|event| matches!(
        &event.event,
        EngineEventKind::MessageDelta { text, .. } if text == "world"
    )));
}

#[test]
fn image_only_turn_is_visible_and_capped_by_shared_boundary() {
    let message = json!({"content": (0..10).map(|_| json!({
        "type":"image", "source":{"type":"base64", "media_type":"image/png", "data":"YQ=="}
    })).collect::<Vec<_>>()});
    let images = user_turn_images(Some(&message));
    assert_eq!(images.len(), 8);
    assert_eq!(user_turn_text(Some(&message)), None);
}

#[test]
fn cross_language_image_history_fixture_matches_rust_events() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/provider-history-user-image-events.json"
    ))
    .unwrap();
    for name in [
        "claudeInline",
        "claudeImageOnly",
        "claudePlain",
        "claudeRefusedRemote",
    ] {
        let case = &fixture[name];
        let actual = history_events([&case["input"]]);
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            case["events"],
            "fixture case {name}"
        );
    }
}

#[test]
fn mutations_are_scoped_and_fork_leaves_original() {
    let root = tempdir().unwrap();
    let source = fixture(root.path());
    let stale_cwd = "/work/app-before-it-moved";
    let page = history(root.path(), SESSION_ID, stale_cwd, 500);
    assert_eq!(page.messages, Some(2));
    rename(root.path(), SESSION_ID, " New name ", stale_cwd).unwrap();
    tag(root.path(), SESSION_ID, false, stale_cwd).unwrap();
    let forked = fork(root.path(), SESSION_ID, stale_cwd).unwrap();
    assert!(source.exists());
    assert!(source.with_file_name(format!("{forked}.jsonl")).exists());
    delete(root.path(), &forked, stale_cwd).unwrap();
    assert!(!source.with_file_name(format!("{forked}.jsonl")).exists());

    let records = read_jsonl(&source).unwrap();
    assert_eq!(
        records.last().and_then(|record| record.get("tag")),
        Some(&json!(""))
    );
}

#[cfg(unix)]
#[test]
fn scoped_store_fault_isolated_for_history_but_never_retried_for_mutation() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let source = fixture(root.path());
    let stale_cwd = "/work/stale-recorded-path";
    let stale_project = root.path().join(project_dir_name(stale_cwd));
    fs::create_dir_all(&stale_project).unwrap();
    let unsafe_scoped = stale_project.join(format!("{SESSION_ID}.jsonl"));
    symlink(&source, &unsafe_scoped).unwrap();

    let page = history(root.path(), SESSION_ID, stale_cwd, 500);
    assert_eq!(page.messages, Some(2));

    let before = fs::read(&source).unwrap();
    let error = rename(root.path(), SESSION_ID, "must not land", stale_cwd).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert_eq!(fs::read(&source).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn listing_keeps_healthy_rows_when_an_independent_transcript_is_unsafe() {
    use std::os::unix::fs::symlink;

    const BAD_ID: &str = "44444444-4444-4444-8444-444444444444";
    let root = tempdir().unwrap();
    let source = fixture(root.path());
    let broken_project = root.path().join("broken-project");
    fs::create_dir_all(&broken_project).unwrap();
    symlink(&source, broken_project.join(format!("{BAD_ID}.jsonl"))).unwrap();

    let listed = list_sessions(
        root.path(),
        &EngineId::new("claude").unwrap(),
        &mut ListingCache::default(),
    )
    .unwrap();
    assert!(!listed.complete);
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].session_id, SESSION_ID);
}

#[test]
fn delete_validates_the_sidecar_before_removing_the_authoritative_transcript() {
    let root = tempdir().unwrap();
    let source = fixture(root.path());
    let original = fs::read(&source).unwrap();
    let unsafe_sidecar = source.with_extension("");
    fs::write(&unsafe_sidecar, b"not a directory").unwrap();

    let error = delete(root.path(), SESSION_ID, "/work/app").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(fs::read(&unsafe_sidecar).unwrap(), b"not a directory");
}

#[test]
fn fork_matches_provider_reparenting_and_metadata_inheritance() {
    let records = vec![
        json!({"type":"history-suppression","sessionId":SESSION_ID}),
        json!({"type":"user","uuid":USER_ID,"parentUuid":null,
            "sessionId":SESSION_ID,"message":{"content":"hello"}}),
        json!({"type":"progress","uuid":"55555555-5555-4555-8555-555555555555",
            "parentUuid":USER_ID,"sessionId":SESSION_ID}),
        json!({"type":"assistant","uuid":ASSISTANT_ID,
            "parentUuid":"55555555-5555-4555-8555-555555555555",
            "sessionId":SESSION_ID,"timestamp":"2000-01-01T00:00:00Z",
            "message":{"content":"world"}}),
        json!({"type":"content-replacement","sessionId":SESSION_ID,
            "replacements":[{"old":"a","new":"b"}]}),
        json!({"type":"relocated","sessionId":SESSION_ID,
            "relocatedCwd":"/work/moved"}),
        json!({"type":"custom-title","sessionId":SESSION_ID,
            "customTitle":"Source"}),
    ];
    let forked_id = "66666666-6666-4666-8666-666666666666";
    let forked = fork_records(&records, SESSION_ID, forked_id).unwrap();
    assert_eq!(forked[0]["type"], "history-suppression");
    let user = forked
        .iter()
        .find(|record| record["type"] == "user")
        .unwrap();
    let assistant = forked
        .iter()
        .find(|record| record["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["parentUuid"], user["uuid"]);
    assert_ne!(assistant["timestamp"], "2000-01-01T00:00:00Z");
    assert!(!forked.iter().any(|record| record["type"] == "progress"));
    assert!(forked.iter().any(|record| {
        record["type"] == "content-replacement" && record["sessionId"] == forked_id
    }));
    assert!(forked.iter().any(|record| {
        record["type"] == "relocated" && record["relocatedCwd"] == "/work/moved"
    }));
    assert_eq!(forked.last().unwrap()["customTitle"], "Source (fork)");
    assert!(!forked.iter().any(|record| record["type"] == "tag"));
}

/// Four owner prompts. Between them: a tool round trip, a system entry, a
/// meta caveat, a slash command and a compaction summary; the fourth prompt
/// has no answer. A rename lands at the end.
fn turn_records() -> Vec<Value> {
    let mut parent = Value::Null;
    let mut records = Vec::new();
    let mut entry = |index: usize, kind: &str, extra: Value| {
        let uuid = format!("00000000-0000-4000-8000-{index:012}");
        let mut record = json!({"type":kind,"uuid":uuid,"parentUuid":parent,
            "sessionId":SESSION_ID,"cwd":"/work/app","timestamp":"2026-10-06T10:00:00.000Z"});
        for (key, value) in extra.as_object().unwrap() {
            record[key] = value.clone();
        }
        parent = Value::String(uuid);
        records.push(record);
    };
    let user = |content: Value| json!({"message":{"role":"user","content":content}});
    let assistant = |text: &str| json!({"message":{"role":"assistant","content":[{"type":"text","text":text}]}});
    entry(1, "user", user(json!("first prompt")));
    entry(
        2,
        "assistant",
        json!({"message":{"role":"assistant","content":[
        {"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"/work/app/a"}}]}}),
    );
    entry(
        3,
        "user",
        user(json!([{"type":"tool_result","tool_use_id":"tool-1","content":"a"}])),
    );
    entry(4, "assistant", assistant("first answer"));
    entry(
        5,
        "system",
        json!({"subtype":"turn_duration","durationMs":5}),
    );
    entry(
        6,
        "user",
        json!({"isMeta":true,"message":{"role":"user","content":"Caveat: local"}}),
    );
    entry(
        7,
        "user",
        user(json!("<command-name>/model</command-name>")),
    );
    entry(8, "user", user(json!("second prompt")));
    entry(9, "assistant", assistant("second answer"));
    entry(
        10,
        "user",
        user(json!(
            "This session is being continued from a previous conversation that ran out of context."
        )),
    );
    entry(
        11,
        "user",
        user(json!([{"type":"text","text":"third prompt"}])),
    );
    entry(12, "assistant", assistant("third answer, part one"));
    entry(13, "assistant", assistant("third answer, part two"));
    entry(14, "user", user(json!("fourth prompt")));
    records
        .push(json!({"type":"custom-title","customTitle":"Named session","sessionId":SESSION_ID}));
    records
}

fn at(prompt: &str, later_same_digest: u32) -> ForkBoundary {
    ForkBoundary {
        prompt_digest: interface_engine::prompt_digest(prompt),
        later_same_digest,
    }
}

#[test]
fn fork_at_cuts_after_the_last_answer_before_the_next_owner_prompt() {
    let records = turn_records();
    let uuid_at = |cut: usize| records[cut]["uuid"].as_str().unwrap().to_owned();
    assert_eq!(
        uuid_at(fork_cut(&records, &at("first prompt", 0)).unwrap()),
        "00000000-0000-4000-8000-000000000004"
    );
    assert_eq!(
        uuid_at(fork_cut(&records, &at("second prompt", 0)).unwrap()),
        "00000000-0000-4000-8000-000000000009"
    );
    assert_eq!(
        uuid_at(fork_cut(&records, &at("third prompt", 0)).unwrap()),
        "00000000-0000-4000-8000-000000000013"
    );
    assert!(
        fork_cut(&records, &at("fourth prompt", 0))
            .unwrap_err()
            .contains("no answer yet")
    );
    for missing in [at("fifth prompt", 0), at("first prompt", 1)] {
        assert_eq!(
            fork_cut(&records, &missing).unwrap_err(),
            FORK_BOUNDARY_NOT_FOUND
        );
    }

    // The prompts are the history page's: every prompt it shows, nothing else.
    let shown = history_events(records.iter().filter(|record| is_history_message(record)))
        .into_iter()
        .filter(|event| matches!(event.event, EngineEventKind::UserMessage { .. }))
        .count();
    assert_eq!(shown, 4);
}

/// 6 Oct 2026 (Threads review): the cut counted prompts from the start of
/// the transcript, so a thread seeded from older history, or a prompt sent
/// from a terminal between two recorded turns, cut at the wrong place. It
/// names the kept prompt instead, the newest match first.
#[test]
fn fork_at_finds_the_kept_prompt_past_older_history_repeats_and_outside_prompts() {
    let mut records = Vec::new();
    let mut parent = Value::Null;
    for (index, (kind, text)) in [
        ("user", "from before the thread"),
        ("assistant", "old answer"),
        ("user", "again"),
        ("assistant", "first again"),
        ("user", "typed in a terminal"),
        ("assistant", "terminal answer"),
        ("user", "again\r\n"),
        ("assistant", "second again"),
    ]
    .into_iter()
    .enumerate()
    {
        let uuid = format!("00000000-0000-4000-8000-{index:012}");
        records.push(json!({"type":kind,"uuid":uuid,"parentUuid":parent,
            "sessionId":SESSION_ID,"message":{"role":kind,"content":text}}));
        parent = Value::String(uuid);
    }
    let answer = |boundary: ForkBoundary| {
        let cut = fork_cut(&records, &boundary).unwrap();
        records[cut]["message"]["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    // The thread recorded "again" twice: the earlier one has one later turn
    // with the same digest.
    assert_eq!(answer(at("again", 1)), "first again");
    assert_eq!(answer(at("again", 0)), "second again");
    assert_eq!(answer(at("from before the thread", 0)), "old answer");
    assert_eq!(
        fork_cut(&records, &at("again", 2)).unwrap_err(),
        FORK_BOUNDARY_NOT_FOUND
    );
}

/// Two owner prompts with a stopped turn between them: the stop's marker
/// (both forms) and a `!` bash echo are user messages Claude Code wrote.
#[test]
fn fork_at_counts_owner_prompts_never_interrupt_markers_or_bash_echoes() {
    let mut records = Vec::new();
    let mut parent = Value::Null;
    for (index, (kind, content)) in [
        ("user", json!("first prompt")),
        ("assistant", json!([{"type":"tool_use","id":"tool-1","name":"Bash","input":{}}])),
        ("user", json!([{"type":"tool_result","tool_use_id":"tool-1","content":"The user doesn't want to proceed with this tool use.","is_error":true}])),
        ("user", json!([{"type":"text","text":"[Request interrupted by user for tool use]"}])),
        ("user", json!("<bash-input>ls</bash-input>")),
        ("user", json!("<bash-stdout>Cargo.toml</bash-stdout><bash-stderr></bash-stderr>")),
        ("user", json!("second prompt")),
        ("assistant", json!([{"type":"text","text":"second answer"}])),
        ("user", json!([{"type":"text","text":"[Request interrupted by user]"}])),
        ("user", json!("third prompt mentions [Request interrupted by user]")),
        ("assistant", json!([{"type":"text","text":"third answer"}])),
    ]
    .into_iter()
    .enumerate()
    {
        let uuid = format!("00000000-0000-4000-8000-{index:012}");
        records.push(json!({"type":kind,"uuid":uuid,"parentUuid":parent,
            "sessionId":SESSION_ID,"message":{"role":kind,"content":content}}));
        parent = Value::String(uuid);
    }
    let uuid_at = |cut: usize| records[cut]["uuid"].as_str().unwrap().to_owned();
    // The stopped first turn's answer is its tool call.
    assert_eq!(
        uuid_at(fork_cut(&records, &at("first prompt", 0)).unwrap()),
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        uuid_at(fork_cut(&records, &at("second prompt", 0)).unwrap()),
        "00000000-0000-4000-8000-000000000007"
    );
    assert_eq!(
        uuid_at(
            fork_cut(
                &records,
                &at("third prompt mentions [Request interrupted by user]", 0)
            )
            .unwrap()
        ),
        "00000000-0000-4000-8000-000000000010"
    );
    for marker in [
        "[Request interrupted by user]",
        "<bash-input>ls</bash-input>",
    ] {
        assert_eq!(
            fork_cut(&records, &at(marker, 0)).unwrap_err(),
            FORK_BOUNDARY_NOT_FOUND
        );
    }
    // The history page still shows the markers and the echoes.
    let shown = history_events(records.iter().filter(|record| is_history_message(record)))
        .into_iter()
        .filter(|event| matches!(event.event, EngineEventKind::UserMessage { .. }))
        .count();
    assert_eq!(shown, 7);
}

#[test]
fn fork_at_writes_a_resumable_copy_with_exactly_the_kept_turns() {
    let root = tempdir().unwrap();
    let project = root.path().join(project_dir_name("/work/app"));
    fs::create_dir_all(&project).unwrap();
    let source = project.join(format!("{SESSION_ID}.jsonl"));
    let mut transcript = String::new();
    for record in turn_records() {
        writeln!(transcript, "{record}").unwrap();
    }
    fs::write(&source, &transcript).unwrap();

    let forked = fork_at(
        root.path(),
        SESSION_ID,
        "/work/app",
        &at("second prompt", 0),
    )
    .unwrap()
    .unwrap();
    assert_eq!(fs::read_to_string(&source).unwrap(), transcript);
    let copy = read_jsonl(&source.with_file_name(format!("{forked}.jsonl"))).unwrap();
    assert!(
        copy.iter()
            .all(|record| record["sessionId"] == forked.as_str())
    );
    let kept_from = copy
        .iter()
        .filter_map(|record| record["forkedFrom"]["messageUuid"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        kept_from,
        (1..=9)
            .map(|index| format!("00000000-0000-4000-8000-{index:012}"))
            .collect::<Vec<_>>()
    );
    let last = copy
        .iter()
        .rev()
        .find(|record| record["type"] == "assistant")
        .unwrap();
    assert_eq!(last["message"]["content"][0]["text"], "second answer");
    assert_ne!(last["timestamp"], "2026-10-06T10:00:00.000Z");
    assert_eq!(copy.last().unwrap()["customTitle"], "Named session (fork)");

    let page = history(root.path(), &forked, "/work/app", 0);
    let prompts = page
        .events
        .iter()
        .filter_map(|event| match &event.event {
            EngineEventKind::UserMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(prompts, ["first prompt", "second prompt"]);

    let refused = fork_at(
        root.path(),
        SESSION_ID,
        "/work/app",
        &at("fourth prompt", 0),
    )
    .unwrap();
    assert!(refused.unwrap_err().contains("no answer yet"));
    assert_eq!(fs::read_dir(&project).unwrap().count(), 2);
}

#[test]
fn fork_parent_walk_ends_on_a_progress_cycle() {
    let first = "55555555-5555-4555-8555-555555555555";
    let second = "77777777-7777-4777-8777-777777777777";
    let records = vec![
        json!({"type":"progress","uuid":first,"parentUuid":second,"sessionId":SESSION_ID}),
        json!({"type":"progress","uuid":second,"parentUuid":first,"sessionId":SESSION_ID}),
        json!({"type":"progress","uuid":"88888888-8888-4888-8888-888888888888",
            "parentUuid":"88888888-8888-4888-8888-888888888888","sessionId":SESSION_ID}),
        json!({"type":"assistant","uuid":ASSISTANT_ID,"parentUuid":first,
            "sessionId":SESSION_ID,"message":{"content":"world"}}),
    ];
    let forked =
        fork_records(&records, SESSION_ID, "66666666-6666-4666-8666-666666666666").unwrap();
    let assistant = forked
        .iter()
        .find(|record| record["type"] == "assistant")
        .unwrap();
    assert_eq!(assistant["parentUuid"], Value::Null);
}

#[test]
fn listing_streams_header_fields_and_skips_unparseable_payloads() {
    let root = tempdir().unwrap();
    let project = root.path().join(project_dir_name("/work/app"));
    fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION_ID}.jsonl"));
    let assistant = r#"{"type":"assistant","userType":"external","message":{"content":[{"type":"tool_use","name":"Write","input":{"content":"not a listing field"}}]}}"#;
    fs::write(
        &path,
        format!(
            "{assistant}\n\
                 not-json\n\
                 {{\"type\":\"user\",\"cwd\":\"/work/app\",\"userType\":\"human\",\"message\":{{\"content\":\"first prompt\"}}}}\n\
                 {assistant}\n\
                 {{\"type\":\"custom-title\",\"customTitle\":\"Last title\",\"sessionId\":\"{SESSION_ID}\"}}\n\
                 {{\"type\":\"tag\",\"tag\":\"interface\",\"sessionId\":\"{SESSION_ID}\"}}\n\
                 {{\"summary\":\"Later summary\"}}\n"
        ),
    )
    .unwrap();

    let listed = list_sessions(
        root.path(),
        &EngineId::new("claude").unwrap(),
        &mut ListingCache::default(),
    )
    .unwrap();
    assert!(listed.complete);
    assert_eq!(listed.sessions.len(), 1);
    assert_eq!(listed.sessions[0].title, "Later summary");
    assert_eq!(listed.sessions[0].cwd, "/work/app");
    assert_eq!(listed.sessions[0].origin, Some(SessionOrigin::App));
    assert_eq!(listed.sessions[0].title_from_summary, Some(true));
}

#[test]
fn history_retains_only_the_filtered_tail_page() {
    let root = tempdir().unwrap();
    let project = root.path().join(project_dir_name("/work/app"));
    fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION_ID}.jsonl"));
    let mut body = String::new();
    for index in 0..5 {
        write!(
            body,
            "{{\"type\":\"user\",\"uuid\":\"{USER_ID}\",\"message\":{{\"content\":\"u{index}\"}}}}\n\
                 {{\"type\":\"assistant\",\"uuid\":\"{ASSISTANT_ID}\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"a{index}\"}}]}}}}\n\
                 {{\"type\":\"user\",\"isSidechain\":true,\"message\":{{\"content\":\"side {index}\"}}}}\n\
                 {{\"type\":\"assistant\",\"isMeta\":true,\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"meta {index}\"}}]}}}}\n\
                 {{\"type\":\"user\",\"teamName\":\"other\",\"message\":{{\"content\":\"team {index}\"}}}}\n"
        )
        .unwrap();
    }
    fs::write(&path, body).unwrap();

    let page = history(root.path(), SESSION_ID, "/work/app", 3);
    assert_eq!(page.messages, Some(3));
    assert_eq!(page.older_messages, 7);
    assert!(matches!(
        page.events.first().map(|event| &event.event),
        Some(EngineEventKind::MessageDelta { text, .. }) if text == "a3"
    ));
    assert!(page.events.iter().any(
        |event| matches!(&event.event, EngineEventKind::UserMessage { text, .. } if text == "u4")
    ));
    assert!(!page.events.iter().any(|event| matches!(
        &event.event,
        EngineEventKind::UserMessage { text, .. } if text.starts_with("side") || text.starts_with("team")
    )));

    let unbounded = history(root.path(), SESSION_ID, "/work/app", 0);
    assert_eq!(unbounded.messages, Some(10));
    assert_eq!(unbounded.older_messages, 0);
}

#[test]
fn user_turn_strips_engine_blocks_only_when_present_and_in_table_order() {
    assert_eq!(
        user_turn_text(Some(&json!({"content": "plain prompt"}))),
        Some("plain prompt".into())
    );
    assert_eq!(
        user_turn_text(Some(&json!({
            "content": "keep <command-name>SECRET</command-name> end"
        }))),
        Some("keep  end".into())
    );
    assert_eq!(
        user_turn_text(Some(&json!({
            "content": "a <command-name> b <command-message> c </command-name> d </command-message> e"
        }))),
        Some("a  d </command-message> e".into())
    );
}

#[test]
fn lifecycle_rejects_non_provider_ids_without_touching_similarly_named_files() {
    let root = tempdir().unwrap();
    let project = root.path().join(project_dir_name("/work/app"));
    fs::create_dir_all(&project).unwrap();
    let transcript = project.join("not-a-provider-id.jsonl");
    fs::write(&transcript, b"unchanged\n").unwrap();

    let error = rename(root.path(), "not-a-provider-id", "changed", "/work/app").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(fs::read(&transcript).unwrap(), b"unchanged\n");
}

#[test]
fn a_listing_read_on_several_threads_matches_one_read_in_order() {
    let root = tempdir().unwrap();
    let engine = EngineId::new("claude").unwrap();
    for (index, cwd) in ["/work/a", "/work/b", "/work/c"].iter().enumerate() {
        let project = root.path().join(project_dir_name(cwd));
        fs::create_dir_all(&project).unwrap();
        for session in 0..4 {
            let id = format!("{index}{session}222222-2222-4222-8222-222222222222");
            let title = format!("Session {index}.{session}");
            fs::write(
                project.join(format!("{id}.jsonl")),
                format!(
                    "{{\"type\":\"user\",\"cwd\":\"{cwd}\",\"message\":{{\"role\":\"user\",\"content\":\"hello\"}}}}\n\
                     {{\"type\":\"custom-title\",\"customTitle\":\"{title}\"}}\n"
                ),
            )
            .unwrap();
        }
        // The same session id in two projects: the walk order decides ties.
        fs::write(
            project.join(format!("{SESSION_ID}.jsonl")),
            format!("{{\"type\":\"custom-title\",\"customTitle\":\"Shared in {cwd}\"}}\n"),
        )
        .unwrap();
    }
    fs::write(
        root.path()
            .join(project_dir_name("/work/a"))
            .join("99999999-1111-4111-8111-111111111111.jsonl"),
        [0xff_u8, 0xfe, b'\n'],
    )
    .unwrap();
    let serial = list_sessions_with(root.path(), &engine, &mut ListingCache::default(), 1).unwrap();
    let mut cache = ListingCache::default();
    let parallel = list_sessions_with(root.path(), &engine, &mut cache, 4).unwrap();
    assert_eq!(parallel.sessions, serial.sessions);
    assert_eq!(parallel.complete, serial.complete);
    assert!(
        !parallel.complete,
        "the unreadable transcript still marks it incomplete"
    );
    assert_eq!(parallel.sessions.len(), 13);
    // Every readable transcript file is kept for the next listing.
    assert_eq!(cache.entries.len(), 15);
    let warm = list_sessions_with(root.path(), &engine, &mut cache, 4).unwrap();
    assert_eq!(warm.sessions, serial.sessions);
}
