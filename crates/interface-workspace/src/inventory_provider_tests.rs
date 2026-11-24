use super::*;
use std::os::unix::fs::symlink;
use std::{collections::BTreeSet, fs};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn codex_inventory_uses_its_configured_home_and_never_leaks_server_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let project = root.join("project");
    let mut roots = CustomizationRoots::for_home(&root.join("home"));
    roots.codex = root.join("separate-codex");
    write(&project.join("AGENTS.md"), "# Project guidance\n");
    write(
        &project.join(".agents/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review changes\n---\nprivate body",
    );
    write(&roots.claude.join("CLAUDE.md"), "Claude-only guidance");
    write(&roots.codex.join("AGENTS.md"), "Codex-only guidance");
    write(
        &roots.codex.join("config.toml"),
        r#"[mcp_servers.docs]
command = "private-command"
args = ["private-argument"]
[mcp_servers.docs.env]
TOKEN = "private-token"
"#,
    );
    let result = provider_inventory(&project, &roots, CustomizationProvider::Codex);
    assert_eq!(result.loaded, CustomizationLoaded::Unknown);
    assert!(
        result
            .inventory
            .entries
            .iter()
            .any(|entry| entry.name == "review")
    );
    assert!(
        result
            .inventory
            .entries
            .iter()
            .any(|entry| entry.name == "docs")
    );
    let encoded = serde_json::to_string(&result).unwrap();
    for private in [
        "private-command",
        "private-argument",
        "private-token",
        "private body",
        "Claude-only",
    ] {
        assert!(!encoded.contains(private));
    }
    assert!(encoded.contains("separate-codex"));
}

#[test]
fn a_source_link_is_refused_after_replacement_by_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let project = root.join("project");
    let roots = CustomizationRoots::for_home(&root.join("home"));
    let file = project.join("CLAUDE.md");
    write(&file, "Project rules");
    let result = provider_inventory(&project, &roots, CustomizationProvider::Claude);
    assert_eq!(
        revalidate_customization_source(&result, file.to_str().unwrap()).unwrap(),
        file
    );
    fs::remove_file(&file).unwrap();
    symlink("/etc/passwd", &file).unwrap();
    assert!(revalidate_customization_source(&result, file.to_str().unwrap()).is_err());
    assert!(revalidate_customization_source(&result, "/etc/passwd").is_err());
}

#[test]
fn mcp_check_initializes_and_pings_without_tools_or_raw_output() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let project = root.join("project");
    let roots = CustomizationRoots::for_home(&root.join("home"));
    let script = root.join("server.sh");
    let log = root.join("requests");
    write(
        &script,
        r#"while IFS= read -r line; do
printf '%s\n' "$line" >> "$1"
case "$line" in
*'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"private-server-text"}}}' ;;
*'"method":"ping"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{}}' ;;
esac
done
"#,
    );
    let config = project.join(".mcp.json");
    write(&config, &serde_json::json!({"mcpServers":{"demo":{"command":"/bin/sh","args":[script,log],"env":{"PRIVATE_TOKEN":"secret-value"}}}}).to_string());
    let inventory = provider_inventory(&project, &roots, CustomizationProvider::Claude);
    let health = check_mcp(&inventory, config.to_str().unwrap(), "demo");
    assert_eq!(health.state, McpHealthState::Ready, "{}", health.message);
    let requests = fs::read_to_string(log).unwrap();
    assert_eq!(requests.lines().count(), 3);
    assert!(!requests.contains("tools/"));
    let response = serde_json::to_string(&health).unwrap();
    assert!(!response.contains("secret-value"));
    assert!(!response.contains("private-server-text"));
}

#[test]
fn disabled_and_http_servers_never_start_a_probe() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let roots = CustomizationRoots::for_home(&root.join("home"));
    let config = root.join(".mcp.json");
    write(
        &config,
        r#"{"mcpServers":{"disabled":{"command":"must-not-run","enabled":false},"remote":{"url":"https://example.com/private?token=secret"}}}"#,
    );
    let inventory = provider_inventory(&root, &roots, CustomizationProvider::Claude);
    assert_eq!(
        check_mcp(&inventory, config.to_str().unwrap(), "disabled").state,
        McpHealthState::Disabled
    );
    assert_eq!(
        check_mcp(&inventory, config.to_str().unwrap(), "remote").state,
        McpHealthState::Unsupported
    );
    assert_eq!(
        check_mcp(&inventory, config.to_str().unwrap(), "absent").state,
        McpHealthState::Failed
    );
    assert!(
        !serde_json::to_string(&inventory)
            .unwrap()
            .contains("token=secret")
    );
}

#[test]
fn claude_user_file_lists_only_the_selected_projects_local_servers() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let roots = CustomizationRoots::for_home(&root.join("home"));
    write(&roots.claude_mcp, &serde_json::json!({"primaryApiKey":"secret-value",
        "mcpServers":{"shared":{"command":"never-started"}},
        "projects":{project.to_string_lossy().as_ref():{"mcpServers":{"local":{"command":"never-started"}}},
            "/other/project":{"mcpServers":{"other-private-name":{"command":"never-started"}}}}}).to_string());
    let result = provider_inventory(&project, &roots, CustomizationProvider::Claude);
    let names: Vec<_> = result
        .inventory
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["shared", "local"]);
    let source =
        revalidate_customization_source(&result, roots.claude_mcp.to_str().unwrap()).unwrap();
    assert_eq!(source, roots.claude_mcp);
    let encoded = serde_json::to_string(&result).unwrap();
    assert!(!encoded.contains("secret-value"));
    assert!(!encoded.contains("other-private-name"));
}

#[test]
fn codex_user_agents_skills_remain_discoverable_with_a_separate_codex_home() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let mut roots = CustomizationRoots::for_home(&root.join("home"));
    roots.codex = root.join("separate-codex");
    let skill = roots.agents.join("skills/shared/SKILL.md");
    write(
        &skill,
        "---\nname: shared\ndescription: Shared review skill\n---\nprivate body",
    );
    let result = provider_inventory(&project, &roots, CustomizationProvider::Codex);
    let entry = result
        .inventory
        .entries
        .iter()
        .find(|entry| entry.name == "shared")
        .unwrap();
    assert_eq!(entry.scope, CustomizationScope::User);
    assert_eq!(entry.source_path, skill.to_string_lossy());
    assert_eq!(
        revalidate_customization_source(&result, &entry.source_path).unwrap(),
        skill
    );
    assert!(
        !serde_json::to_string(&result)
            .unwrap()
            .contains("private body")
    );
}

#[test]
fn opencode_custom_directory_adds_to_global_config_instead_of_hiding_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    let mut roots = CustomizationRoots::for_home(&root.join("home"));
    let extra = root.join("extra-opencode");
    roots.opencode_extra = Some(extra.clone());
    let global = roots.opencode.join("opencode.json");
    let additional = extra.join("opencode.jsonc");
    write(
        &global,
        r#"{"mcp":{"global":{"type":"local","command":["never-started"]}}}"#,
    );
    write(
        &additional,
        r#"{// additional config
"mcp":{"additional":{"type":"local","command":["never-started"]}},}"#,
    );
    let result = provider_inventory(&project, &roots, CustomizationProvider::Opencode);
    let names: BTreeSet<_> = result
        .inventory
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, BTreeSet::from(["additional", "global"]));
    for file in [global, additional] {
        assert_eq!(
            revalidate_customization_source(&result, file.to_str().unwrap()).unwrap(),
            file
        );
    }
    assert_eq!(result.loaded, CustomizationLoaded::Unknown);
}

#[test]
fn incompatible_mcp_protocol_is_not_initialized_or_reported_ready() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let roots = CustomizationRoots::for_home(&root.join("home"));
    let script = root.join("server.sh");
    let log = root.join("requests");
    write(
        &script,
        r#"while IFS= read -r line; do
printf '%s\n' "$line" >> "$1"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"unsupported-version","capabilities":{}}}'
done
"#,
    );
    let config = root.join(".mcp.json");
    write(
        &config,
        &serde_json::json!({"mcpServers":{"demo":{"command":"/bin/sh","args":[script,log]}}})
            .to_string(),
    );
    let inventory = provider_inventory(&root, &roots, CustomizationProvider::Claude);
    let result = check_mcp(&inventory, config.to_str().unwrap(), "demo");
    assert_eq!(result.state, McpHealthState::Failed);
    assert_eq!(fs::read_to_string(log).unwrap().lines().count(), 1);
}
