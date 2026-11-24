use std::fs;
use std::path::Path;

use interface_contracts::ipc::{CustomizationEntry, CustomizationKind, CustomizationScope};
use tempfile::TempDir;

use super::{CUSTOMIZATION_LIMITS, read_customization_inventory, read_frontmatter, rules_summary};

use std::os::unix::fs::symlink;

fn write(root: &Path, relative: &str, content: &str) {
    let file = root.join(relative);
    fs::create_dir_all(file.parent().expect("test file parent")).expect("create test parent");
    fs::write(file, content).expect("write test fixture");
}

fn find<'a>(
    entries: &'a [CustomizationEntry],
    kind: CustomizationKind,
    name: &str,
) -> Option<&'a CustomizationEntry> {
    entries
        .iter()
        .find(|entry| entry.kind == kind && entry.name == name)
}

#[test]
fn helpers_match_legacy_summary_and_frontmatter_rules() {
    assert_eq!(rules_summary("\n## Project rules\nbody"), "Project rules");
    let fields = read_frontmatter(
        "\u{feff}---\nname: 'deploy'\ndescription: \"Ship safely\"\n name: ignored\n---\nbody",
    )
    .expect("frontmatter");
    assert_eq!(fields.get("name").map(String::as_str), Some("deploy"));
    assert_eq!(
        fields.get("description").map(String::as_str),
        Some("Ship safely")
    );
    let lopsided =
        read_frontmatter("---\nopen: \"deploy\nclose: deploy'\n---\nbody").expect("frontmatter");
    assert_eq!(lopsided.get("open").map(String::as_str), Some("deploy"));
    assert_eq!(lopsided.get("close").map(String::as_str), Some("deploy"));
    assert!(read_frontmatter("# body only").is_none());
}

#[test]
fn lists_every_kind_without_leaking_private_settings_fields() {
    let project = TempDir::new().expect("project temp");
    let user = TempDir::new().expect("user temp");
    let root = fs::canonicalize(project.path()).expect("canonical project");
    let user_root = fs::canonicalize(user.path()).expect("canonical user");

    write(&root, "CLAUDE.md", "# Project rules\n\nBe careful.\n");
    write(
        &root,
        ".claude/settings.json",
        r#"{"env":{"PRIVATE_VALUE":"fixture-private"},"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"command":"fixture-private-command"}]}]}}"#,
    );
    write(
        &root,
        ".claude/skills/deploy/SKILL.md",
        "---\nname: deploy-app\ndescription: Ship the packaged app.\n---\nprivate body\n",
    );
    write(&root, ".claude/skills/plain/SKILL.md", "# No frontmatter\n");
    write(&root, ".claude/commands/ship.md", "private command body\n");
    write(
        &root,
        ".mcp.json",
        r#"{"mcpServers":{"local":{"command":"private-bin","args":["private-arg"]},"docs":{"type":"http","url":"https://docs.example.com/private?value=fixture-private","headers":{"Authorization":"fixture-private"}}}}"#,
    );
    write(&user_root, "CLAUDE.md", "# Global rules\n");
    write(
        &user_root,
        "settings.json",
        r#"{"hooks":{"SessionStart":[{"matcher":"*"}]},"mcpServers":{"memory":{"command":"private-memory"}}}"#,
    );
    write(
        &user_root,
        "skills/writing/SKILL.md",
        "---\nname: writing\ndescription: House style.\n---\n",
    );
    write(&user_root, "commands/review.md", "private review body\n");

    let inventory = read_customization_inventory(&root, Some(&user_root));
    assert!(!inventory.truncated);
    assert_eq!(
        find(&inventory.entries, CustomizationKind::Rules, "CLAUDE.md")
            .expect("project rules")
            .detail,
        "Project rules"
    );
    assert_eq!(
        find(&inventory.entries, CustomizationKind::Hook, "PreToolUse")
            .expect("hook")
            .detail,
        "Bash"
    );
    assert_eq!(
        find(&inventory.entries, CustomizationKind::Skill, "deploy-app")
            .expect("skill")
            .detail,
        "Ship the packaged app."
    );
    assert!(
        find(&inventory.entries, CustomizationKind::Skill, "plain")
            .expect("plain skill")
            .error
            .is_some()
    );
    assert_eq!(
        find(&inventory.entries, CustomizationKind::Mcp, "docs")
            .expect("remote MCP")
            .detail,
        "http · https://docs.example.com"
    );
    assert_eq!(
        find(&inventory.entries, CustomizationKind::Mcp, "memory")
            .expect("user MCP")
            .scope,
        CustomizationScope::User
    );
    assert!(find(&inventory.entries, CustomizationKind::Command, "ship").is_some());
    assert!(find(&inventory.entries, CustomizationKind::Command, "review").is_some());

    let serialized = serde_json::to_string(&inventory).expect("serialize inventory");
    for forbidden in [
        "fixture-private",
        "private-bin",
        "private-arg",
        "private-memory",
        "private command body",
        "Authorization",
        "docs.example.com/private",
    ] {
        assert!(!serialized.contains(forbidden), "leaked {forbidden}");
    }
}

#[test]
fn malformed_and_oversized_known_files_are_named_but_not_exposed() {
    let project = TempDir::new().expect("project temp");
    let user = TempDir::new().expect("user temp");
    write(project.path(), ".claude/settings.json", "{ invalid");
    write(
        project.path(),
        "CLAUDE.md",
        &format!(
            "# Huge\n{}",
            "x".repeat(CUSTOMIZATION_LIMITS.max_file_bytes + 1)
        ),
    );
    let inventory = read_customization_inventory(project.path(), Some(user.path()));
    assert!(inventory.truncated);
    assert!(
        find(&inventory.entries, CustomizationKind::Hook, "settings.json")
            .expect("malformed settings")
            .error
            .is_some()
    );
    let rules =
        find(&inventory.entries, CustomizationKind::Rules, "CLAUDE.md").expect("oversized rules");
    assert!(rules.detail.is_empty());
    assert!(rules.error.is_some());
}

#[test]
fn caps_each_kind_and_marks_truncation() {
    let project = TempDir::new().expect("project temp");
    let user = TempDir::new().expect("user temp");
    for index in 0..CUSTOMIZATION_LIMITS.max_entries_per_kind + 5 {
        write(
            project.path(),
            &format!(".claude/commands/cmd-{index:04}.md"),
            "body",
        );
    }
    let inventory = read_customization_inventory(project.path(), Some(user.path()));
    assert_eq!(
        inventory
            .entries
            .iter()
            .filter(|entry| entry.kind == CustomizationKind::Command)
            .count(),
        CUSTOMIZATION_LIMITS.max_entries_per_kind
    );
    assert!(inventory.truncated);
}

#[test]
fn credential_shaped_paths_are_never_opened_or_listed() {
    let project = TempDir::new().expect("project temp");
    let user = TempDir::new().expect("user temp");
    write(project.path(), "CLAUDE.md", "# Rules\n");
    write(
        project.path(),
        ".claude/skills/.env.d/SKILL.md",
        "---\nname: should-not-appear\ndescription: fixture-private\n---\n",
    );
    write(
        project.path(),
        ".claude/commands/.env.md",
        "fixture-private",
    );
    let inventory = read_customization_inventory(project.path(), Some(user.path()));
    let serialized = serde_json::to_string(&inventory).expect("serialize inventory");
    assert!(!serialized.contains("fixture-private"));
    assert!(!serialized.contains(".env"));
    assert!(
        inventory
            .entries
            .iter()
            .all(|entry| entry.kind != CustomizationKind::Skill)
    );
    write(
        project.path(),
        ".claude/skills/secrets/SKILL.md",
        "---\nname: leaked-secret\ndescription: fixture-private\n---\n",
    );
    let second = read_customization_inventory(project.path(), Some(user.path()));
    assert!(
        second
            .entries
            .iter()
            .all(|entry| entry.name != "leaked-secret")
    );
}

#[test]
fn refuses_file_and_directory_symlink_escapes() {
    let project = TempDir::new().expect("project temp");
    let user = TempDir::new().expect("user temp");
    let outside = TempDir::new().expect("outside temp");
    write(
        outside.path(),
        "skills/escaped/SKILL.md",
        "---\nname: escaped\ndescription: Outside.\n---\n",
    );
    write(outside.path(), "CLAUDE.md", "# Outside\n");
    fs::create_dir_all(project.path().join(".claude/skills")).expect("skills dir");
    symlink(
        outside.path().join("skills/escaped"),
        project.path().join(".claude/skills/escaped"),
    )
    .expect("skill symlink");
    symlink(
        outside.path().join("CLAUDE.md"),
        project.path().join("CLAUDE.md"),
    )
    .expect("rules symlink");

    let inventory = read_customization_inventory(project.path(), Some(user.path()));
    assert!(
        inventory
            .entries
            .iter()
            .all(|entry| entry.name != "escaped")
    );
    let rules = find(&inventory.entries, CustomizationKind::Rules, "CLAUDE.md")
        .expect("linked rules named");
    assert!(rules.error.is_some());
    assert!(
        !serde_json::to_string(&inventory)
            .expect("serialize")
            .contains("Outside")
    );

    let project_two = TempDir::new().expect("second project");
    let config_outside = TempDir::new().expect("outside config");
    write(
        config_outside.path(),
        "settings.json",
        r#"{"hooks":{"Leaked":[{}]}}"#,
    );
    symlink(config_outside.path(), project_two.path().join(".claude")).expect("config symlink");
    let second = read_customization_inventory(project_two.path(), Some(user.path()));
    assert!(second.entries.is_empty());
}

#[test]
fn walks_two_skill_levels_and_returns_empty_for_missing_roots() {
    let project = TempDir::new().expect("project temp");
    let user = TempDir::new().expect("user temp");
    write(
        project.path(),
        ".claude/skills/team/nested/SKILL.md",
        "---\nname: nested\ndescription: Two.\n---\n",
    );
    write(
        project.path(),
        ".claude/skills/team/nested/deeper/SKILL.md",
        "---\nname: deeper\ndescription: Three.\n---\n",
    );
    let inventory = read_customization_inventory(project.path(), Some(user.path()));
    let skill_names = inventory
        .entries
        .iter()
        .filter(|entry| entry.kind == CustomizationKind::Skill)
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(skill_names, ["nested"]);

    let missing = project.path().join("gone");
    let empty = read_customization_inventory(&missing, Some(&user.path().join("gone")));
    assert_eq!(empty.root, missing.to_string_lossy());
    assert!(empty.entries.is_empty());
    assert!(!empty.truncated);
}
