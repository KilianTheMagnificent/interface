use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use interface_contracts::engine::{EngineEvent, EngineEventKind};
use serde_json::json;
use tempfile::TempDir;

use super::*;

#[test]
fn indexes_track_ordered_lifecycle_and_reload() {
    let (_temp, root) = private_root();
    let engine = "codex".parse().unwrap();
    let mut registry = SessionRegistry::open(&root).unwrap();
    for round in 0..80 {
        let id = (round % 13).to_string();
        let key = format!("codex#{id}");
        registry
            .upsert_all(
                registry.revision(),
                &engine,
                [summary(&id, SessionStatus::Idle, "2026-01-01")],
            )
            .unwrap();
        match round % 4 {
            0 => {
                registry
                    .set_title(registry.revision(), &key, format!("Title {round}"))
                    .unwrap();
            }
            1 => {
                registry.forget(registry.revision(), &key).unwrap();
            }
            2 => {
                registry.remember(registry.revision(), &key).unwrap();
            }
            _ => {
                registry.remove(registry.revision(), &key).unwrap();
            }
        }
        assert_eq!(registry.session_positions, positions(&registry.sessions));
        assert_eq!(registry.title_positions, positions(&registry.titles));
        assert_eq!(
            registry.forgotten_keys,
            registry.forgotten.iter().cloned().collect()
        );
        for (key, summary) in &registry.sessions {
            assert_eq!(registry.get(key), Some(summary));
        }
    }
    registry.save(registry.revision()).unwrap();
    let original = fs::read(root.join(REGISTRY_FILE)).unwrap();
    let expected = registry.all();
    drop(registry);
    let mut reopened = SessionRegistry::open(&root).unwrap();
    assert_eq!(reopened.all(), expected);
    assert_eq!(reopened.session_positions, positions(&reopened.sessions));
    reopened.save(reopened.revision()).unwrap();
    // The legacy loader normalizes title-map order; compare that existing
    // normalization rather than imposing a new on-disk ordering contract.
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(root.join(REGISTRY_FILE)).unwrap()).unwrap(),
        serde_json::from_slice::<Value>(&original).unwrap()
    );
}

fn private_root() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("registry");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    (temp, root)
}

fn write_private_file(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn remove_many_matches_one_by_one_removal_with_one_bump() {
    let fixture = |root: &Path| {
        let engine = "codex".parse().unwrap();
        let mut registry = SessionRegistry::open(root).unwrap();
        let rows = (0..12).map(|id| summary(&id.to_string(), SessionStatus::Idle, "2026-01-01"));
        registry
            .upsert_all(registry.revision(), &engine, rows)
            .unwrap();
        for id in [1, 4, 7] {
            let key = format!("codex#{id}");
            registry
                .set_title(registry.revision(), &key, format!("T{id}"))
                .unwrap();
        }
        registry.forget(registry.revision(), "codex#9").unwrap();
        registry
    };
    let (_batched_temp, batched_root) = private_root();
    let (_single_temp, single_root) = private_root();
    let mut batched = fixture(&batched_root);
    let mut single = fixture(&single_root);
    let stale: HashSet<String> = ["codex#1", "codex#2", "codex#7", "codex#9", "codex#missing"]
        .map(str::to_owned)
        .into();
    let before = batched.revision();
    let (revision, removed) = batched.remove_many(before, &stale).unwrap();
    // codex#9 was already forgotten, so only three rows remained.
    assert_eq!((revision, removed), (before + 1, 3));
    for key in &stale {
        single.remove(single.revision(), key).unwrap();
    }
    assert_eq!(batched.sessions, single.sessions);
    assert_eq!(batched.titles, single.titles);
    assert_eq!(batched.forgotten, single.forgotten);
    assert_eq!(batched.session_positions, positions(&batched.sessions));
    assert_eq!(batched.title_positions, positions(&batched.titles));
    assert_eq!(
        batched.remove_many(revision, &stale).unwrap(),
        (revision, 0)
    );
    assert!(batched.remove_many(before, &stale).is_err());
}

fn summary(id: &str, status: SessionStatus, first_seen: &str) -> SessionSummary {
    SessionSummary {
        engine: "codex".parse().unwrap(),
        session_id: id.into(),
        title: id.into(),
        cwd: "/work".into(),
        status,
        cloud: false,
        model: None,
        effort: None,
        last_activity: first_seen.into(),
        first_seen: first_seen.into(),
        archived: false,
        size: None,
        title_from_summary: None,
        origin: Some(SessionOrigin::Store),
        project_root: None,
        worktree_home: None,
        worktree_path: None,
        setting_sources: None,
        disallowed_tools: None,
        kind: None,
        permission_mode: None,
    }
}

#[test]
fn golden_schema_merge_order_and_read_only_access_are_exact() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let revision = registry
        .upsert_all(
            registry.revision(),
            &engine,
            [
                summary("late", SessionStatus::Working, "2026-01-02"),
                summary("wait", SessionStatus::Waiting, "2026-01-03"),
                summary("early", SessionStatus::Working, "2026-01-01"),
            ],
        )
        .unwrap();
    registry.set_title(revision, "codex#late", "Named").unwrap();
    registry.save(registry.revision()).unwrap();
    let path = root.join(REGISTRY_FILE);
    let before = fs::read(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&before).unwrap(),
        json!({
            "sessions": {
                "codex#late": {
                    "engine":"codex", "sessionId":"late", "title":"Named",
                    "cwd":"/work", "status":"working", "cloud":false,
                    "lastActivity":"2026-01-02", "firstSeen":"2026-01-02",
                    "archived":false, "origin":"store"
                },
                "codex#wait": {
                    "engine":"codex", "sessionId":"wait", "title":"wait",
                    "cwd":"/work", "status":"waiting", "cloud":false,
                    "lastActivity":"2026-01-03", "firstSeen":"2026-01-03",
                    "archived":false, "origin":"store"
                },
                "codex#early": {
                    "engine":"codex", "sessionId":"early", "title":"early",
                    "cwd":"/work", "status":"working", "cloud":false,
                    "lastActivity":"2026-01-01", "firstSeen":"2026-01-01",
                    "archived":false, "origin":"store"
                }
            },
            "titles":{"codex#late":"Named"}
        })
    );
    assert_eq!(
        registry
            .list()
            .iter()
            .map(|item| item.session_id.as_str())
            .collect::<Vec<_>>(),
        ["wait", "early", "late"]
    );
    // `summaries` is the borrowing twin of `all`: same rows, same order.
    assert_eq!(
        registry.summaries().cloned().collect::<Vec<_>>(),
        registry.all()
    );
    drop(registry);
    let metadata_before = fs::metadata(&path).unwrap();
    let loaded = read_registry_file(&root, REGISTRY_FILE).unwrap();
    let metadata_after = fs::metadata(&path).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(before, fs::read(&path).unwrap());
    assert_eq!(
        metadata_before.modified().unwrap(),
        metadata_after.modified().unwrap()
    );
}

/// Pins the saved bytes, including map entry order (the `serde_json::Value`
/// comparison above ignores it): sessions and titles keep insertion order and
/// empty `titles`/`forgotten` are left out.
#[test]
fn saved_bytes_keep_insertion_order_and_omit_empty_sections() {
    let saved = |registry: &mut SessionRegistry, root: &Path| {
        registry.save(registry.revision()).unwrap();
        String::from_utf8(fs::read(root.join(REGISTRY_FILE)).unwrap()).unwrap()
    };
    let engine: EngineId = "codex".parse().unwrap();

    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    assert_eq!(saved(&mut registry, &root), r#"{"sessions":{}}"#);
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("gone", SessionStatus::Idle, "2026-01-01")],
        )
        .unwrap();
    registry
        .set_title(registry.revision(), "codex#gone", "Gone")
        .unwrap();
    registry.forget(registry.revision(), "codex#gone").unwrap();
    assert_eq!(
        saved(&mut registry, &root),
        r#"{"sessions":{},"forgotten":["codex#gone"]}"#
    );
    drop(registry);

    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [
                summary("late", SessionStatus::Working, "2026-01-02"),
                summary("wait", SessionStatus::Waiting, "2026-01-03"),
            ],
        )
        .unwrap();
    registry
        .set_title(registry.revision(), "codex#wait", "Waiting")
        .unwrap();
    registry
        .set_title(registry.revision(), "codex#late", "Named")
        .unwrap();
    registry.forget(registry.revision(), "codex#zeta").unwrap();
    registry.forget(registry.revision(), "codex#alpha").unwrap();
    assert_eq!(
        saved(&mut registry, &root),
        concat!(
            r#"{"sessions":{"#,
            r#""codex#late":{"engine":"codex","sessionId":"late","title":"Named","cwd":"/work","status":"working","cloud":false,"lastActivity":"2026-01-02","firstSeen":"2026-01-02","archived":false,"origin":"store"},"#,
            r#""codex#wait":{"engine":"codex","sessionId":"wait","title":"Waiting","cwd":"/work","status":"waiting","cloud":false,"lastActivity":"2026-01-03","firstSeen":"2026-01-03","archived":false,"origin":"store"}},"#,
            r#""titles":{"codex#wait":"Waiting","codex#late":"Named"},"#,
            r#""forgotten":["codex#zeta","codex#alpha"]}"#,
        )
    );
}

#[test]
fn store_merge_preserves_live_archive_origin_and_worktree_rules() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let mut original = summary("s", SessionStatus::Working, "1");
    original.title = "Real title".into();
    original.origin = Some(SessionOrigin::App);
    original.project_root = Some("/repo".into());
    original.worktree_home = Some("/repo".into());
    original.worktree_path = Some("/wt".into());
    original.cwd = "/repo".into();
    registry
        .upsert_all(registry.revision(), &engine, [original])
        .unwrap();
    registry
        .mark_archived(registry.revision(), "codex#s", true)
        .unwrap();
    let mut stale = summary("s", SessionStatus::Idle, "0");
    stale.title = "Codex session".into();
    stale.cwd = "/wt".into();
    stale.origin = Some(SessionOrigin::Store);
    registry
        .upsert_all(registry.revision(), &engine, [stale])
        .unwrap();
    let merged = registry.get("codex#s").unwrap();
    assert_eq!(merged.status, SessionStatus::Working);
    assert_eq!(merged.title, "Real title");
    assert!(merged.archived);
    assert_eq!(merged.origin, Some(SessionOrigin::App));
    assert_eq!(merged.cwd, "/repo");
    assert_eq!(merged.project_root.as_deref(), Some("/repo"));
}

#[test]
fn events_forget_and_stale_revisions_follow_legacy_rules() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("id#part", SessionStatus::Idle, "1")],
        )
        .unwrap();
    let stale = registry.revision() - 1;
    assert!(
        registry
            .mark_archived(stale, "codex#id#part", true)
            .is_err()
    );
    let event = EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text: "x".into(),
            agent_key: None,
        },
    };
    registry
        .apply_event(registry.revision(), "codex#id#part", &event, "2")
        .unwrap();
    assert_eq!(
        registry.get("codex#id#part").unwrap().status,
        SessionStatus::Working
    );
    registry
        .forget(registry.revision(), "codex#id#part")
        .unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("id#part", SessionStatus::Idle, "3")],
        )
        .unwrap();
    assert!(registry.get("codex#id#part").is_none());
}

#[test]
fn corrupt_cache_is_quarantined_before_starting_empty() {
    let (_temp, root) = private_root();
    fs::write(root.join(REGISTRY_FILE), b"{broken").unwrap();
    fs::set_permissions(root.join(REGISTRY_FILE), fs::Permissions::from_mode(0o600)).unwrap();
    let registry = SessionRegistry::open(&root).unwrap();
    assert!(registry.all().is_empty());
    assert!(!registry.restored_from_cache(), "rebuilt from listings");
    assert!(!root.join(REGISTRY_FILE).exists());
    assert_eq!(fs::read_dir(root.join(".quarantine")).unwrap().count(), 1);
    // The host's one-time notice reveals exactly these bytes.
    let artifact = registry.quarantined_artifact().unwrap().to_path_buf();
    assert!(artifact.starts_with(root.join(".quarantine")));
    assert_eq!(fs::read(&artifact).unwrap(), b"{broken");
    drop(registry);
    assert!(
        SessionRegistry::open(&root)
            .unwrap()
            .quarantined_artifact()
            .is_none()
    );
}

#[test]
fn legacy_cache_mode_is_repaired_and_remains_persistable() {
    let (_temp, root) = private_root();
    fs::write(root.join(REGISTRY_FILE), b"{\"sessions\":{}}").unwrap();
    fs::set_permissions(root.join(REGISTRY_FILE), fs::Permissions::from_mode(0o644)).unwrap();
    let mut registry = SessionRegistry::open(&root).unwrap();
    assert!(registry.all().is_empty());
    assert_eq!(
        fs::metadata(root.join(REGISTRY_FILE))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    registry.save(registry.revision()).unwrap();
}

#[test]
fn failed_quarantine_degrades_but_writer_lease_remains_exclusive() {
    let (_temp, root) = private_root();
    fs::write(root.join(".quarantine"), b"not a directory").unwrap();
    fs::set_permissions(root.join(".quarantine"), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(root.join(REGISTRY_FILE), b"{broken").unwrap();
    fs::set_permissions(root.join(REGISTRY_FILE), fs::Permissions::from_mode(0o600)).unwrap();
    let mut unavailable = SessionRegistry::open(&root).unwrap();
    assert!(unavailable.all().is_empty());
    assert!(unavailable.save(unavailable.revision()).is_err());
    assert!(root.join(REGISTRY_FILE).exists());

    let (_temp, root) = private_root();
    let mut owner = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let revision = owner
        .upsert_all(
            owner.revision(),
            &engine,
            [summary("read-only", SessionStatus::Idle, "1")],
        )
        .unwrap();
    owner.save(revision).unwrap();
    assert!(matches!(
        SessionRegistry::open(&root),
        Err(RegistryError::Storage(
            interface_storage::StorageError::LockHeld { .. }
        ))
    ));
    drop(owner);
    let next_owner = SessionRegistry::open(&root).unwrap();
    assert!(next_owner.get("codex#read-only").is_some());
}

#[test]
fn open_quarantines_only_this_registry_atomic_temps() {
    use interface_storage::{atomic_temp_digest_prefix, is_atomic_temp_name_for};
    use uuid::Uuid;

    let (_temp, root) = private_root();
    let registry_temp = format!(
        ".interface-atomic-{}-{}.tmp",
        atomic_temp_digest_prefix(REGISTRY_FILE),
        Uuid::new_v4()
    );
    let foreign_temp = format!(
        ".interface-atomic-{}-{}.tmp",
        atomic_temp_digest_prefix("appearance.json"),
        Uuid::new_v4()
    );
    assert!(is_atomic_temp_name_for(&registry_temp, REGISTRY_FILE));
    assert!(!is_atomic_temp_name_for(&foreign_temp, REGISTRY_FILE));
    assert!(is_atomic_temp_name_for(&foreign_temp, "appearance.json"));

    write_private_file(&root.join(&registry_temp), b"stale-registry-temp");
    write_private_file(&root.join(&foreign_temp), b"foreign-store-temp");

    let registry = SessionRegistry::open(&root).unwrap();
    assert!(registry.all().is_empty());
    assert!(!root.join(&registry_temp).exists());
    assert!(root.join(&foreign_temp).exists());
    assert_eq!(
        fs::read(root.join(&foreign_temp)).unwrap(),
        b"foreign-store-temp"
    );
    assert_eq!(fs::read_dir(root.join(".quarantine")).unwrap().count(), 1);
}

#[test]
fn linked_cache_starts_empty_without_touching_the_link_target() {
    let (_temp, root) = private_root();
    let outside = root.join("outside.json");
    let original = b"{\"sessions\":{\"outside\":true}}";
    fs::write(&outside, original).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&outside, root.join(REGISTRY_FILE)).unwrap();

    let mut registry = SessionRegistry::open(&root).unwrap();
    assert!(registry.all().is_empty());
    assert!(registry.save(registry.revision()).is_err());
    assert_eq!(fs::read(outside).unwrap(), original);
}

#[test]
fn an_older_prepared_save_never_overwrites_a_newer_one() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let older = registry.prepare_save(registry.revision()).unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("newer", SessionStatus::Idle, "2026-01-01")],
        )
        .unwrap();
    // Saves run outside the registry lock, so the older image can finish last.
    registry.save(registry.revision()).unwrap();
    let newer = fs::read(root.join(REGISTRY_FILE)).unwrap();
    older.write().unwrap();
    assert_eq!(fs::read(root.join(REGISTRY_FILE)).unwrap(), newer);
    assert_eq!(read_registry_file(&root, REGISTRY_FILE).unwrap().len(), 1);
}

#[test]
fn kind_round_trips_through_save_and_read_registry_file() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let mut stored = summary("chat-row", SessionStatus::Idle, "2026-01-01");
    stored.kind = Some(SessionKind::Chat);
    registry
        .upsert_all(registry.revision(), &engine, [stored])
        .unwrap();
    registry.save(registry.revision()).unwrap();
    drop(registry);

    let loaded = read_registry_file(&root, REGISTRY_FILE).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].kind, Some(SessionKind::Chat));
    let persisted =
        serde_json::from_slice::<Value>(&fs::read(root.join(REGISTRY_FILE)).unwrap()).unwrap();
    assert_eq!(persisted["sessions"]["codex#chat-row"]["kind"], "chat");
}

#[test]
fn invalid_kind_is_dropped_without_failing_the_row() {
    let (_temp, root) = private_root();
    let path = root.join(REGISTRY_FILE);
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "sessions": {
                "codex#s": {
                    "engine": "codex",
                    "sessionId": "s",
                    "title": "s",
                    "cwd": "/work",
                    "status": "idle",
                    "cloud": false,
                    "lastActivity": "1",
                    "firstSeen": "1",
                    "archived": false,
                    "kind": "banana"
                },
                "codex#n": {
                    "engine": "codex",
                    "sessionId": "n",
                    "title": "n",
                    "cwd": "/work",
                    "status": "idle",
                    "cloud": false,
                    "lastActivity": "1",
                    "firstSeen": "1",
                    "archived": false,
                    "kind": 1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    let loaded = read_registry_file(&root, REGISTRY_FILE).unwrap();
    assert_eq!(loaded.len(), 2);
    assert!(loaded.iter().all(|summary| summary.kind.is_none()));
}

#[test]
fn listing_merge_keeps_a_stored_kind() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let mut original = summary("s", SessionStatus::Idle, "1");
    original.kind = Some(SessionKind::Chat);
    registry
        .upsert_all(registry.revision(), &engine, [original])
        .unwrap();
    let mut listing = summary("s", SessionStatus::Idle, "2");
    listing.origin = Some(SessionOrigin::Store);
    listing.kind = None;
    registry
        .upsert_all(registry.revision(), &engine, [listing])
        .unwrap();
    assert_eq!(
        registry.get("codex#s").unwrap().kind,
        Some(SessionKind::Chat)
    );
}

#[test]
fn content_revision_skips_only_activity_only_events() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let key = "codex#a";
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [
                summary("b", SessionStatus::Idle, "2"),
                summary("a", SessionStatus::Idle, "1"),
            ],
        )
        .unwrap();
    let delta = EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text: "x".into(),
            agent_key: None,
        },
    };
    // Idle -> Working is a status change and counts as content.
    let before = registry.content_revision();
    registry
        .apply_event(registry.revision(), key, &delta, "3")
        .unwrap();
    assert_eq!(registry.content_revision(), before + 1);
    // A further delta moves only last_activity: the revision advances, the
    // content revision does not.
    let (revision, content) = (registry.revision(), registry.content_revision());
    registry.apply_event(revision, key, &delta, "4").unwrap();
    assert_eq!(registry.revision(), revision + 1);
    assert_eq!(registry.content_revision(), content);
    assert_eq!(registry.get(key).unwrap().last_activity, "4");
    // Every other mutator is content.
    let mutations: [&dyn Fn(&mut SessionRegistry); 5] = [
        &|registry| {
            registry
                .set_title(registry.revision(), key, "Renamed")
                .unwrap();
        },
        &|registry| {
            registry
                .mark_archived(registry.revision(), key, true)
                .unwrap();
        },
        &|registry| {
            registry
                .set_origin(registry.revision(), key, SessionOrigin::App)
                .unwrap();
        },
        &|registry| {
            let engine: EngineId = "codex".parse().unwrap();
            registry
                .upsert_all(
                    registry.revision(),
                    &engine,
                    [summary("c", SessionStatus::Idle, "5")],
                )
                .unwrap();
        },
        &|registry| {
            registry.forget(registry.revision(), "codex#b").unwrap();
        },
    ];
    for mutate in mutations {
        let content = registry.content_revision();
        mutate(&mut registry);
        assert_eq!(registry.content_revision(), content + 1);
    }
    // The unsorted clone plus the shared sort is exactly list().
    let mut rows = registry.list_unsorted();
    sort_listed(&mut rows);
    assert_eq!(rows, registry.list());
}

fn cache_identity(root: &Path) -> (u64, std::time::SystemTime) {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(root.join(REGISTRY_FILE)).unwrap();
    (metadata.ino(), metadata.modified().unwrap())
}

#[test]
fn an_identical_listing_neither_advances_the_revision_nor_rewrites_the_cache() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    let listing = || {
        [
            summary("a", SessionStatus::Idle, "2026-01-01"),
            summary("b", SessionStatus::Working, "2026-01-02"),
        ]
    };
    registry
        .upsert_all(registry.revision(), &engine, listing())
        .unwrap();
    assert!(!registry.cache_is_current());
    registry.save(registry.revision()).unwrap();
    assert!(registry.cache_is_current());
    let written = cache_identity(&root);
    let bytes = fs::read(root.join(REGISTRY_FILE)).unwrap();

    // The same listing again, and an empty one: no row changes.
    let (revision, content) = (registry.revision(), registry.content_revision());
    assert_eq!(
        registry.upsert_all(revision, &engine, listing()).unwrap(),
        revision
    );
    assert_eq!(
        registry.upsert_all(revision, &engine, []).unwrap(),
        revision
    );
    assert_eq!(
        (registry.revision(), registry.content_revision()),
        (revision, content)
    );
    assert!(registry.cache_is_current());
    // A save of the unchanged revision, and a stale image of it, write nothing.
    let stale = registry.prepare_save(revision).unwrap();
    registry.save(revision).unwrap();
    stale.write().unwrap();
    assert_eq!(cache_identity(&root), written);
    assert_eq!(fs::read(root.join(REGISTRY_FILE)).unwrap(), bytes);

    // A changed row, even only its activity time, still advances both
    // revisions and is written.
    let mut newer = summary("a", SessionStatus::Idle, "2026-01-01");
    newer.last_activity = "2026-01-05".into();
    let changed = registry.upsert_all(revision, &engine, [newer]).unwrap();
    assert_eq!(changed, revision + 1);
    assert_eq!(registry.content_revision(), content + 1);
    assert!(!registry.cache_is_current());
    registry.save(changed).unwrap();
    assert_ne!(cache_identity(&root).0, written.0);
    assert_eq!(
        read_registry_file(&root, REGISTRY_FILE).unwrap()[0].last_activity,
        "2026-01-05"
    );
}

#[test]
fn a_moved_revision_with_identical_bytes_keeps_the_installed_cache() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("a", SessionStatus::Idle, "2026-01-01")],
        )
        .unwrap();
    registry.save(registry.revision()).unwrap();
    let written = cache_identity(&root);
    // Archiving an unarchived row to its current value still counts as a
    // change, but the image it saves is byte-identical to the installed one.
    let revision = registry
        .mark_archived(registry.revision(), "codex#a", false)
        .unwrap();
    assert!(!registry.cache_is_current());
    registry.save(revision).unwrap();
    assert!(registry.cache_is_current());
    assert_eq!(cache_identity(&root), written);

    // Drift outside the snapshot is still reported, not papered over.
    let bytes = fs::read(root.join(REGISTRY_FILE)).unwrap();
    write_private_file(&root.join("replacement.json"), &bytes);
    fs::rename(root.join("replacement.json"), root.join(REGISTRY_FILE)).unwrap();
    let revision = registry
        .mark_archived(registry.revision(), "codex#a", false)
        .unwrap();
    assert!(matches!(
        registry.save(revision),
        Err(RegistryError::Storage(
            interface_storage::StorageError::ExternalDrift { .. }
        ))
    ));
    assert!(!registry.cache_is_current());
}

#[test]
fn a_reopened_registry_writes_its_first_save_even_when_the_bytes_match() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "codex".parse().unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("a", SessionStatus::Idle, "2026-01-01")],
        )
        .unwrap();
    registry.save(registry.revision()).unwrap();
    let bytes = fs::read(root.join(REGISTRY_FILE)).unwrap();
    let written = cache_identity(&root);
    drop(registry);

    // A file this process did not install is not known to be durable, so the
    // first save rewrites (and syncs) it as it always has.
    let mut reopened = SessionRegistry::open(&root).unwrap();
    assert!(!reopened.cache_is_current());
    reopened.save(reopened.revision()).unwrap();
    assert!(reopened.cache_is_current());
    assert_ne!(cache_identity(&root).0, written.0);
    assert_eq!(fs::read(root.join(REGISTRY_FILE)).unwrap(), bytes);
}

#[test]
fn a_registry_of_ten_thousand_sessions_still_saves_and_reloads() {
    // The node budget once refused every save past ~7,900 sessions, so
    // archive, rename and forget changes were lost on restart.
    let (_temp, root) = private_root();
    let engine: EngineId = "codex".parse().unwrap();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let rows = (0..10_000).map(|n| {
        let mut row = summary(&format!("s{n}"), SessionStatus::Idle, "2026-01-01");
        row.model = Some("gpt-5".into());
        row.project_root = Some("/work".into());
        row
    });
    registry
        .upsert_all(registry.revision(), &engine, rows)
        .unwrap();
    registry.save(registry.revision()).unwrap();
    drop(registry);
    assert_eq!(
        read_registry_file(&root, REGISTRY_FILE).unwrap().len(),
        10_000
    );
}

/// Thread records are marked missing only against a registry whose rows came
/// from its cache: one starting empty is rebuilt as providers list.
#[test]
fn only_a_readable_cache_file_counts_as_restored() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    assert!(!registry.restored_from_cache(), "no file yet");
    let engine: EngineId = "codex".parse().unwrap();
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [summary("s", SessionStatus::Idle, "1")],
        )
        .unwrap();
    registry.save(registry.revision()).unwrap();
    drop(registry);
    let reopened = SessionRegistry::open(&root).unwrap();
    assert!(reopened.restored_from_cache());
    assert_eq!(reopened.all().len(), 1);
}

/// 6 Oct 2026 (Threads F1 follow-up): Code launches are titled with their
/// prompt at once, which made every later provider title lose to it. The
/// provider's first own title (Claude's summary or custom title, a Codex
/// thread name) replaces the prompt for Code sessions; Chat keeps its
/// prompt, and an owner's or generated title (an override) always wins.
#[test]
fn a_providers_first_own_title_replaces_a_code_launchs_prompt_title_only() {
    let (_temp, root) = private_root();
    let mut registry = SessionRegistry::open(&root).unwrap();
    let engine: EngineId = "claude".parse().unwrap();
    let launched = |id: &str, kind: Option<SessionKind>| SessionSummary {
        engine: engine.clone(),
        title: "Fix the flaky login test".into(),
        origin: Some(SessionOrigin::App),
        kind,
        ..summary(id, SessionStatus::Idle, "1")
    };
    let listed = |id: &str, title: &str, from_summary: Option<bool>| SessionSummary {
        engine: engine.clone(),
        title: title.into(),
        title_from_summary: from_summary,
        ..summary(id, SessionStatus::Idle, "2")
    };
    let title = |registry: &SessionRegistry, id: &str| {
        registry.get(&format!("claude#{id}")).unwrap().title.clone()
    };
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [
                launched("code", Some(SessionKind::Code)),
                launched("kindless", None),
                launched("chat", Some(SessionKind::Chat)),
                launched("renamed", Some(SessionKind::Code)),
            ],
        )
        .unwrap();
    registry
        .set_title(registry.revision(), "claude#renamed", "Owner's name")
        .unwrap();

    // A listing that titles the session with its first prompt changes nothing.
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [listed("code", "Fix the flaky login", Some(false))],
        )
        .unwrap();
    assert_eq!(title(&registry, "code"), "Fix the flaky login test");

    // The provider's own title replaces the prompt for Code sessions only.
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            ["code", "kindless", "chat", "renamed"]
                .map(|id| listed(id, "Login test stabilized", Some(true))),
        )
        .unwrap();
    assert_eq!(title(&registry, "code"), "Login test stabilized");
    assert_eq!(title(&registry, "kindless"), "Login test stabilized");
    assert_eq!(title(&registry, "chat"), "Fix the flaky login test");
    assert_eq!(title(&registry, "renamed"), "Owner's name");

    // Once named by the provider, a later title (or a meta event carrying
    // the row's own flag) does not churn it.
    registry
        .upsert_all(
            registry.revision(),
            &engine,
            [listed("code", "Something else", Some(true))],
        )
        .unwrap();
    assert_eq!(title(&registry, "code"), "Login test stabilized");
}
