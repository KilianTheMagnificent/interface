use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliPaths {
    pub data_dir: PathBuf,
    pub instance_lock: PathBuf,
    pub agent_socket: PathBuf,
    pub agent_names: PathBuf,
    pub registry: PathBuf,
    pub worktrees: PathBuf,
    pub review_checkpoints: PathBuf,
    pub acp_profiles: PathBuf,
    pub config: PathBuf,
    pub history: PathBuf,
    pub steers: PathBuf,
    pub logs: PathBuf,
    pub background_sessions: PathBuf,
    pub collaboration: PathBuf,
}

/// Resolves every CLI-owned path without touching the filesystem.
#[must_use]
pub fn resolve_cli_paths(configured: Option<OsString>, home: &std::path::Path) -> CliPaths {
    let configured = configured.and_then(|value| {
        let value = value.to_string_lossy().trim().to_owned();
        (!value.is_empty()).then_some(value)
    });
    let unresolved = configured.map_or_else(|| home.join(".config").join("iface"), PathBuf::from);
    let data_dir = std::path::absolute(&unresolved).unwrap_or(unresolved);
    CliPaths {
        instance_lock: data_dir.join("instance.lock"),
        agent_socket: data_dir.join("iface.sock"),
        agent_names: data_dir.join("agents.json"),
        registry: data_dir.join("registry.json"),
        worktrees: data_dir.join("worktrees.json"),
        review_checkpoints: data_dir.join("review-checkpoints"),
        acp_profiles: data_dir.join("acp-profiles.json"),
        config: data_dir.join("config.json"),
        history: data_dir.join("history"),
        steers: data_dir.join("steers.json"),
        logs: data_dir.join("logs"),
        background_sessions: data_dir.join("bg-sessions"),
        collaboration: data_dir.join("collaboration"),
        data_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_matches_the_existing_iface_store() {
        let paths = resolve_cli_paths(None, std::path::Path::new("/Users/tester"));
        assert_eq!(paths.data_dir, PathBuf::from("/Users/tester/.config/iface"));
        assert_eq!(paths.agent_socket, paths.data_dir.join("iface.sock"));
        assert_eq!(
            paths.background_sessions,
            paths.data_dir.join("bg-sessions")
        );
    }

    #[test]
    fn explicit_data_directory_keeps_the_same_child_names() {
        let paths = resolve_cli_paths(Some("/private/iface".into()), std::path::Path::new("/home"));
        assert_eq!(paths.config, PathBuf::from("/private/iface/config.json"));
        assert_eq!(
            paths.collaboration,
            PathBuf::from("/private/iface/collaboration")
        );
    }
}
