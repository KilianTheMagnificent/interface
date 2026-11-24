//! Read-only, dependency-free task routing over a checked repository map.
//! Commands in the map are displayed, never executed. Paths have no authority.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

const MAP: &str = "docs/agent-map.json";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Map {
    schema_version: u8,
    routes: Vec<Route>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Route {
    id: String,
    summary: String,
    keywords: Vec<String>,
    roots: Vec<String>,
    rules: Vec<String>,
    guides: Vec<String>,
    entrypoints: Vec<String>,
    tests: Vec<String>,
    checks: Vec<String>,
    constraints: Vec<String>,
}

fn load(root: &Path) -> Result<Map> {
    validate_path(root, MAP)?;
    let map: Map = serde_json::from_slice(&fs::read(root.join(MAP))?)
        .with_context(|| format!("decode {MAP}"))?;
    ensure!(map.schema_version == 1, "unsupported navigation schema");
    ensure!(!map.routes.is_empty(), "navigation map has no routes");
    Ok(map)
}

fn validate_path(root: &Path, name: &str) -> Result<()> {
    ensure!(!name.is_empty(), "empty navigation path");
    let mut current = root.to_path_buf();
    for part in name.split('/') {
        let lower = part.to_ascii_lowercase();
        ensure!(
            !matches!(part, "" | "." | "..")
                && !part.contains(['\\', ':'])
                && !part.chars().any(char::is_control)
                && !matches!(
                    lower.as_str(),
                    ".git" | "node_modules" | "target" | ".worktrees"
                )
                && !interface_common::paths::is_sensitive_name(&lower),
            "navigation paths must be relative, non-sensitive source paths"
        );
        current.push(part);
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("navigation path missing: {name}"))?;
        ensure!(
            metadata.is_file() || metadata.is_dir(),
            "navigation path must not traverse symlinks or special files: {name}"
        );
    }
    Ok(())
}

fn validate(root: &Path, map: &Map, members: &BTreeSet<String>) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    for route in &map.routes {
        ensure!(
            !route.id.is_empty()
                && route
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                && ids.insert(&route.id),
            "navigation route id must be unique lowercase kebab case: {}",
            route.id
        );
        ensure!(
            !route.summary.trim().is_empty(),
            "{}: missing summary",
            route.id
        );
        for (label, values) in [
            ("keywords", &route.keywords),
            ("roots", &route.roots),
            ("rules", &route.rules),
            ("guides", &route.guides),
            ("entrypoints", &route.entrypoints),
            ("tests", &route.tests),
            ("checks", &route.checks),
            ("constraints", &route.constraints),
        ] {
            ensure!(
                !values.is_empty() && values.iter().all(|v| !v.trim().is_empty()),
                "{}: missing {label}",
                route.id
            );
        }
        ensure!(
            route.rules.iter().any(|p| p == "AGENTS.md"),
            "{}: root rules missing",
            route.id
        );
        for path in route
            .roots
            .iter()
            .chain(&route.rules)
            .chain(&route.guides)
            .chain(&route.entrypoints)
            .chain(&route.tests)
        {
            validate_path(root, path).with_context(|| format!("route {}", route.id))?;
        }
        for path in &route.roots {
            ensure!(
                root.join(path).is_dir(),
                "{}: root is not a directory: {path}",
                route.id
            );
            covered.insert(path.clone());
        }
        for path in route
            .rules
            .iter()
            .chain(&route.guides)
            .chain(&route.entrypoints)
            .chain(&route.tests)
        {
            ensure!(
                root.join(path).is_file(),
                "{}: expected file: {path}",
                route.id
            );
        }
        // Every existing ancestor rule must be explicit even when a harness only
        // discovers its nearest AGENTS.md. File references are not auto-imports.
        for source in route.entrypoints.iter().chain(&route.tests) {
            let mut parent = Path::new(source).parent();
            while let Some(dir) = parent {
                let rules = dir.join("AGENTS.md");
                if root.join(&rules).is_file() {
                    let rules = rules.to_str().context("non-UTF8 rule path")?;
                    ensure!(
                        route.rules.iter().any(|p| p == rules),
                        "{}: missing rules {rules}",
                        route.id
                    );
                }
                parent = dir.parent();
            }
        }
    }
    let missing: Vec<_> = members.difference(&covered).collect();
    ensure!(
        missing.is_empty(),
        "Cargo workspace members missing from navigation: {missing:?}"
    );
    Ok(())
}

fn workspace_members(root: &Path) -> Result<BTreeSet<String>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--offline",
            "--locked",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(root)
        .output()
        .context("read Cargo workspace metadata")?;
    ensure!(
        output.status.success(),
        "cargo metadata failed; run it directly to diagnose"
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let members = metadata["workspace_members"]
        .as_array()
        .context("workspace members missing")?;
    let packages = metadata["packages"]
        .as_array()
        .context("workspace packages missing")?;
    let canonical_root = root.canonicalize()?;
    packages
        .iter()
        .filter(|package| members.contains(&package["id"]))
        .map(|package| {
            let manifest = package["manifest_path"]
                .as_str()
                .context("manifest path missing")?;
            let directory = Path::new(manifest)
                .parent()
                .context("manifest directory missing")?
                .canonicalize()?;
            Ok(directory
                .strip_prefix(&canonical_root)?
                .to_str()
                .context("non-UTF8 workspace member")?
                .to_owned())
        })
        .collect()
}

fn matching<'a>(map: &'a Map, query: &str) -> Vec<&'a Route> {
    let query = query.trim().trim_end_matches('/').to_ascii_lowercase();
    let mut matches: Vec<_> = map
        .routes
        .iter()
        .filter_map(|route| {
            let searchable = std::iter::once(route.id.as_str())
                .chain(std::iter::once(route.summary.as_str()))
                .chain(
                    route
                        .keywords
                        .iter()
                        .chain(&route.roots)
                        .chain(&route.entrypoints)
                        .map(String::as_str),
                )
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            let direct = route.id == query
                || route
                    .roots
                    .iter()
                    .any(|p| query == *p || query.starts_with(&format!("{p}/")));
            let matched = query
                .split_whitespace()
                .all(|word| searchable.contains(word));
            (direct || matched).then_some((!direct, route))
        })
        .collect();
    matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
    matches.into_iter().map(|(_, route)| route).collect()
}

pub fn check(root: &Path) -> Result<()> {
    let map = load(root)?;
    let members = workspace_members(root)?;
    validate(root, &map, &members)?;
    println!(
        "navigation: {} routes, {} Cargo members, paths and scoped rules verified",
        map.routes.len(),
        members.len()
    );
    Ok(())
}

pub fn run(root: &Path, query: Option<&str>, json: bool, check_only: bool) -> Result<()> {
    let map = load(root)?;
    if check_only {
        let members = workspace_members(root)?;
        validate(root, &map, &members)?;
        if json {
            println!(
                "{}",
                serde_json::json!({"schemaVersion": 1, "status": "ok", "routes": map.routes.len(), "workspaceMembers": members.len()})
            );
        } else {
            println!(
                "navigation: {} routes, {} Cargo members verified",
                map.routes.len(),
                members.len()
            );
        }
        return Ok(());
    }
    // Reject stale instructions before returning a packet. Cargo coverage is
    // reserved for --check so ordinary lookups never spawn a subprocess.
    validate(root, &map, &BTreeSet::new())?;
    let routes = matching(&map, query.unwrap_or_default());
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"schemaVersion": 1, "query": query, "matches": routes})
            )?
        );
    } else if query.is_none() {
        for route in routes {
            println!("{} — {}", route.id, route.summary);
        }
        println!(
            "\nUse navigate '<terms or path>' [--json]. Read docs/AGENT_TASKS.md before editing."
        );
    } else {
        ensure!(
            !routes.is_empty(),
            "no navigation match; run cargo xtask navigate to list route ids"
        );
        for route in routes {
            println!("\n{} — {}", route.id, route.summary);
            for (label, values) in [
                ("Read rules", &route.rules),
                ("Guide", &route.guides),
                ("Start", &route.entrypoints),
                ("Tests", &route.tests),
                ("Focused checks (from repo root)", &route.checks),
                ("Preserve", &route.constraints),
            ] {
                println!("  {label}: {}", values.join("; "));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "navigation_tests.rs"]
mod tests;
