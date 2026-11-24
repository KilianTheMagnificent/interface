use super::{
    Collector, CustomizationKind, CustomizationScope, FileRead, Path, PathBuf, ScanTarget, Value,
    canonical_or_absolute, collect_hooks, collect_mcp, entry, lexically_normalized,
    read_bounded_file, scan_commands, scan_rules, scan_skills,
};
use interface_contracts::customization::{
    CustomizationLoaded, CustomizationProvider, McpHealth, McpHealthState, ProviderCustomization,
};

#[path = "inventory_jsonc.rs"]
mod jsonc;
#[path = "inventory_probe.rs"]
mod probe;

/// Startup configuration supplied by the host, never by renderer paths.
#[derive(Clone, Debug)]
pub struct CustomizationRoots {
    pub claude: PathBuf,
    pub claude_mcp: PathBuf,
    pub opencode_file: Option<PathBuf>,
    pub opencode_extra: Option<PathBuf>,
    pub agents: PathBuf,
    pub codex: PathBuf,
    pub grok: PathBuf,
    pub qwen: PathBuf,
    pub opencode: PathBuf,
}
impl CustomizationRoots {
    #[must_use]
    pub fn for_home(home: &Path) -> Self {
        Self {
            claude: home.join(".claude"),
            claude_mcp: home.join(".claude.json"),
            opencode_file: None,
            opencode_extra: None,
            agents: home.join(".agents"),
            codex: home.join(".codex"),
            grok: home.join(".grok"),
            qwen: home.join(".qwen"),
            opencode: home.join(".config/opencode"),
        }
    }
    #[must_use]
    pub fn root(&self, provider: CustomizationProvider) -> &Path {
        match provider {
            CustomizationProvider::Claude => &self.claude,
            CustomizationProvider::Codex => &self.codex,
            CustomizationProvider::Grok => &self.grok,
            CustomizationProvider::Qwen => &self.qwen,
            CustomizationProvider::Opencode => &self.opencode,
        }
    }
}

fn provider_files(
    provider: CustomizationProvider,
) -> (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
) {
    match provider {
        CustomizationProvider::Claude => (
            ".claude",
            &["CLAUDE.md"],
            &["settings.json", "settings.local.json"],
        ),
        CustomizationProvider::Codex => (
            ".codex",
            &["AGENTS.md", "AGENTS.override.md"],
            &["config.toml"],
        ),
        CustomizationProvider::Grok => (".grok", &["GROK.md", "AGENTS.md"], &["settings.json"]),
        CustomizationProvider::Qwen => (".qwen", &["QWEN.md", "AGENTS.md"], &["settings.json"]),
        CustomizationProvider::Opencode => (
            ".opencode",
            &["AGENTS.md"],
            &["opencode.json", "opencode.jsonc"],
        ),
    }
}

fn read_provider_config(file: &Path, root: &Path) -> Result<Value, &'static str> {
    let text = match read_bounded_file(file, root) {
        FileRead::Read(text) => text,
        FileRead::Missing => return Err("Source is missing or outside the configured folder."),
        FileRead::Failed(message) => return Err(message),
    };
    if file.extension().is_some_and(|ext| ext == "toml") {
        let parsed: toml::Value =
            toml::from_str(&text).map_err(|_| "Could not be parsed as TOML.")?;
        serde_json::to_value(parsed).map_err(|_| "Could not read the TOML configuration.")
    } else if file.extension().is_some_and(|ext| ext == "jsonc") {
        jsonc::parse(&text)
    } else {
        serde_json::from_str(&text).map_err(|_| "Could not be parsed as JSON.")
    }
}

fn servers(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    value
        .get("mcpServers")
        .or_else(|| value.get("mcp_servers"))
        .or_else(|| value.get("mcp"))?
        .as_object()
}

fn scan_provider_config(file: &Path, target: ScanTarget<'_>, collector: &mut Collector) {
    if !file.exists() {
        return;
    }
    let value = match read_provider_config(file, target.root) {
        Ok(value) => value,
        Err(message) => {
            collector.add(entry(
                CustomizationKind::Mcp,
                file.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                String::new(),
                target,
                file,
                Some(message),
            ));
            return;
        }
    };
    collect_hooks(&value, file, target, collector);
    if let Some(servers) = servers(&value) {
        collect_mcp(
            &serde_json::json!({"mcpServers": servers}),
            file,
            target,
            collector,
        );
    }
}

fn scan_provider_directory(
    provider: CustomizationProvider,
    scope: CustomizationScope,
    scan_root: &Path,
    config: &Path,
    collector: &mut Collector,
) {
    let (_, rules, configs) = provider_files(provider);
    let target = ScanTarget {
        scope,
        root: scan_root,
        rules_directory: scan_root,
        config_directory: config,
        rules_files: rules,
        settings_files: &[],
        mcp_files: &[],
    };
    scan_rules(target, collector);
    let _ = scan_skills(&config.join("skills"), target, collector, 1);
    scan_commands(&config.join("commands"), target, collector);
    for name in configs {
        scan_provider_config(&config.join(name), target, collector);
    }
    if scope == CustomizationScope::Project {
        if provider == CustomizationProvider::Claude {
            scan_provider_config(&scan_root.join(".mcp.json"), target, collector);
        }
        if provider == CustomizationProvider::Opencode {
            for name in configs {
                scan_provider_config(&scan_root.join(name), target, collector);
            }
        }
        if provider == CustomizationProvider::Codex {
            let _ = scan_skills(&scan_root.join(".agents/skills"), target, collector, 1);
        }
    }
}

fn scan_extra_file(
    file: &Path,
    provider: CustomizationProvider,
    root: &Path,
    collector: &mut Collector,
) {
    let file = lexically_normalized(file);
    if let Some(parent) = file.parent() {
        let target = ScanTarget {
            scope: CustomizationScope::User,
            root: parent,
            rules_directory: parent,
            config_directory: parent,
            rules_files: &[],
            settings_files: &[],
            mcp_files: &[],
        };
        scan_provider_config(&file, target, collector);
        if provider == CustomizationProvider::Claude {
            if let Ok(value) = read_provider_config(&file, parent) {
                if let Some(local) = value
                    .get("projects")
                    .and_then(|projects| projects.get(root.to_string_lossy().as_ref()))
                {
                    // A local definition wins within this same user file. Keep one row
                    // per path/name so a Check cannot choose an ambiguous definition.
                    if let Some(local_servers) = servers(local) {
                        collector.entries.retain(|entry| {
                            !(entry.source_path == file.to_string_lossy()
                                && entry.kind == CustomizationKind::Mcp
                                && local_servers.contains_key(&entry.name))
                        });
                    }
                    collect_mcp(
                        local,
                        &file,
                        ScanTarget {
                            scope: CustomizationScope::Project,
                            ..target
                        },
                        collector,
                    );
                }
            }
        }
    }
}

/// File discovery only: no claim of precedence, trust, enabled state or live loading.
#[must_use]
pub fn provider_inventory(
    root: &Path,
    roots: &CustomizationRoots,
    provider: CustomizationProvider,
) -> ProviderCustomization {
    let root = canonical_or_absolute(root);
    let user = canonical_or_absolute(roots.root(provider));
    let (directory, _, _) = provider_files(provider);
    let project_config = root.join(directory);
    let mut collector = Collector::new();
    scan_provider_directory(
        provider,
        CustomizationScope::Project,
        &root,
        &project_config,
        &mut collector,
    );
    scan_provider_directory(
        provider,
        CustomizationScope::User,
        &user,
        &user,
        &mut collector,
    );
    if provider == CustomizationProvider::Opencode {
        if let Some(extra) = roots.opencode_extra.as_deref() {
            let extra = canonical_or_absolute(extra);
            if extra != user {
                scan_provider_directory(
                    provider,
                    CustomizationScope::User,
                    &extra,
                    &extra,
                    &mut collector,
                );
            }
        }
    }
    if provider == CustomizationProvider::Codex {
        let agents = canonical_or_absolute(&roots.agents);
        let target = ScanTarget {
            scope: CustomizationScope::User,
            root: &agents,
            rules_directory: &agents,
            config_directory: &agents,
            rules_files: &[],
            settings_files: &[],
            mcp_files: &[],
        };
        let _ = scan_skills(&agents.join("skills"), target, &mut collector, 1);
    }
    let extra = match provider {
        CustomizationProvider::Claude => Some(roots.claude_mcp.as_path()),
        CustomizationProvider::Opencode => roots.opencode_file.as_deref(),
        _ => None,
    };
    if let Some(file) = extra {
        scan_extra_file(file, provider, &root, &mut collector);
    }
    ProviderCustomization {
        provider,
        user_root: user.to_string_lossy().into_owned(),
        inventory: collector.result(&root),
        loaded: CustomizationLoaded::Unknown,
        source_path: None,
        health: None,
    }
}

/// Rebuilds the inventory and reopens only an exact discovered source without links.
/// # Errors
/// Refuses moved, replaced, non-inventory and symlinked paths.
pub fn revalidate_customization_source(
    inventory: &ProviderCustomization,
    path: &str,
) -> Result<PathBuf, &'static str> {
    if !inventory
        .inventory
        .entries
        .iter()
        .any(|entry| entry.source_path == path)
    {
        return Err(
            "That source is no longer in this provider's inventory. Refresh and choose it again.",
        );
    }
    let source = Path::new(path);
    let base = source.parent().ok_or("Source has no parent folder.")?;
    match read_bounded_file(source, base) {
        FileRead::Read(_) => Ok(source.to_path_buf()),
        FileRead::Failed(message) => Err(message),
        FileRead::Missing => Err("That source is no longer available. Refresh the inventory."),
    }
}

/// An explicit initialize/ping probe. Never calls tools and never emits raw output.
#[must_use]
pub fn check_mcp(inventory: &ProviderCustomization, path: &str, name: &str) -> McpHealth {
    let result = || -> Result<(McpHealthState, &'static str), &'static str> {
        if !inventory.inventory.entries.iter().any(|entry| {
            entry.kind == CustomizationKind::Mcp && entry.source_path == path && entry.name == name
        }) {
            return Err("This server is no longer in the inventory. Refresh before checking.");
        }
        let source = revalidate_customization_source(inventory, path)?;
        let base = source.parent().ok_or("Source has no parent folder.")?;
        let value = read_provider_config(&source, base)?;
        let local = value
            .get("projects")
            .and_then(|projects| projects.get(&inventory.inventory.root));
        let server = local
            .and_then(servers)
            .and_then(|servers| servers.get(name))
            .or_else(|| servers(&value).and_then(|servers| servers.get(name)))
            .ok_or("The server's configuration changed. Refresh before checking.")?;
        if server.get("enabled") == Some(&Value::Bool(false))
            || server.get("disabled") == Some(&Value::Bool(true))
        {
            return Ok((
                McpHealthState::Disabled,
                "Disabled in the provider configuration; no connection attempted.",
            ));
        }
        if server.get("url").is_some() {
            return Ok((
                McpHealthState::Unsupported,
                "HTTP server: check health in the provider's authenticated MCP session. No request was sent.",
            ));
        }
        if serde_json::to_string(server).is_ok_and(|text| {
            text.contains("${") || text.contains("{env:") || text.contains("{file:")
        }) {
            return Ok((
                McpHealthState::Unsupported,
                "This server uses provider-specific substitutions. Check it in that provider so its environment and credentials resolve correctly. No process was started.",
            ));
        }
        probe::stdio_probe(server, Path::new(&inventory.inventory.root))?;
        Ok((
            McpHealthState::Ready,
            "A temporary connection answered initialize and ping. No tools were called. Loading in existing sessions is still unknown.",
        ))
    };
    let (state, message) = result().unwrap_or_else(|message| (McpHealthState::Failed, message));
    McpHealth {
        path: path.to_owned(),
        name: name.to_owned(),
        state,
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[path = "inventory_provider_tests.rs"]
mod tests;
