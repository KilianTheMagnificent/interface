use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

const MAX_ARGUMENT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    Auto,
    DontAsk,
    BypassPermissions,
}

impl PermissionMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("unknown Claude permission mode `{0}`")]
pub struct PermissionModeParseError(String);

impl FromStr for PermissionMode {
    type Err = PermissionModeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "acceptEdits" => Ok(Self::AcceptEdits),
            "plan" => Ok(Self::Plan),
            "auto" => Ok(Self::Auto),
            "dontAsk" => Ok(Self::DontAsk),
            "bypassPermissions" => Ok(Self::BypassPermissions),
            _ => Err(PermissionModeParseError(value.to_owned())),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    #[must_use]
    const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTarget {
    New { session_id: String },
    Resume { session_id: String, fork: bool },
}

impl SessionTarget {
    #[must_use]
    pub fn session_id(&self) -> &str {
        match self {
            Self::New { session_id } | Self::Resume { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpServer {
    pub command: PathBuf,
    pub args: Vec<String>,
}

impl McpServer {
    fn as_wire_value(&self) -> Value {
        json!({
            "type": "stdio",
            "command": self.command,
            "args": self.args,
            // Host-owned MCP descriptors deliberately cannot smuggle secrets.
            "env": {},
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct Initialization {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    pub exclude_dynamic_sections: bool,
    pub forward_subagent_text: bool,
    pub prompt_suggestions: bool,
    pub agent_progress_summaries: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_dialog_kinds: Vec<String>,
}

impl Default for Initialization {
    fn default() -> Self {
        Self {
            system_prompt: None,
            append_system_prompt: None,
            exclude_dynamic_sections: true,
            forward_subagent_text: true,
            prompt_suggestions: true,
            agent_progress_summaries: true,
            supported_dialog_kinds: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LaunchOptions {
    pub executable: PathBuf,
    pub cwd: PathBuf,
    pub target: SessionTarget,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Session-scoped settings passed through Claude's flag-settings layer.
    /// This is the launch-time equivalent of `apply_flag_settings` and must be
    /// present before initialization when the first turn starts in ultracode or
    /// fast mode.
    pub settings: Option<Map<String, Value>>,
    pub permission_mode: PermissionMode,
    /// `Some([])` is Claude's full settings isolation mode.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Forces `--strict-mcp-config` so repository `.mcp.json` descriptors are ignored.
    pub strict_mcp_config: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// `Some([])` disables every built-in tool (`--tools` with an empty value).
    pub tools: Option<Vec<String>>,
    pub mcp_servers: BTreeMap<String, McpServer>,
    /// Extra folders granted natively with `--add-dir`; each must be absolute.
    pub additional_directories: Vec<PathBuf>,
    pub include_partial_messages: bool,
    pub persist_session: bool,
    pub initialization: Initialization,
    /// Set on the child over what it inherits, after Interface's own
    /// entries: an added provider account's `CLAUDE_CONFIG_DIR`. Empty (the
    /// Default account) sets nothing.
    pub environment: Vec<(OsString, OsString)>,
}

impl LaunchOptions {
    #[must_use]
    pub fn new(
        executable: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        target: SessionTarget,
    ) -> Self {
        Self {
            executable: executable.into(),
            cwd: cwd.into(),
            target,
            model: None,
            effort: None,
            settings: None,
            permission_mode: PermissionMode::Default,
            setting_sources: None,
            strict_mcp_config: false,
            allowed_tools: Vec::new(),
            disallowed_tools: Vec::new(),
            tools: None,
            mcp_servers: BTreeMap::new(),
            additional_directories: Vec::new(),
            include_partial_messages: true,
            persist_session: true,
            initialization: Initialization::default(),
            environment: Vec::new(),
        }
    }

    /// A naming alias for `build_spec` that discards the argv: it exists so a
    /// caller admitting a session can fail fast without pretending to keep a
    /// spec it never uses. It is **not** a second validator — the argv-safety
    /// checks stay where the arguments are assembled so they cannot drift from
    /// the code that actually spawns.
    ///
    /// # Errors
    ///
    /// Same as [`ClaudeLaunchOptions::build_spec`].
    pub fn validate(&self) -> Result<(), LaunchSpecError> {
        self.build_spec().map(|_| ())
    }

    /// Builds the exact argument vector used for a direct CLI spawn.
    ///
    /// # Errors
    ///
    /// Returns an error when an identifier, path, provider option, or MCP
    /// descriptor is unsafe or cannot be encoded as JSON.
    pub fn build_spec(&self) -> Result<LaunchSpec, LaunchSpecError> {
        validate_path("executable", &self.executable)?;
        validate_path("cwd", &self.cwd)?;
        validate_text("session id", self.target.session_id())?;

        let mut arguments = vec![
            OsString::from("--output-format"),
            OsString::from("stream-json"),
            OsString::from("--verbose"),
            OsString::from("--input-format"),
            OsString::from("stream-json"),
        ];

        push_option(&mut arguments, "--effort", self.effort.as_deref())?;
        push_option(&mut arguments, "--model", self.model.as_deref())?;
        if let Some(settings) = &self.settings {
            arguments.extend([
                OsString::from("--settings"),
                OsString::from(serde_json::to_string(settings)?),
            ]);
        }
        push_directories(&mut arguments, &self.additional_directories)?;
        // The direct host owns approval rendering and must receive permission
        // requests on the control channel instead of the terminal.
        arguments.extend([
            OsString::from("--permission-prompt-tool"),
            OsString::from("stdio"),
        ]);

        match &self.target {
            SessionTarget::New { session_id } => {
                arguments.push(OsString::from(format!("--session-id={session_id}")));
            }
            SessionTarget::Resume { session_id, fork } => {
                arguments.push(OsString::from(format!("--resume={session_id}")));
                if *fork {
                    arguments.push(OsString::from("--fork-session"));
                }
            }
        }

        push_csv(&mut arguments, "--allowedTools", &self.allowed_tools)?;
        push_csv(&mut arguments, "--disallowedTools", &self.disallowed_tools)?;
        if let Some(tools) = &self.tools {
            if tools.is_empty() {
                // `push_csv` skips empty vectors; Claude's tool-less chat is
                // `--tools` followed by an empty argument.
                arguments.extend([OsString::from("--tools"), OsString::from("")]);
            } else {
                push_csv(&mut arguments, "--tools", tools)?;
            }
        }

        if !self.mcp_servers.is_empty() {
            let servers = self
                .mcp_servers
                .iter()
                .map(|(name, server)| {
                    validate_mcp_name(name)?;
                    validate_path("MCP command", &server.command)?;
                    for argument in &server.args {
                        validate_mcp_argument(argument)?;
                    }
                    Ok((name.clone(), server.as_wire_value()))
                })
                .collect::<Result<Map<String, Value>, LaunchSpecError>>()?;
            arguments.extend([
                OsString::from("--mcp-config"),
                OsString::from(serde_json::to_string(&json!({ "mcpServers": servers }))?),
            ]);
        }

        let mut strict_mcp_config = self.strict_mcp_config;
        if let Some(sources) = &self.setting_sources {
            let value = sources
                .iter()
                .map(|source| source.as_str())
                .collect::<Vec<_>>()
                .join(",");
            arguments.push(OsString::from(format!("--setting-sources={value}")));
            if !sources.contains(&SettingSource::Project)
                && !sources.contains(&SettingSource::Local)
            {
                strict_mcp_config = true;
            }
        }
        if strict_mcp_config {
            arguments.push(OsString::from("--strict-mcp-config"));
        }

        arguments.extend([
            OsString::from("--permission-mode"),
            OsString::from(self.permission_mode.as_str()),
        ]);
        if self.permission_mode == PermissionMode::BypassPermissions {
            arguments.push(OsString::from("--allow-dangerously-skip-permissions"));
        }
        if self.include_partial_messages {
            arguments.push(OsString::from("--include-partial-messages"));
        }
        if !self.persist_session {
            arguments.push(OsString::from("--no-session-persistence"));
        }

        Ok(LaunchSpec {
            program: self.executable.clone(),
            arguments,
            cwd: self.cwd.clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchSpec {
    program: PathBuf,
    arguments: Vec<OsString>,
    cwd: PathBuf,
}

impl LaunchSpec {
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
}

#[derive(Debug, Error)]
pub enum LaunchSpecError {
    #[error(
        "{field} cannot be empty, start with '-', contain control bytes, or exceed {MAX_ARGUMENT_BYTES} bytes"
    )]
    InvalidText { field: &'static str },
    #[error("{field} must be a non-empty path without control bytes")]
    InvalidPath { field: &'static str },
    #[error("{field} must be an absolute path")]
    RelativePath { field: &'static str },
    #[error("MCP server name `{0}` is not a safe identifier")]
    InvalidMcpName(String),
    #[error("could not serialize the MCP launch descriptor: {0}")]
    Json(#[from] serde_json::Error),
}

fn validate_text(field: &'static str, value: &str) -> Result<(), LaunchSpecError> {
    if value.is_empty()
        || value.len() > MAX_ARGUMENT_BYTES
        || value.starts_with('-')
        || value.chars().any(char::is_control)
    {
        Err(LaunchSpecError::InvalidText { field })
    } else {
        Ok(())
    }
}

fn validate_mcp_argument(value: &str) -> Result<(), LaunchSpecError> {
    if value.is_empty() || value.len() > MAX_ARGUMENT_BYTES || value.chars().any(char::is_control) {
        Err(LaunchSpecError::InvalidText {
            field: "MCP argument",
        })
    } else {
        Ok(())
    }
}

fn validate_path(field: &'static str, value: &Path) -> Result<(), LaunchSpecError> {
    let raw = value.as_os_str().to_string_lossy();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        Err(LaunchSpecError::InvalidPath { field })
    } else {
        Ok(())
    }
}

fn validate_mcp_name(value: &str) -> Result<(), LaunchSpecError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' => true,
            b'0'..=b'9' | b'_' | b'-' => index > 0,
            _ => false,
        });
    if valid {
        Ok(())
    } else {
        Err(LaunchSpecError::InvalidMcpName(value.to_owned()))
    }
}

fn push_option(
    arguments: &mut Vec<OsString>,
    flag: &'static str,
    value: Option<&str>,
) -> Result<(), LaunchSpecError> {
    if let Some(value) = value {
        validate_text(flag, value)?;
        arguments.extend([OsString::from(flag), OsString::from(value)]);
    }
    Ok(())
}

fn push_directories(
    arguments: &mut Vec<OsString>,
    directories: &[PathBuf],
) -> Result<(), LaunchSpecError> {
    for directory in directories {
        validate_path("additional directory", directory)?;
        if !directory.is_absolute() {
            return Err(LaunchSpecError::RelativePath {
                field: "additional directory",
            });
        }
        arguments.extend([
            OsString::from("--add-dir"),
            directory.as_os_str().to_owned(),
        ]);
    }
    Ok(())
}

fn push_csv(
    arguments: &mut Vec<OsString>,
    flag: &'static str,
    values: &[String],
) -> Result<(), LaunchSpecError> {
    if values.is_empty() {
        return Ok(());
    }
    for value in values {
        validate_text(flag, value)?;
        if value.contains(',') {
            return Err(LaunchSpecError::InvalidText { field: flag });
        }
    }
    arguments.extend([OsString::from(flag), OsString::from(values.join(","))]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(spec: &LaunchSpec) -> Vec<&str> {
        spec.arguments()
            .iter()
            .map(|argument| argument.to_str())
            .collect::<Option<Vec<_>>>()
            .expect("test arguments are UTF-8")
    }

    #[test]
    fn new_isolated_session_has_exact_safety_flags() {
        let mut options = LaunchOptions::new(
            "/usr/local/bin/claude",
            "/workspace",
            SessionTarget::New {
                session_id: "8b764f80-19ee-47c7-a9da-08088a6db888".into(),
            },
        );
        options.setting_sources = Some(Vec::new());
        options.permission_mode = PermissionMode::BypassPermissions;

        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        assert!(arguments.contains(&"--setting-sources="));
        assert!(arguments.contains(&"--strict-mcp-config"));
        assert!(arguments.contains(&"--allow-dangerously-skip-permissions"));
        assert!(arguments.contains(&"--permission-prompt-tool"));
        assert!(arguments.contains(&"--include-partial-messages"));
    }

    #[test]
    fn resume_never_sends_a_conflicting_new_session_id() {
        let options = LaunchOptions::new(
            "claude",
            "/workspace",
            SessionTarget::Resume {
                session_id: "existing".into(),
                fork: true,
            },
        );
        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        assert!(arguments.contains(&"--resume=existing"));
        assert!(arguments.contains(&"--fork-session"));
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.starts_with("--session-id="))
        );
    }

    #[test]
    fn bypass_safety_flag_is_not_set_for_other_modes() {
        let options = LaunchOptions::new(
            "claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        let spec = options.build_spec().expect("valid launch");
        assert!(!rendered(&spec).contains(&"--allow-dangerously-skip-permissions"));
    }

    #[test]
    fn empty_tools_emits_the_flag_and_an_empty_argument() {
        let mut options = LaunchOptions::new(
            "claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        options.tools = Some(Vec::new());
        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        assert_eq!(
            arguments
                .windows(2)
                .find(|pair| pair[0] == "--tools")
                .map(|pair| pair[1]),
            Some("")
        );
    }

    #[test]
    fn unset_tools_does_not_emit_the_flag() {
        let options = LaunchOptions::new(
            "claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        assert!(!rendered(&options.build_spec().expect("valid launch")).contains(&"--tools"));
    }

    #[test]
    fn nonempty_tools_emits_csv_like_the_other_tool_flags() {
        let mut options = LaunchOptions::new(
            "claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        options.tools = Some(vec!["Read".into(), "Edit".into()]);
        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        assert_eq!(
            arguments
                .windows(2)
                .find(|pair| pair[0] == "--tools")
                .map(|pair| pair[1]),
            Some("Read,Edit")
        );
    }

    #[test]
    fn additional_directories_emit_add_dir_pairs_in_order_before_the_prompt_tool() {
        let mut options = LaunchOptions::new(
            "/usr/local/bin/claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        options.additional_directories = vec![
            PathBuf::from("/agents/scout"),
            PathBuf::from("/projects/app"),
        ];
        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        let prompt_tool = arguments
            .iter()
            .position(|argument| *argument == "--permission-prompt-tool")
            .expect("prompt tool flag");
        let granted = arguments
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| pair[0] == "--add-dir")
            .map(|(index, pair)| (index, pair[1]))
            .collect::<Vec<_>>();
        assert_eq!(
            granted
                .iter()
                .map(|(_, directory)| *directory)
                .collect::<Vec<_>>(),
            ["/agents/scout", "/projects/app"]
        );
        assert!(granted.iter().all(|(index, _)| *index < prompt_tool));

        options.additional_directories = vec![PathBuf::from("relative/dir")];
        assert!(options.build_spec().is_err());
        options.additional_directories = vec![PathBuf::from("/bad\ndir")];
        assert!(options.build_spec().is_err());
    }

    #[test]
    fn launch_settings_are_serialized_as_one_flag_layer_before_the_process_starts() {
        let mut options = LaunchOptions::new(
            "/usr/local/bin/claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        options.effort = Some("xhigh".into());
        options.settings = Some(Map::from_iter([
            ("ultracode".into(), Value::Bool(true)),
            ("fastMode".into(), Value::Bool(true)),
        ]));

        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        assert_eq!(
            arguments
                .windows(2)
                .find(|pair| pair[0] == "--settings")
                .map(|pair| pair[1]),
            Some(r#"{"fastMode":true,"ultracode":true}"#)
        );
    }

    #[test]
    fn user_only_setting_sources_and_strict_flag_emit_strict_mcp_config_once() {
        let mut options = LaunchOptions::new(
            "/usr/local/bin/claude",
            "/workspace",
            SessionTarget::New {
                session_id: "new-session".into(),
            },
        );
        options.setting_sources = Some(vec![SettingSource::User]);
        options.strict_mcp_config = true;

        let spec = options.build_spec().expect("valid launch");
        let arguments = rendered(&spec);
        assert!(arguments.contains(&"--setting-sources=user"));
        assert_eq!(
            arguments
                .iter()
                .filter(|&&argument| argument == "--strict-mcp-config")
                .count(),
            1
        );
    }

    #[test]
    fn validate_text_rejects_leading_hyphen_flag_injection() {
        let base = || {
            LaunchOptions::new(
                "/usr/local/bin/claude",
                "/workspace",
                SessionTarget::New {
                    session_id: "new-session".into(),
                },
            )
        };

        let mut model = base();
        model.model = Some("--dangerously-skip-permissions".into());
        assert!(model.build_spec().is_err());

        let mut effort = base();
        effort.effort = Some("-o".into());
        assert!(effort.build_spec().is_err());

        let mut allowed = base();
        allowed.allowed_tools = vec!["-Bash".into()];
        assert!(allowed.build_spec().is_err());

        let mut session = base();
        session.target = SessionTarget::New {
            session_id: "--evil".into(),
        };
        assert!(session.build_spec().is_err());

        let mut mcp = base();
        mcp.mcp_servers.insert(
            "iface".into(),
            McpServer {
                command: PathBuf::from("/usr/bin/nc"),
                args: vec!["-U".into(), "/tmp/test.sock".into()],
            },
        );
        assert!(mcp.build_spec().is_ok());
    }
}
