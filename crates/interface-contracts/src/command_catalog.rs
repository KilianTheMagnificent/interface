//! Shared application command vocabulary. Catalog reads never dispatch effects.
use serde::{Deserialize, Serialize};

/// One application command and everything its catalog entry says about it.
/// Each entry names its own spec or starts from one of the templates below.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppCommand {
    pub name: &'static str,
    pub description: &'static str,
    pub aliases: &'static [&'static str],
    pub arguments: &'static [AppCommandArgument],
    /// Empty means the bare `/<name>`.
    pub example: &'static str,
    pub target: &'static str,
    pub effect: &'static str,
    pub capability: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppCommandArgument {
    pub name: &'static str,
    pub required: bool,
    pub description: &'static str,
}

const fn argument(
    name: &'static str,
    required: bool,
    description: &'static str,
) -> AppCommandArgument {
    AppCommandArgument {
        name,
        required,
        description,
    }
}

/// A read-only look at the selected session or workspace.
const INSPECT: AppCommand = AppCommand {
    name: "",
    description: "",
    aliases: &[],
    arguments: &[],
    example: "",
    target: "selected session or workspace",
    effect: "inspect",
    capability: None,
};

const LAUNCH_DRAFT: AppCommand = AppCommand {
    target: "new-session launch draft",
    effect: "draft",
    ..INSPECT
};

const SESSION_SETTING: AppCommand = AppCommand {
    arguments: &[argument("value", false, "Omit to browse supported values")],
    target: "selected session or launch draft",
    effect: "provider",
    capability: Some("configuration"),
    ..INSPECT
};

const PEER_MODEL: AppCommand = AppCommand {
    target: "selected session and explicit peer model",
    effect: "provider",
    capability: Some("collaboration"),
    ..INSPECT
};

const COLLABORATION_REQUEST: AppCommand = AppCommand {
    target: "explicit collaboration request",
    effect: "provider",
    capability: Some("collaboration"),
    ..INSPECT
};

const EXIT: AppCommand = AppCommand {
    example: "/quit",
    target: "this iface process and its owned provider attachments",
    effect: "stopOwnedProcesses",
    ..INSPECT
};

pub const APP_COMMANDS: &[AppCommand] = &[
    AppCommand {
        name: "sessions",
        description: "Pick a known session",
        ..INSPECT
    },
    AppCommand {
        name: "new",
        description: "Return to the new-session composer",
        ..LAUNCH_DRAFT
    },
    AppCommand {
        name: "cwd",
        description: "Choose a launch directory with completion",
        ..LAUNCH_DRAFT
    },
    AppCommand {
        name: "engine",
        description: "Choose a launch engine",
        ..LAUNCH_DRAFT
    },
    AppCommand {
        name: "resume",
        description: "Resume this session or an id prefix",
        arguments: &[argument("session", false, "Stored session ID prefix")],
        target: "selected stored session",
        effect: "provider",
        capability: Some("resume"),
        ..INSPECT
    },
    AppCommand {
        name: "model",
        description: "Choose the provider model",
        arguments: &[argument(
            "model",
            false,
            "Provider model ID; omit to browse",
        )],
        target: "selected session or launch draft",
        effect: "provider",
        capability: Some("models"),
        ..INSPECT
    },
    AppCommand {
        name: "effort",
        description: "Choose reasoning effort",
        arguments: &[argument(
            "effort",
            false,
            "Provider reasoning effort; omit to browse",
        )],
        target: "selected session or launch draft",
        effect: "provider",
        capability: Some("effort"),
        ..INSPECT
    },
    AppCommand {
        name: "fast",
        description: "Toggle fast mode when the selected model supports it",
        ..SESSION_SETTING
    },
    AppCommand {
        name: "access",
        description: "Choose the provider permission mode",
        ..SESSION_SETTING
    },
    AppCommand {
        name: "consult",
        description: "Ask another model, then return its answer here",
        ..PEER_MODEL
    },
    AppCommand {
        name: "handoff",
        description: "Transfer this project to another model",
        target: "selected session and explicit destination",
        effect: "preview",
        capability: Some("handoff"),
        ..INSPECT
    },
    AppCommand {
        name: "collab",
        description: "Review, approve, deny, or cancel model requests",
        ..COLLABORATION_REQUEST
    },
    AppCommand {
        name: "peer",
        description: "Accept or discard held peer advice",
        ..COLLABORATION_REQUEST
    },
    AppCommand {
        name: "steers",
        description: "Inspect steering attempts for this session",
        aliases: &["receipts"],
        target: "selected session delivery receipts",
        ..INSPECT
    },
    AppCommand {
        name: "commands",
        description: "Browse provider slash commands",
        ..INSPECT
    },
    AppCommand {
        name: "queue",
        description: "View, edit, or remove an unsent follow-up",
        aliases: &["send later", "follow-up"],
        target: "unsent follow-up for selected session",
        effect: "draft",
        ..INSPECT
    },
    AppCommand {
        name: "details",
        description: "Open the latest tool call details",
        ..INSPECT
    },
    AppCommand {
        name: "window",
        description: "Inspect provider-reported context usage",
        ..INSPECT
    },
    AppCommand {
        name: "stop",
        description: "Interrupt the active turn",
        aliases: &["interrupt"],
        target: "active turn in selected session",
        effect: "provider",
        capability: Some("interrupt"),
        ..INSPECT
    },
    AppCommand {
        name: "notifications",
        description: "Turn terminal notifications on or off",
        target: "this iface user configuration",
        effect: "configuration",
        ..INSPECT
    },
    AppCommand {
        name: "diagnostics",
        description: "Show sanitized diagnostics",
        ..INSPECT
    },
    AppCommand {
        name: "doctor",
        description: "Show configured inventory; provider startup health is separate",
        ..INSPECT
    },
    AppCommand {
        name: "advisor",
        description: "Review the last turn with another model, then hold the note",
        ..PEER_MODEL
    },
    AppCommand {
        name: "agents",
        description: "List and inspect provider-reported child agents",
        ..INSPECT
    },
    AppCommand {
        name: "orchestrate",
        description: "Inspect shared orchestration runs",
        ..INSPECT
    },
    AppCommand {
        name: "spawn",
        description: "Start a named orchestrated agent",
        arguments: &[
            argument("engine", true, "codex, claude, or acp:<profile>"),
            argument("name", true, "Unique agent name"),
            argument("--model", false, "Explicit provider model ID"),
            argument("--effort", false, "Explicit provider reasoning effort"),
            argument("prompt", true, "Bounded assignment"),
        ],
        example: "/spawn codex reviewer Inspect the changed files",
        target: "current project",
        effect: "startTask",
        capability: Some("orchestration"),
        ..INSPECT
    },
    AppCommand {
        name: "send",
        description: "Send a durable message to an orchestrated agent",
        arguments: &[
            argument("agent", true, "Exact agent name or ID"),
            argument("message", true, "Message to send"),
        ],
        example: "/send reviewer Check the error handling",
        target: "named agent",
        effect: "provider",
        capability: Some("orchestration"),
        ..INSPECT
    },
    AppCommand {
        name: "inbox",
        description: "Inspect an orchestrated agent's durable inbox",
        ..INSPECT
    },
    AppCommand {
        name: "monitor",
        description: "Inspect the shared orchestration event timeline",
        ..INSPECT
    },
    AppCommand {
        name: "help",
        description: "Show iface keybindings and commands",
        ..INSPECT
    },
    AppCommand {
        name: "quit",
        description: "Close providers and exit iface",
        ..EXIT
    },
    AppCommand {
        name: "exit",
        description: "Close providers and exit iface",
        ..EXIT
    },
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandDescriptor {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub arguments: Vec<CommandArgument>,
    pub example: String,
    pub target: String,
    pub effect: String,
    pub required_capability: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandArgument {
    pub name: String,
    pub required: bool,
    pub description: String,
}

#[must_use]
pub fn command_catalog() -> Vec<CommandDescriptor> {
    APP_COMMANDS
        .iter()
        .map(|command| CommandDescriptor {
            id: format!("iface:{}", command.name),
            name: command.name.into(),
            aliases: command
                .aliases
                .iter()
                .map(|alias| (*alias).into())
                .collect(),
            description: command.description.into(),
            arguments: command
                .arguments
                .iter()
                .map(|argument| CommandArgument {
                    name: argument.name.into(),
                    required: argument.required,
                    description: argument.description.into(),
                })
                .collect(),
            example: if command.example.is_empty() {
                format!("/{}", command.name)
            } else {
                command.example.into()
            },
            target: command.target.into(),
            effect: command.effect.into(),
            required_capability: command.capability.map(Into::into),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    /// The catalog reaches the harness detail and the iface agent server as
    /// JSON; these bytes are the contract.
    #[test]
    fn catalog_serializes_to_the_golden_json() {
        let mut actual =
            serde_json::to_string_pretty(&super::command_catalog()).expect("catalog serializes");
        actual.push('\n');
        assert_eq!(actual, include_str!("../fixtures/command-catalog.json"));
    }
}
