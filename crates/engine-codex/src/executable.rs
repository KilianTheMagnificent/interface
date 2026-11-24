use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use interface_contracts::engine::{COLLABORATION_MCP_TOOL_NAMES, CollaborationMcpStdioDescriptor};
use interface_engine::{ExecutableResolution, ProcessSpec, ResolvedExecutable, resolve_executable};
use thiserror::Error;

pub const CODEX_EXECUTABLE_ENV: &str = "CODEX_EXECUTABLE";

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum CodexLaunchError {
    #[error("Codex executable path must be absolute")]
    ExecutableNotAbsolute,
    #[error("Codex collaboration descriptor is invalid")]
    InvalidCollaborationDescriptor,
}

/// Resolve override, login-shell result, then known install paths.
pub fn resolve_codex_executable(
    home: &Path,
    explicit_override: Option<PathBuf>,
    login_shell_path: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Option<ResolvedExecutable> {
    let resolution = ExecutableResolution {
        explicit_override,
        login_shell_path,
        known_locations: vec![
            home.join(".local/bin/codex"),
            PathBuf::from("/opt/homebrew/bin/codex"),
            PathBuf::from("/usr/local/bin/codex"),
        ],
        require_absolute: true,
    };
    resolve_executable(&resolution, exists)
}

/// Exact app-server argv. `-c` belongs to Codex, not a command shell.
#[must_use]
fn codex_app_server_args() -> Vec<OsString> {
    ["app-server", "-c", "features.plugins=false"]
        .map(OsString::from)
        .into()
}

/// Codex features that are on by default and act on the world (the built-in
/// connector apps, the browser and computer tools, the in-app browser,
/// sub-agents) or feed a running exec, and that Codex runs without ever raising
/// `approval.requested`. A gated agent launch must not have them: they would
/// bypass the approval gate. `worker.rs` disables the same shapes for
/// restricted one-shot workers.
///
/// `code_mode_host` stays on: in Codex 0.154.0 it carries command execution
/// itself, and a gated live probe with it off had every command refused as
/// "code-mode host is disabled" without raising an approval.
pub const GATED_DISABLED_FEATURES: &[&str] = &[
    "apps",
    "browser_use",
    "computer_use",
    "hooks",
    "in_app_browser",
    "multi_agent",
    "multi_agent_v2",
    "unified_exec",
    "unified_exec_tty",
    "workspace_dependencies",
];

/// The `-c features.<name>=false` argv pairs that turn [`GATED_DISABLED_FEATURES`]
/// off, appended to a gated launch's `app-server` argv (the same per-process
/// `features.X=false` form the frozen `features.plugins=false` already uses).
#[must_use]
pub fn codex_gated_feature_args() -> Vec<OsString> {
    GATED_DISABLED_FEATURES
        .iter()
        .flat_map(|feature| {
            [
                OsString::from("-c"),
                OsString::from(format!("features.{feature}=false")),
            ]
        })
        .collect()
}

/// Exact app-server argv with the frozen required collaboration MCP override.
/// The inline TOML is one inert `-c` argument; no shell or config file is used.
///
/// # Errors
///
/// Refuses an invalid or environment-bearing collaboration descriptor.
pub fn codex_app_server_args_with_collaboration(
    input: Option<&CollaborationMcpStdioDescriptor>,
) -> Result<Vec<OsString>, CodexLaunchError> {
    let mut args = codex_app_server_args();
    let Some(descriptor) = input else {
        return Ok(args);
    };
    descriptor
        .validate()
        .map_err(|_| CodexLaunchError::InvalidCollaborationDescriptor)?;
    let command = serde_json::to_string(&descriptor.command)
        .map_err(|_| CodexLaunchError::InvalidCollaborationDescriptor)?;
    let descriptor_args = descriptor
        .args
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CodexLaunchError::InvalidCollaborationDescriptor)?
        .join(",");
    let enabled_tools = COLLABORATION_MCP_TOOL_NAMES
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CodexLaunchError::InvalidCollaborationDescriptor)?
        .join(",");
    let config = format!(
        "mcp_servers.{}={{command={},args={},env={{}},enabled_tools=[{}],required=true,startup_timeout_sec=5,tool_timeout_sec=120,default_tools_approval_mode=\"prompt\"}}",
        descriptor.name,
        command,
        format_args!("[{descriptor_args}]"),
        enabled_tools,
    );
    args.extend([OsString::from("-c"), OsString::from(config)]);
    Ok(args)
}

/// Build an argv-only process spec. No `sh -c`, interpolation, or string join.
///
/// # Errors
///
/// Returns `CodexLaunchError::ExecutableNotAbsolute` for unresolved relative programs.
pub(crate) fn codex_app_server_process(executable: &Path) -> Result<ProcessSpec, CodexLaunchError> {
    codex_app_server_process_with_collaboration(executable, None)
}

/// Build an argv-only process spec with an optional required collaboration MCP.
///
/// # Errors
///
/// Refuses unresolved relative programs and invalid collaboration descriptors.
pub fn codex_app_server_process_with_collaboration(
    executable: &Path,
    collaboration: Option<&CollaborationMcpStdioDescriptor>,
) -> Result<ProcessSpec, CodexLaunchError> {
    if !executable.is_absolute() {
        return Err(CodexLaunchError::ExecutableNotAbsolute);
    }
    Ok(ProcessSpec {
        program: executable.to_path_buf(),
        args: codex_app_server_args_with_collaboration(collaboration)?,
        cwd: None,
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use interface_engine::{ExecutableSource, provider_search_path};

    use super::*;

    #[test]
    fn resolver_uses_login_shell_before_known_locations() {
        let result = resolve_codex_executable(
            Path::new("/Users/test"),
            Some("/stale/codex".into()),
            Some("/shell/codex".into()),
            |path| {
                matches!(
                    path.to_str(),
                    Some("/shell/codex" | "/opt/homebrew/bin/codex")
                )
            },
        )
        .unwrap();
        assert_eq!(result.path, PathBuf::from("/shell/codex"));
        assert_eq!(result.source, ExecutableSource::LoginShellPath);
    }

    #[test]
    fn search_path_prepends_the_exact_codex_toolchain_and_keeps_the_inherited_tail() {
        let search_path = provider_search_path(
            Path::new("/resolved/bin/codex"),
            Some(OsStr::new("/usr/bin:/usr/local/bin")),
            Some(Path::new("/Users/test")),
        )
        .unwrap();
        assert_eq!(
            search_path,
            OsString::from(
                "/resolved/bin:/Users/test/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin"
            )
        );
    }

    #[test]
    fn search_path_omits_the_home_entry_when_home_is_absent_or_empty() {
        let absent = provider_search_path(Path::new("/resolved/bin/codex"), None, None).unwrap();
        assert_eq!(
            absent,
            OsString::from("/resolved/bin:/opt/homebrew/bin:/usr/local/bin")
        );
        let empty =
            provider_search_path(Path::new("/resolved/bin/codex"), None, Some(Path::new("")))
                .unwrap();
        assert_eq!(empty, absent);
    }

    #[test]
    fn app_server_launch_is_an_exact_no_shell_argv() {
        let spec = codex_app_server_process(Path::new("/Applications/bin/codex")).unwrap();
        assert_eq!(spec.program, PathBuf::from("/Applications/bin/codex"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("app-server"),
                OsString::from("-c"),
                OsString::from("features.plugins=false"),
            ]
        );
        assert!(!spec.args.iter().any(|arg| arg == "-lic" || arg == "sh"));
    }

    #[test]
    fn gated_feature_args_disable_every_acting_feature_as_inline_config() {
        let args = codex_gated_feature_args();
        assert_eq!(args.len(), GATED_DISABLED_FEATURES.len() * 2);
        let mut chunks = args.chunks_exact(2);
        for feature in GATED_DISABLED_FEATURES {
            let chunk = chunks.next().expect("a -c pair per feature");
            assert_eq!(chunk[0], OsString::from("-c"));
            assert_eq!(
                chunk[1],
                OsString::from(format!("features.{feature}=false"))
            );
        }
        assert!(chunks.remainder().is_empty());
        assert!(
            GATED_DISABLED_FEATURES.contains(&"unified_exec")
                && GATED_DISABLED_FEATURES.contains(&"apps")
                && GATED_DISABLED_FEATURES.contains(&"browser_use")
                && GATED_DISABLED_FEATURES.contains(&"hooks")
                && GATED_DISABLED_FEATURES.contains(&"multi_agent_v2")
                && GATED_DISABLED_FEATURES.contains(&"workspace_dependencies")
        );
        // Command execution runs through the code-mode host; turning it off leaves a gated
        // agent unable to run any command.
        assert!(!GATED_DISABLED_FEATURES.contains(&"code_mode_host"));
    }

    #[test]
    fn collaboration_override_is_one_env_free_required_inline_toml_argument() {
        let descriptor = CollaborationMcpStdioDescriptor {
            name: "iface_collaboration".into(),
            command: "/usr/bin/nc".into(),
            args: vec!["-U".into(), "/tmp/interface broker.sock".into()],
            env: Vec::new(),
        };
        let args = codex_app_server_args_with_collaboration(Some(&descriptor)).unwrap();
        assert_eq!(
            &args[..3],
            [
                OsString::from("app-server"),
                OsString::from("-c"),
                OsString::from("features.plugins=false"),
            ]
        );
        assert_eq!(args[3], "-c");
        let config = args[4].to_string_lossy();
        assert!(config.starts_with("mcp_servers.iface_collaboration={"));
        assert!(config.contains("command=\"/usr/bin/nc\""));
        assert!(config.contains("args=[\"-U\",\"/tmp/interface broker.sock\"]"));
        assert!(config.contains("env={}"));
        assert!(config.contains("required=true"));
        assert!(config.contains("startup_timeout_sec=5"));
        assert!(config.contains("tool_timeout_sec=120"));
        for tool in COLLABORATION_MCP_TOOL_NAMES {
            assert!(config.contains(&format!("\"{tool}\"")));
        }
        assert!(
            !args
                .iter()
                .any(|argument| argument == "sh" || argument == "-lic")
        );
    }

    #[test]
    fn unresolved_relative_program_is_refused() {
        assert_eq!(
            codex_app_server_process(Path::new("codex")),
            Err(CodexLaunchError::ExecutableNotAbsolute)
        );
    }
}
