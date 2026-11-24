use super::*;
use std::os::unix::fs::symlink;

fn fixture() -> (tempfile::TempDir, Map, BTreeSet<String>) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("sample/src")).unwrap();
    for file in [
        "AGENTS.md",
        "sample/AGENTS.md",
        "sample/src/lib.rs",
        "guide.md",
    ] {
        fs::write(root.path().join(file), "fixture\n").unwrap();
    }
    let map = Map {
        schema_version: 1,
        routes: vec![Route {
            id: "sample".into(),
            summary: "Small example".into(),
            keywords: vec!["example".into()],
            roots: vec!["sample".into()],
            rules: vec!["AGENTS.md".into(), "sample/AGENTS.md".into()],
            guides: vec!["guide.md".into()],
            entrypoints: vec!["sample/src/lib.rs".into()],
            tests: vec!["sample/src/lib.rs".into()],
            checks: vec!["cargo test -p sample".into()],
            constraints: vec!["Keep the existing contract".into()],
        }],
    };
    (root, map, BTreeSet::from(["sample".into()]))
}

#[test]
fn routes_representative_tasks_to_actual_owners() {
    let map: Map = serde_json::from_str(include_str!("../../docs/agent-map.json")).unwrap();
    for (query, expected) in [
        ("sidebar pinning", "sidebar"),
        ("PTY launch", "interface-terminal"),
        ("phone pairing", "interface-remote"),
        ("slash help", "cli"),
        ("speech chunk", "interface-voice"),
        ("atomic write", "interface-storage"),
        ("receipt", "pipeline"),
        ("away stop", "away"),
        ("WINDOW CSP", "electron-bridge"),
        ("streaming transcript", "conversation"),
    ] {
        assert!(
            matching(&map, query)
                .iter()
                .any(|route| route.id == expected),
            "{query}"
        );
    }
    assert_eq!(
        matching(&map, "interface-terminal")[0].id,
        "interface-terminal"
    );
    assert!(matching(&map, "no-such-subsystem").is_empty());
    assert_eq!(matching(&map, "").len(), map.routes.len());
    assert!(
        matching(&map, "crates/interface-remote/src/http/direct.rs")
            .iter()
            .any(|route| route.id == "interface-remote")
    );
    assert!(
        matching(&map, "crates/interface-remote/")
            .iter()
            .any(|route| route.id == "interface-remote")
    );
}

#[test]
fn rejects_deleted_entries_and_missing_workspace_coverage() {
    let (root, map, mut members) = fixture();
    validate(root.path(), &map, &members).unwrap();
    members.insert("new-crate".into());
    assert!(
        validate(root.path(), &map, &members)
            .unwrap_err()
            .to_string()
            .contains("new-crate")
    );
    members.remove("new-crate");
    fs::remove_file(root.path().join("sample/src/lib.rs")).unwrap();
    assert!(validate(root.path(), &map, &members).is_err());
}

#[test]
fn rejects_missing_scoped_rules_and_empty_checks() {
    let (root, mut map, members) = fixture();
    map.routes[0].checks.clear();
    assert!(
        validate(root.path(), &map, &members)
            .unwrap_err()
            .to_string()
            .contains("checks")
    );
    map.routes[0].checks.push("cargo test -p sample".into());
    fs::write(root.path().join("sample/src/AGENTS.md"), "narrow rules").unwrap();
    assert!(
        validate(root.path(), &map, &members)
            .unwrap_err()
            .to_string()
            .contains("sample/src/AGENTS.md")
    );
}

#[test]
fn rejects_duplicate_ids_and_unknown_schema_fields() {
    let (root, mut map, members) = fixture();
    let duplicate = serde_json::from_value(serde_json::to_value(&map.routes[0]).unwrap()).unwrap();
    map.routes.push(duplicate);
    assert!(validate(root.path(), &map, &members).is_err());
    let mut value = serde_json::to_value(&map).unwrap();
    value["schemaVerison"] = 1.into();
    assert!(serde_json::from_value::<Map>(value).is_err());
    fs::create_dir(root.path().join("docs")).unwrap();
    map.schema_version = 2;
    fs::write(root.path().join(MAP), serde_json::to_vec(&map).unwrap()).unwrap();
    assert!(
        load(root.path())
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
}

#[test]
fn refuses_escape_secret_and_symlink_paths_without_reading_them() {
    let (root, _, _) = fixture();
    for path in [
        "../outside",
        "/absolute",
        ".env",
        "sample/.env.production",
        "sample/config.env",
        "sample/private.key",
        "credentials.json",
        ".ssh/id_rsa",
        "sample/id_ed25519",
        "sample/release.jks",
        "sample/putty.ppk",
        ".git/config",
        "target/output",
    ] {
        assert!(validate_path(root.path(), path).is_err(), "{path}");
    }
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("linked")).unwrap();
    assert!(validate_path(root.path(), "linked/file").is_err());
    fs::write(root.path().join(".env.example"), "example").unwrap();
    validate_path(root.path(), ".env.example").unwrap();
}
