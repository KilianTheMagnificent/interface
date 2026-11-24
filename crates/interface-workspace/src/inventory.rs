use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use interface_common::paths::lexically_normalized;
use interface_contracts::ipc::{
    CustomizationEntry, CustomizationInventory, CustomizationKind, CustomizationScope,
};
use serde_json::Value;
use url::Url;

use crate::fsid::{inside, relative_string};
use crate::is_search_excluded_path;

use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

/// Hard limits for the read-only rules/skills/hooks/commands/MCP inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CustomizationLimits {
    pub max_entries_per_kind: usize,
    pub max_file_bytes: usize,
    pub max_skill_depth: usize,
    pub max_dir_entries: usize,
    pub max_detail_chars: usize,
}

pub const CUSTOMIZATION_LIMITS: CustomizationLimits = CustomizationLimits {
    max_entries_per_kind: 200,
    max_file_bytes: 256 * 1024,
    max_skill_depth: 2,
    max_dir_entries: 500,
    max_detail_chars: 80,
};

/// The legacy Claude-compatible customization directory. Production callers
/// may override this with an engine-specific root; tests always use a temp dir.
#[must_use]
pub fn default_customization_user_root() -> PathBuf {
    BaseDirs::new().map_or_else(
        || PathBuf::from(".claude"),
        |directories| directories.home_dir().join(".claude"),
    )
}

fn clip(value: &str, max: usize) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        flat.chars().take(max).collect::<String>().trim().to_owned()
    }
}

/// Returns the first non-empty rules line, stripping Markdown heading marks.
#[must_use]
pub fn rules_summary(text: &str) -> String {
    text.split('\n')
        .take(200)
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(String::new, |line| {
            let summary = if line.starts_with('#') {
                line.trim_start_matches('#').trim_start()
            } else {
                line
            };
            clip(summary, CUSTOMIZATION_LIMITS.max_detail_chars)
        })
}

/// Reads only scalar `key: value` rows from a leading frontmatter block. This
/// intentionally is not a YAML parser: inventory never needs a skill body.
#[must_use]
pub fn read_frontmatter(text: &str) -> Option<BTreeMap<String, String>> {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let content = body
        .strip_prefix("---\r\n")
        .or_else(|| body.strip_prefix("---\n"))?;
    let end = content.find("\n---")?;
    let mut fields = BTreeMap::new();
    for line in content[..end].split('\n').take(100) {
        if line.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() || fields.contains_key(key) {
            continue;
        }
        let value = raw_value.trim();
        let value = value.strip_prefix(['\'', '"']).unwrap_or(value);
        let value = value.strip_suffix(['\'', '"']).unwrap_or(value).to_owned();
        fields.insert(key.to_owned(), value);
    }
    Some(fields)
}

#[derive(Clone, Copy)]
struct ScanTarget<'a> {
    scope: CustomizationScope,
    root: &'a Path,
    rules_directory: &'a Path,
    config_directory: &'a Path,
    rules_files: &'static [&'static str],
    settings_files: &'static [&'static str],
    mcp_files: &'static [&'static str],
}

struct Collector {
    entries: Vec<CustomizationEntry>,
    counts: [usize; 5],
    truncated: bool,
}

impl Collector {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            counts: [0; 5],
            truncated: false,
        }
    }

    fn add(&mut self, entry: CustomizationEntry) -> bool {
        let index = kind_index(entry.kind);
        if self.counts[index] >= CUSTOMIZATION_LIMITS.max_entries_per_kind {
            self.truncated = true;
            return false;
        }
        self.counts[index] += 1;
        self.entries.push(entry);
        true
    }

    const fn truncate(&mut self) {
        self.truncated = true;
    }

    fn result(self, root: &Path) -> CustomizationInventory {
        CustomizationInventory {
            root: root.to_string_lossy().into_owned(),
            entries: self.entries,
            truncated: self.truncated,
        }
    }
}

const fn kind_index(kind: CustomizationKind) -> usize {
    match kind {
        CustomizationKind::Rules => 0,
        CustomizationKind::Skill => 1,
        CustomizationKind::Hook => 2,
        CustomizationKind::Mcp => 3,
        CustomizationKind::Command => 4,
    }
}

enum FileRead {
    Read(String),
    Missing,
    Failed(&'static str),
}

fn reached_without_links(directory: &Path, scan_root: &Path) -> bool {
    fs::canonicalize(directory)
        .is_ok_and(|canonical| canonical == directory && inside(scan_root, &canonical))
}

fn read_bounded_file(file: &Path, scan_root: &Path) -> FileRead {
    let Some(parent) = file.parent() else {
        return FileRead::Missing;
    };
    if !reached_without_links(parent, scan_root) {
        return FileRead::Missing;
    }
    let Ok(before) = fs::symlink_metadata(file) else {
        return FileRead::Missing;
    };
    if before.file_type().is_symlink() {
        return FileRead::Failed("Not read: this path is a symbolic link.");
    }
    if !before.file_type().is_file() {
        return FileRead::Missing;
    }
    if before.len() > u64::try_from(CUSTOMIZATION_LIMITS.max_file_bytes).unwrap_or(u64::MAX) {
        return FileRead::Failed("Not read: larger than 256 KB.");
    }

    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let Ok(mut opened) = options.open(file) else {
        return FileRead::Failed("Could not be read.");
    };
    let Ok(after) = opened.metadata() else {
        return FileRead::Failed("Could not be read.");
    };
    if !after.is_file() || !same_file(&before, &after) {
        return FileRead::Failed("Could not be read.");
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(after.len())
            .unwrap_or(CUSTOMIZATION_LIMITS.max_file_bytes)
            .min(CUSTOMIZATION_LIMITS.max_file_bytes),
    );
    let Ok(_) = opened
        .by_ref()
        .take(u64::try_from(CUSTOMIZATION_LIMITS.max_file_bytes + 1).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
    else {
        return FileRead::Failed("Could not be read.");
    };
    if bytes.len() > CUSTOMIZATION_LIMITS.max_file_bytes {
        return FileRead::Failed("Not read: larger than 256 KB.");
    }
    FileRead::Read(String::from_utf8_lossy(&bytes).into_owned())
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

struct SafeChild {
    name: String,
    directory: bool,
}

fn safe_children(directory: &Path, scan_root: &Path, collector: &mut Collector) -> Vec<SafeChild> {
    if !reached_without_links(directory, scan_root) {
        return Vec::new();
    }
    let Ok(listing) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for result in listing {
        let Ok(entry) = result else {
            continue;
        };
        if entries.len() >= CUSTOMIZATION_LIMITS.max_dir_entries {
            collector.truncate();
            break;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let absolute = directory.join(&name);
        let Some(relative) = relative_string(scan_root, &absolute) else {
            continue;
        };
        if is_search_excluded_path(&relative, file_type.is_dir()) {
            continue;
        }
        entries.push(SafeChild {
            name,
            directory: file_type.is_dir(),
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

fn entry(
    kind: CustomizationKind,
    name: String,
    detail: String,
    target: ScanTarget<'_>,
    source_path: &Path,
    error: Option<&str>,
) -> CustomizationEntry {
    CustomizationEntry {
        kind,
        name,
        detail,
        scope: target.scope,
        source_path: source_path.to_string_lossy().into_owned(),
        error: error.map(str::to_owned),
    }
}

fn scan_rules(target: ScanTarget<'_>, collector: &mut Collector) {
    for name in target.rules_files {
        let file = target.rules_directory.join(name);
        let Some(relative) = relative_string(target.root, &file) else {
            continue;
        };
        if is_search_excluded_path(&relative, false) {
            continue;
        }
        let read = read_bounded_file(&file, target.root);
        if matches!(read, FileRead::Missing) {
            continue;
        }
        let (detail, error) = match &read {
            FileRead::Read(text) => (rules_summary(text), None),
            FileRead::Failed(error) => {
                collector.truncate();
                (String::new(), Some(*error))
            }
            FileRead::Missing => unreachable!(),
        };
        if !collector.add(entry(
            CustomizationKind::Rules,
            (*name).to_owned(),
            detail,
            target,
            &file,
            error,
        )) {
            return;
        }
    }
}

fn read_json_config(
    file: &Path,
    kind: CustomizationKind,
    name: &str,
    target: ScanTarget<'_>,
    collector: &mut Collector,
) -> Option<Value> {
    let relative = relative_string(target.root, file)?;
    if is_search_excluded_path(&relative, false) {
        return None;
    }
    match read_bounded_file(file, target.root) {
        FileRead::Missing => None,
        FileRead::Failed(error) => {
            collector.truncate();
            collector.add(entry(
                kind,
                name.to_owned(),
                String::new(),
                target,
                file,
                Some(error),
            ));
            None
        }
        FileRead::Read(text) => {
            if let Ok(value) = serde_json::from_str(&text) {
                Some(value)
            } else {
                collector.add(entry(
                    kind,
                    name.to_owned(),
                    String::new(),
                    target,
                    file,
                    Some("Could not be parsed as JSON."),
                ));
                None
            }
        }
    }
}

fn collect_hooks(value: &Value, file: &Path, target: ScanTarget<'_>, collector: &mut Collector) {
    let Some(hooks) = value
        .as_object()
        .and_then(|settings| settings.get("hooks"))
        .and_then(Value::as_object)
    else {
        return;
    };
    for (event, raw) in hooks {
        let Some(groups) = raw.as_array() else {
            continue;
        };
        for group in groups {
            let matcher = group
                .as_object()
                .and_then(|value| value.get("matcher"))
                .and_then(Value::as_str)
                .map_or_else(String::new, |value| clip(value, 40));
            if !collector.add(entry(
                CustomizationKind::Hook,
                clip(event, 60),
                matcher,
                target,
                file,
                None,
            )) {
                return;
            }
        }
    }
}

fn origin_of(raw: &str) -> String {
    if raw.is_empty() || raw.len() > 2_048 {
        return String::new();
    }
    Url::parse(raw).map_or_else(
        |_| String::new(),
        |parsed| match parsed.scheme() {
            "http" | "https" => parsed.origin().ascii_serialization(),
            _ => String::new(),
        },
    )
}

fn collect_mcp(value: &Value, file: &Path, target: ScanTarget<'_>, collector: &mut Collector) {
    let Some(servers) = value
        .as_object()
        .and_then(|settings| settings.get("mcpServers"))
        .and_then(Value::as_object)
    else {
        return;
    };
    for (name, raw) in servers {
        let Some(server) = raw.as_object() else {
            continue;
        };
        let declared = server
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| server.get("transport").and_then(Value::as_str))
            .unwrap_or_default();
        let raw_url = server
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let transport = if declared.is_empty() {
            if raw_url.is_empty() { "stdio" } else { "http" }
        } else {
            declared
        };
        let origin = origin_of(raw_url);
        let detail = if origin.is_empty() {
            clip(transport, 16)
        } else {
            format!("{} · {origin}", clip(transport, 16))
        };
        if !collector.add(entry(
            CustomizationKind::Mcp,
            clip(name, 60),
            detail,
            target,
            file,
            None,
        )) {
            return;
        }
    }
}

fn scan_settings(target: ScanTarget<'_>, collector: &mut Collector) {
    for name in target.settings_files {
        let file = target.config_directory.join(name);
        let Some(value) = read_json_config(&file, CustomizationKind::Hook, name, target, collector)
        else {
            continue;
        };
        collect_hooks(&value, &file, target, collector);
        collect_mcp(&value, &file, target, collector);
    }
}

fn scan_mcp_files(target: ScanTarget<'_>, collector: &mut Collector) {
    for name in target.mcp_files {
        let file = target.rules_directory.join(name);
        let Some(value) = read_json_config(&file, CustomizationKind::Mcp, name, target, collector)
        else {
            continue;
        };
        collect_mcp(&value, &file, target, collector);
    }
}

fn scan_skills(
    directory: &Path,
    target: ScanTarget<'_>,
    collector: &mut Collector,
    depth: usize,
) -> bool {
    for child in safe_children(directory, target.root, collector) {
        if !child.directory {
            continue;
        }
        let skill_directory = directory.join(&child.name);
        let file = skill_directory.join("SKILL.md");
        match read_bounded_file(&file, target.root) {
            FileRead::Missing => {}
            FileRead::Read(text) => {
                let fields = read_frontmatter(&text);
                let name = fields
                    .as_ref()
                    .and_then(|fields| fields.get("name"))
                    .filter(|name| !name.is_empty())
                    .map_or_else(|| child.name.clone(), |name| clip(name, 60));
                let detail = fields
                    .as_ref()
                    .and_then(|fields| fields.get("description"))
                    .map_or_else(String::new, |description| {
                        clip(description, CUSTOMIZATION_LIMITS.max_detail_chars)
                    });
                let error = fields
                    .is_none()
                    .then_some("No frontmatter block; named for its folder.");
                if !collector.add(entry(
                    CustomizationKind::Skill,
                    name,
                    detail,
                    target,
                    &file,
                    error,
                )) {
                    return false;
                }
            }
            FileRead::Failed(error) => {
                collector.truncate();
                if !collector.add(entry(
                    CustomizationKind::Skill,
                    child.name.clone(),
                    String::new(),
                    target,
                    &file,
                    Some(error),
                )) {
                    return false;
                }
            }
        }
        if depth < CUSTOMIZATION_LIMITS.max_skill_depth
            && !scan_skills(&skill_directory, target, collector, depth + 1)
        {
            return false;
        }
    }
    true
}

fn scan_commands(directory: &Path, target: ScanTarget<'_>, collector: &mut Collector) {
    for child in safe_children(directory, target.root, collector) {
        if child.directory || !child.name.to_ascii_lowercase().ends_with(".md") {
            continue;
        }
        let command_name = child.name[..child.name.len() - 3].to_owned();
        if !collector.add(entry(
            CustomizationKind::Command,
            command_name,
            String::new(),
            target,
            &directory.join(&child.name),
            None,
        )) {
            return;
        }
    }
}

fn scan_target(target: ScanTarget<'_>, collector: &mut Collector) {
    scan_rules(target, collector);
    scan_settings(target, collector);
    scan_mcp_files(target, collector);
    let _ = scan_skills(
        &target.config_directory.join("skills"),
        target,
        collector,
        1,
    );
    scan_commands(&target.config_directory.join("commands"), target, collector);
}

fn canonical_or_absolute(target: &Path) -> PathBuf {
    fs::canonicalize(target).unwrap_or_else(|_| {
        let absolute = if target.is_absolute() {
            target.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join(target)
        };
        lexically_normalized(&absolute)
    })
}

/// Reports the bounded customization surface inherited by a project. The
/// settings parser deliberately emits only hook event/matcher names and MCP
/// name/transport/origin; command argv, environment, headers, URL paths, query
/// strings, and skill bodies never enter the result.
#[must_use]
pub fn read_customization_inventory(
    root_path: &Path,
    user_root_path: Option<&Path>,
) -> CustomizationInventory {
    let root = canonical_or_absolute(root_path);
    let requested_user =
        user_root_path.map_or_else(default_customization_user_root, Path::to_path_buf);
    let user_root = canonical_or_absolute(&requested_user);
    let mut collector = Collector::new();

    let project_config = root.join(".claude");
    scan_target(
        ScanTarget {
            scope: CustomizationScope::Project,
            root: &root,
            rules_directory: &root,
            config_directory: &project_config,
            rules_files: &["CLAUDE.md", "AGENTS.md"],
            settings_files: &["settings.json", "settings.local.json"],
            mcp_files: &[".mcp.json"],
        },
        &mut collector,
    );

    if user_root != root && user_root != project_config {
        scan_target(
            ScanTarget {
                scope: CustomizationScope::User,
                root: &user_root,
                rules_directory: &user_root,
                config_directory: &user_root,
                rules_files: &["CLAUDE.md"],
                settings_files: &["settings.json"],
                mcp_files: &[],
            },
            &mut collector,
        );
    }

    collector.result(&root)
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;

#[path = "inventory_providers.rs"]
pub mod providers;
