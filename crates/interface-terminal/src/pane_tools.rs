//! Interface's tools for Grok and Antigravity: the one `iface` MCP entry each
//! CLI keeps in its own user config. Claude and Codex learn the pane socket
//! per launch; these two read it from a static entry, so the entry is the
//! owner's to turn on (Settings → Models & providers, protocol 48). Interface
//! writes it only then, and removes only an entry that is exactly its own.
//!
//! Pure: each CLI's own `mcp` argv and the reading of its `list` output. The
//! host runs them, bounded, with no stdin.

use interface_contracts::terminal::{TerminalLaunchProfileId, TerminalPaneToolsState};
use serde_json::Value;

/// The MCP server name: the `--mcp-config` key and the `mcp__iface__<tool>` prefix.
pub const PANE_TOOLS_SERVER_NAME: &str = "iface";
/// Every pane launch exports the host's pane socket under this name; the
/// static entry expands it.
pub const PANE_TOOLS_SOCKET_ENV: &str = "INTERFACE_PANE_MCP_SOCKET";
/// The entry, identical on every host: a shell that execs `nc` against
/// whatever socket the pane's environment names.
pub const PANE_TOOLS_COMMAND: &str = "/bin/sh";
pub const PANE_TOOLS_ARGS: [&str; 2] = ["-c", "exec /usr/bin/nc -U \"$INTERFACE_PANE_MCP_SOCKET\""];

/// A CLI that reads Interface's tools from a static user-config entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneToolsCli {
    Grok,
    Antigravity,
}

/// What a CLI's config holds under the `iface` name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneToolsEntry {
    /// No `iface` entry.
    Absent,
    /// Exactly Interface's entry; `enabled` is the CLI's own switch for it.
    Ours { enabled: bool },
    /// An `iface` entry that is not Interface's. Interface never touches it.
    Foreign,
    /// The listing could not be read.
    Unreadable,
}

impl PaneToolsEntry {
    /// What Settings shows.
    #[must_use]
    pub const fn state(self) -> TerminalPaneToolsState {
        match self {
            Self::Ours { enabled: true } => TerminalPaneToolsState::On,
            Self::Absent | Self::Ours { enabled: false } => TerminalPaneToolsState::Off,
            Self::Foreign => TerminalPaneToolsState::Taken,
            Self::Unreadable => TerminalPaneToolsState::Unknown,
        }
    }
}

impl PaneToolsCli {
    /// The CLI behind a launch profile, if it takes the static entry.
    #[must_use]
    pub const fn for_profile(id: TerminalLaunchProfileId) -> Option<Self> {
        match id {
            TerminalLaunchProfileId::Grok => Some(Self::Grok),
            TerminalLaunchProfileId::Gemini => Some(Self::Antigravity),
            _ => None,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Antigravity => "Antigravity",
        }
    }

    /// Lists the user config's MCP servers.
    #[must_use]
    pub fn list_arguments(self) -> Vec<&'static str> {
        match self {
            Self::Grok => vec!["mcp", "list", "--json"],
            Self::Antigravity => vec!["mcp", "list"],
        }
    }

    /// Adds (or re-enables) Interface's entry in the user config.
    #[must_use]
    pub fn add_arguments(self) -> Vec<&'static str> {
        let [dash_c, command] = PANE_TOOLS_ARGS;
        match self {
            Self::Grok => vec![
                "mcp",
                "add",
                "--scope",
                "user",
                PANE_TOOLS_SERVER_NAME,
                "--",
                PANE_TOOLS_COMMAND,
                dash_c,
                command,
            ],
            Self::Antigravity => vec![
                "mcp",
                "add",
                PANE_TOOLS_SERVER_NAME,
                "--",
                PANE_TOOLS_COMMAND,
                dash_c,
                command,
            ],
        }
    }

    /// Removes the `iface` entry from the user config. Run only after
    /// [`Self::read_entry`] answered [`PaneToolsEntry::Ours`].
    #[must_use]
    pub fn remove_arguments(self) -> Vec<&'static str> {
        match self {
            Self::Grok => vec!["mcp", "remove", "--scope", "user", PANE_TOOLS_SERVER_NAME],
            Self::Antigravity => vec!["mcp", "remove", PANE_TOOLS_SERVER_NAME],
        }
    }

    /// Reads what the `list` output says about the `iface` entry.
    #[must_use]
    pub fn read_entry(self, stdout: &str) -> PaneToolsEntry {
        match self {
            Self::Grok => read_grok(stdout),
            Self::Antigravity => read_antigravity(stdout),
        }
    }
}

/// `grok mcp list --json`: an array of `{name, scope, command, args, enabled}`.
/// Only the user scope counts; the host lists from `/`, where no project
/// config applies.
fn read_grok(stdout: &str) -> PaneToolsEntry {
    let Ok(Value::Array(servers)) = serde_json::from_str::<Value>(stdout) else {
        return PaneToolsEntry::Unreadable;
    };
    let mut found = PaneToolsEntry::Absent;
    for server in &servers {
        if server.get("name").and_then(Value::as_str) != Some(PANE_TOOLS_SERVER_NAME) {
            continue;
        }
        if server.get("scope").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let ours = server.get("command").and_then(Value::as_str) == Some(PANE_TOOLS_COMMAND)
            && server
                .get("args")
                .and_then(Value::as_array)
                .is_some_and(|args| {
                    args.iter()
                        .map(Value::as_str)
                        .eq(PANE_TOOLS_ARGS.iter().copied().map(Some))
                });
        found = if ours {
            match server.get("enabled") {
                None => PaneToolsEntry::Ours { enabled: true },
                Some(Value::Bool(enabled)) => PaneToolsEntry::Ours { enabled: *enabled },
                Some(_) => return PaneToolsEntry::Unreadable,
            }
        } else {
            PaneToolsEntry::Foreign
        };
    }
    found
}

/// `agy mcp list`: `No MCP servers configured.`, or a `NAME TYPE STATUS
/// COMMAND/URL` table with one server per line.
fn read_antigravity(stdout: &str) -> PaneToolsEntry {
    let mut lines = stdout
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty());
    let Some(header) = lines.next() else {
        return PaneToolsEntry::Unreadable;
    };
    if header.trim() == "No MCP servers configured." {
        return PaneToolsEntry::Absent;
    }
    let columns: Vec<&str> = header.split_whitespace().collect();
    if columns != ["NAME", "TYPE", "STATUS", "COMMAND/URL"] {
        return PaneToolsEntry::Unreadable;
    }
    let ours_command = format!("{PANE_TOOLS_COMMAND} {}", PANE_TOOLS_ARGS.join(" "));
    let mut found = PaneToolsEntry::Absent;
    for line in lines {
        let mut words = line.split_whitespace();
        if words.next() != Some(PANE_TOOLS_SERVER_NAME) {
            continue;
        }
        let (kind, status) = (words.next(), words.next());
        let command = words.collect::<Vec<_>>().join(" ");
        found = if kind == Some("stdio") && command == ours_command {
            match status {
                Some("enabled") => PaneToolsEntry::Ours { enabled: true },
                Some("disabled") => PaneToolsEntry::Ours { enabled: false },
                _ => return PaneToolsEntry::Unreadable,
            }
        } else {
            PaneToolsEntry::Foreign
        };
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS_AGY: &str =
        "iface  stdio  enabled  /bin/sh -c exec /usr/bin/nc -U \"$INTERFACE_PANE_MCP_SOCKET\"";

    #[test]
    fn each_cli_lists_adds_and_removes_only_the_user_entry() {
        assert_eq!(
            PaneToolsCli::Grok.add_arguments().join(" "),
            "mcp add --scope user iface -- /bin/sh -c exec /usr/bin/nc -U \"$INTERFACE_PANE_MCP_SOCKET\""
        );
        assert_eq!(
            PaneToolsCli::Antigravity.add_arguments().join(" "),
            "mcp add iface -- /bin/sh -c exec /usr/bin/nc -U \"$INTERFACE_PANE_MCP_SOCKET\""
        );
        assert_eq!(
            PaneToolsCli::Grok.remove_arguments(),
            ["mcp", "remove", "--scope", "user", "iface"]
        );
        assert_eq!(
            PaneToolsCli::Antigravity.remove_arguments(),
            ["mcp", "remove", "iface"]
        );
        assert_eq!(
            PaneToolsCli::Grok.list_arguments(),
            ["mcp", "list", "--json"]
        );
        assert_eq!(
            PaneToolsCli::for_profile(TerminalLaunchProfileId::Gemini),
            Some(PaneToolsCli::Antigravity)
        );
        assert_eq!(
            PaneToolsCli::for_profile(TerminalLaunchProfileId::Claude),
            None
        );
        assert!(PANE_TOOLS_ARGS[1].contains(PANE_TOOLS_SOCKET_ENV));
    }

    #[test]
    fn grok_listing_tells_interfaces_entry_from_the_owners_own() {
        let grok = PaneToolsCli::Grok;
        let ours = |enabled: bool| {
            serde_json::json!([{
                "command": "/bin/sh", "args": PANE_TOOLS_ARGS, "enabled": enabled,
                "name": "iface", "scope": "user"
            }])
            .to_string()
        };
        assert_eq!(grok.read_entry("[]"), PaneToolsEntry::Absent);
        assert_eq!(
            grok.read_entry(&ours(true)),
            PaneToolsEntry::Ours { enabled: true }
        );
        assert_eq!(
            grok.read_entry(&ours(false)),
            PaneToolsEntry::Ours { enabled: false }
        );
        let foreign = r#"[{"command":"/usr/bin/true","args":[],"enabled":true,"name":"iface","scope":"user"}]"#;
        assert_eq!(grok.read_entry(foreign), PaneToolsEntry::Foreign);
        let project_only = r#"[{"command":"/usr/bin/true","args":[],"enabled":true,"name":"iface","scope":"project"}]"#;
        assert_eq!(grok.read_entry(project_only), PaneToolsEntry::Absent);
        let other = r#"[{"command":"/bin/sh","args":["-c","x"],"enabled":true,"name":"github","scope":"user"}]"#;
        assert_eq!(grok.read_entry(other), PaneToolsEntry::Absent);
        assert_eq!(
            grok.read_entry("error: no leader"),
            PaneToolsEntry::Unreadable
        );
        assert_eq!(grok.read_entry(""), PaneToolsEntry::Unreadable);
        assert_eq!(
            PaneToolsEntry::Foreign.state(),
            TerminalPaneToolsState::Taken
        );
        assert_eq!(
            PaneToolsEntry::Ours { enabled: false }.state(),
            TerminalPaneToolsState::Off
        );
    }

    #[test]
    fn antigravity_listing_tells_interfaces_entry_from_the_owners_own() {
        let agy = PaneToolsCli::Antigravity;
        let table = |rows: &str| format!("NAME   TYPE   STATUS    COMMAND/URL\n{rows}\n");
        assert_eq!(
            agy.read_entry("No MCP servers configured.\n"),
            PaneToolsEntry::Absent
        );
        assert_eq!(
            agy.read_entry(&table(OURS_AGY)),
            PaneToolsEntry::Ours { enabled: true }
        );
        assert_eq!(
            agy.read_entry(&table(&OURS_AGY.replace("enabled", "disabled"))),
            PaneToolsEntry::Ours { enabled: false }
        );
        assert_eq!(
            agy.read_entry(&table("iface  stdio  enabled   /usr/bin/true x")),
            PaneToolsEntry::Foreign
        );
        assert_eq!(
            agy.read_entry(&table("other  stdio  enabled   /usr/bin/true x")),
            PaneToolsEntry::Absent
        );
        assert_eq!(
            agy.read_entry("Usage: agy mcp list\n"),
            PaneToolsEntry::Unreadable
        );
        assert_eq!(agy.read_entry(""), PaneToolsEntry::Unreadable);
    }
}
