//! Private process-local wrappers used by login-shell Claude and Codex
//! invocations.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use interface_terminal::{CODEX_SESSION_CAPTURE_TOKEN_ENV, current_terminal_process_identity};
use std::os::unix::{fs::MetadataExt, fs::PermissionsExt, process::CommandExt};

use crate::bootstrap::{
    PaneToolApproval, claude_pane_mcp_config, codex_terminal_capture_arguments,
};
use crate::provider_capture::claim_manual_nested_capture_process;

pub const CLAUDE_MANUAL_WRAPPER_MODE: &str = "--terminal-manual-claude-wrapper";
pub const CLAUDE_MANUAL_EXECUTABLE_ENV: &str = "INTERFACE_MANUAL_CLAUDE_EXECUTABLE";
pub const CLAUDE_MANUAL_SETTINGS_ENV: &str = "INTERFACE_MANUAL_CLAUDE_SETTINGS";
pub const CLAUDE_MANUAL_HOST_ENV: &str = "INTERFACE_MANUAL_CLAUDE_HOST";
pub const CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV: &str = "INTERFACE_MANUAL_CLAUDE_OVERLAY_ZDOTDIR";
pub const CLAUDE_MANUAL_USER_ZDOTDIR_ENV: &str = "INTERFACE_MANUAL_CLAUDE_USER_ZDOTDIR";
pub const CLAUDE_MANUAL_CAPTURE_TOKEN_ENV: &str = "INTERFACE_MANUAL_CLAUDE_CAPTURE_TOKEN";
pub const CLAUDE_MANUAL_RECEIPTS_ROOT_ENV: &str = "INTERFACE_MANUAL_CLAUDE_RECEIPTS_ROOT";
pub const CODEX_MANUAL_WRAPPER_MODE: &str = "--terminal-manual-codex-wrapper";
pub const CODEX_MANUAL_EXECUTABLE_ENV: &str = "INTERFACE_MANUAL_CODEX_EXECUTABLE";

/// Everything the armed shell exports for these wrappers; none of it reaches
/// the provider.
pub(crate) const MANUAL_ENVIRONMENT: [&str; 9] = [
    CLAUDE_MANUAL_EXECUTABLE_ENV,
    CLAUDE_MANUAL_SETTINGS_ENV,
    CLAUDE_MANUAL_HOST_ENV,
    CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV,
    CLAUDE_MANUAL_USER_ZDOTDIR_ENV,
    CLAUDE_MANUAL_CAPTURE_TOKEN_ENV,
    CLAUDE_MANUAL_RECEIPTS_ROOT_ENV,
    CODEX_MANUAL_EXECUTABLE_ENV,
    PaneToolApproval::ENV,
];

const MAX_SETTINGS_BYTES: usize = 16 * 1024;

#[must_use]
pub fn manual_claude_wrapper_requested() -> bool {
    std::env::args_os().nth(1).as_deref() == Some(OsStr::new(CLAUDE_MANUAL_WRAPPER_MODE))
}

#[must_use]
pub fn manual_codex_wrapper_requested() -> bool {
    std::env::args_os().nth(1).as_deref() == Some(OsStr::new(CODEX_MANUAL_WRAPPER_MODE))
}

/// Replace this helper with the host-pinned Codex executable, carrying the
/// same inline `SessionStart` hook an integrated Codex pane gets, so the
/// thread this shell opens is filed against the pane's capture generation and
/// the pane reopens on it.
///
/// # Errors
///
/// Refuses missing or mutable-looking wrapper inputs and returns only when the
/// operating-system exec itself fails.
pub fn run_manual_codex_wrapper() -> Result<(), String> {
    let executable = required_environment_path(CODEX_MANUAL_EXECUTABLE_ENV)?;
    refuse_recursion("Codex", &executable)?;
    let (capture_token, receipts_root) = capture_inputs("Codex")?;
    // Built from the exported path verbatim: the hook command has to match
    // the integrated pane's byte for byte, or Codex asks for trust again.
    // The pane socket rides the shell pane's environment; a Codex started
    // there gets the same pane tools as a curated Codex pane.
    let pane_mcp_socket = std::env::var_os(crate::pane_mcp::PANE_MCP_SOCKET_ENV).map(PathBuf::from);
    // The host exports the owner's "Pane tools without asking" switch only
    // when it is on; absent, the pane tools that act on other panes ask.
    let pane_tools =
        PaneToolApproval::from_environment(std::env::var_os(PaneToolApproval::ENV).as_deref());
    let capture_arguments = codex_terminal_capture_arguments(
        Path::new(&std::env::var_os(CLAUDE_MANUAL_RECEIPTS_ROOT_ENV).unwrap_or_default()),
        pane_mcp_socket.as_deref(),
        pane_tools,
    )
    .map_err(|error| format!("manual Codex capture was unavailable: {error}"))?;
    let arguments = std::env::args_os().skip(2).collect::<Vec<_>>();
    let command = manual_codex_command(&executable, &capture_arguments, &capture_token, &arguments);
    claim_and_exec("Codex", command, &capture_token, &receipts_root)
}

/// Replace this helper with the host-pinned Claude executable while retaining
/// the foreground PID/process-group identity the terminal owner will verify.
///
/// # Errors
///
/// Refuses missing or mutable-looking wrapper inputs and returns only when the
/// operating-system exec itself fails.
pub fn run_manual_claude_wrapper() -> Result<(), String> {
    let executable = required_environment_path(CLAUDE_MANUAL_EXECUTABLE_ENV)?;
    let settings = std::env::var_os(CLAUDE_MANUAL_SETTINGS_ENV)
        .filter(|settings| {
            !settings.is_empty() && settings.as_encoded_bytes().len() <= MAX_SETTINGS_BYTES
        })
        .ok_or_else(|| "manual Claude capture settings were unavailable".to_owned())?;
    refuse_recursion("Claude", &executable)?;
    let arguments = std::env::args_os().skip(2).collect::<Vec<_>>();
    validate_capture_compatible_arguments(&arguments)?;
    let pane_mcp_config = std::env::var_os(crate::pane_mcp::PANE_MCP_SOCKET_ENV)
        .map(|socket| claude_pane_mcp_config(Path::new(&socket)))
        .transpose()
        .map_err(|error| format!("manual Claude pane tools were unavailable: {error}"))?;
    let command = manual_claude_command(
        &executable,
        &settings,
        pane_mcp_config.as_deref(),
        &arguments,
    );
    let (capture_token, receipts_root) = capture_inputs("Claude")?;
    claim_and_exec("Claude", command, &capture_token, &receipts_root)
}

/// Refuses a provider executable that is this host itself, which would make
/// the wrapper exec into its own wrapper mode forever.
fn refuse_recursion(provider: &str, executable: &Path) -> Result<(), String> {
    let host = std::fs::canonicalize(
        std::env::current_exe().map_err(|_| "the Interface host path was unavailable")?,
    )
    .map_err(|_| "the Interface host path was unsafe")?;
    if same_executable_file(executable, &host)? {
        return Err(format!(
            "the manual {provider} wrapper refused recursive execution"
        ));
    }
    Ok(())
}

/// The shell pane's capture token and the canonical receipts root it names.
fn capture_inputs(provider: &str) -> Result<(String, PathBuf), String> {
    let capture_token = std::env::var(CLAUDE_MANUAL_CAPTURE_TOKEN_ENV)
        .map_err(|_| format!("manual {provider} capture token was unavailable"))?;
    let receipts_root = required_environment_directory(CLAUDE_MANUAL_RECEIPTS_ROOT_ENV)?;
    Ok((capture_token, receipts_root))
}

/// Claims the pane's capture generation for this exact process in its
/// canonical cwd, then replaces this process with the provider. Returns only
/// when a step is refused or the exec itself fails.
fn claim_and_exec(
    provider: &str,
    mut command: Command,
    capture_token: &str,
    receipts_root: &Path,
) -> Result<(), String> {
    let cwd = std::fs::canonicalize(
        std::env::current_dir()
            .map_err(|_| format!("the manual {provider} cwd was unavailable"))?,
    )
    .map_err(|_| format!("the manual {provider} cwd was unsafe"))?;
    let process = current_terminal_process_identity()
        .map_err(|error| format!("the manual {provider} process was unsafe: {error}"))?;
    claim_manual_nested_capture_process(receipts_root, capture_token, &process, &cwd)
        .map_err(|_| format!("manual {provider} capture ownership was unavailable"))?;

    let error = command.exec();
    Err(format!("{provider} could not start: {error}"))
}

fn validate_capture_compatible_arguments(arguments: &[std::ffi::OsString]) -> Result<(), String> {
    for argument in arguments
        .iter()
        .take_while(|argument| argument.as_os_str() != "--")
    {
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if argument == "--settings" || argument.starts_with("--settings=") {
            return Err(
                "manual Claude capture cannot be combined with another --settings option"
                    .to_owned(),
            );
        }
        if matches!(
            argument,
            "--bare" | "--safe-mode" | "--no-session-persistence"
        ) {
            return Err(format!(
                "manual Claude capture cannot be combined with {argument}"
            ));
        }
    }
    Ok(())
}

fn manual_claude_command(
    executable: &Path,
    settings: &OsStr,
    pane_mcp_config: Option<&OsStr>,
    arguments: &[std::ffi::OsString],
) -> Command {
    let mut command = Command::new(executable);
    // The single-valued settings flag terminates Claude's variadic MCP flag,
    // so the user's positional prompt remains a prompt.
    if let Some(config) = pane_mcp_config {
        command.arg("--mcp-config").arg(config);
    }
    command.arg("--settings").arg(settings).args(arguments);
    for name in MANUAL_ENVIRONMENT {
        command.env_remove(name);
    }
    command
}

/// The capture token rides the environment because Codex trusts a hook by
/// its command text, which therefore stays identical for every pane.
fn manual_codex_command(
    executable: &Path,
    capture_arguments: &[OsString],
    capture_token: &str,
    arguments: &[OsString],
) -> Command {
    let mut command = Command::new(executable);
    command.args(capture_arguments).args(arguments);
    for name in MANUAL_ENVIRONMENT {
        command.env_remove(name);
    }
    command.env(CODEX_SESSION_CAPTURE_TOKEN_ENV, capture_token);
    command
}

fn same_executable_file(left: &Path, right: &Path) -> Result<bool, String> {
    if left == right {
        return Ok(true);
    }
    let left = left
        .metadata()
        .map_err(|_| "the manual Claude executable identity was unavailable".to_owned())?;
    let right = right
        .metadata()
        .map_err(|_| "the Interface host identity was unavailable".to_owned())?;
    Ok(left.dev() == right.dev() && left.ino() == right.ino())
}

fn required_environment_directory(name: &str) -> Result<PathBuf, String> {
    let raw = std::env::var_os(name).ok_or_else(|| format!("{name} was unavailable"))?;
    let path = Path::new(&raw);
    if !path.is_absolute() {
        return Err(format!("{name} was not absolute"));
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| format!("{name} was unsafe"))?;
    canonical
        .is_dir()
        .then_some(canonical)
        .ok_or_else(|| format!("{name} was not a directory"))
}

fn required_environment_path(name: &str) -> Result<PathBuf, String> {
    let raw = std::env::var_os(name).ok_or_else(|| format!("{name} was unavailable"))?;
    let path = Path::new(&raw);
    if !path.is_absolute() {
        return Err(format!("{name} was not absolute"));
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| format!("{name} was unsafe"))?;
    let metadata = canonical
        .metadata()
        .map_err(|_| format!("{name} was unavailable"))?;
    let executable = metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
    executable
        .then_some(canonical)
        .ok_or_else(|| format!("{name} was not executable"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_mode_is_reserved_and_unambiguous() {
        assert!(CLAUDE_MANUAL_WRAPPER_MODE.starts_with("--terminal-"));
        assert_ne!(
            CLAUDE_MANUAL_WRAPPER_MODE,
            interface_terminal::CLAUDE_SESSION_HOOK_MODE
        );
    }

    #[test]
    fn settings_limit_is_bounded_below_the_terminal_environment_ceiling() {
        let max_settings_bytes = std::hint::black_box(MAX_SETTINGS_BYTES);
        assert!(max_settings_bytes <= 16 * 1024);
    }

    #[test]
    fn wrapper_rejects_arguments_that_disable_or_replace_capture() {
        for arguments in [
            vec![OsString::from("--settings"), OsString::from("{}")],
            vec![OsString::from("--settings={}")],
            vec![OsString::from("--bare")],
            vec![OsString::from("--safe-mode")],
            vec![OsString::from("--no-session-persistence")],
        ] {
            assert!(validate_capture_compatible_arguments(&arguments).is_err());
        }
        assert!(
            validate_capture_compatible_arguments(&[
                OsString::from("--"),
                OsString::from("--settings"),
            ])
            .is_ok()
        );
    }

    #[test]
    fn wrapper_command_preserves_arguments_and_strips_private_environment() {
        let arguments = vec![
            OsString::from("--resume"),
            OsString::from("12345678-1234-4234-8234-123456789abc"),
        ];
        let command =
            manual_claude_command(Path::new("/bin/echo"), OsStr::new("{}"), None, &arguments);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("--settings"),
                OsStr::new("{}"),
                OsStr::new("--resume"),
                OsStr::new("12345678-1234-4234-8234-123456789abc"),
            ]
        );
        let removed = command
            .get_envs()
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect::<Vec<_>>();
        for name in MANUAL_ENVIRONMENT {
            assert!(removed.contains(&OsStr::new(name)));
        }
    }

    #[test]
    fn manual_claude_gets_pane_tools_without_consuming_the_positional_prompt() {
        let socket = Path::new("/tmp/Interface test/panes/iface.sock");
        let config = claude_pane_mcp_config(socket).expect("inline pane MCP config");
        let prompt = OsString::from("Update the fixture after reading its test.");
        let command = manual_claude_command(
            Path::new("/bin/echo"),
            OsStr::new("{}"),
            Some(&config),
            std::slice::from_ref(&prompt),
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("--mcp-config"),
                config.as_os_str(),
                OsStr::new("--settings"),
                OsStr::new("{}"),
                prompt.as_os_str(),
            ]
        );
        let decoded: serde_json::Value =
            serde_json::from_str(config.to_str().expect("UTF-8 config")).expect("config JSON");
        assert_eq!(decoded["mcpServers"]["iface"]["command"], "/usr/bin/nc");
        assert_eq!(
            decoded["mcpServers"]["iface"]["args"],
            serde_json::json!(["-U", socket])
        );
        assert!(claude_pane_mcp_config(Path::new("relative.sock")).is_err());
    }

    #[test]
    fn codex_wrapper_leads_with_the_hook_and_passes_the_token_in_the_environment() {
        const TOKEN: &str = "12345678-1234-4234-8234-123456789abc";
        let capture = vec![OsString::from("-c"), OsString::from("features.hooks=true")];
        let arguments = vec![OsString::from("resume"), OsString::from("--last")];
        let command = manual_codex_command(Path::new("/bin/echo"), &capture, TOKEN, &arguments);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                OsStr::new("-c"),
                OsStr::new("features.hooks=true"),
                OsStr::new("resume"),
                OsStr::new("--last"),
            ]
        );
        let environment = command.get_envs().collect::<Vec<_>>();
        assert!(environment.contains(&(
            OsStr::new(CODEX_SESSION_CAPTURE_TOKEN_ENV),
            Some(OsStr::new(TOKEN))
        )));
        for name in MANUAL_ENVIRONMENT {
            assert!(environment.contains(&(OsStr::new(name), None)));
        }
        assert_ne!(CODEX_MANUAL_WRAPPER_MODE, CLAUDE_MANUAL_WRAPPER_MODE);
        assert_ne!(
            CODEX_MANUAL_WRAPPER_MODE,
            interface_terminal::CODEX_SESSION_HOOK_MODE
        );
    }

    #[test]
    fn recursion_guard_recognizes_a_hard_link() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let executable = temporary.path().join("host");
        let alias = temporary.path().join("host-alias");
        std::fs::write(&executable, b"host").expect("host fixture");
        std::fs::hard_link(&executable, &alias).expect("hard-linked host fixture");
        assert!(same_executable_file(&executable, &alias).expect("same file"));
    }
}
