use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::ProviderCommand;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProviderCommandLimits {
    pub candidates: usize,
    pub commands: usize,
    pub alias_candidates: usize,
    pub aliases: usize,
    pub name: usize,
    pub description: usize,
    pub argument_hint: usize,
    pub total_bytes: usize,
}

pub const PROVIDER_COMMAND_LIMITS: ProviderCommandLimits = ProviderCommandLimits {
    candidates: 1_024,
    commands: 256,
    alias_candidates: 48,
    aliases: 12,
    name: 96,
    description: 480,
    argument_hint: 160,
    total_bytes: 128 * 1_024,
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCommandCandidate {
    #[serde(default)]
    pub name: Value,
    #[serde(default)]
    pub description: Value,
    #[serde(default)]
    pub argument_hint: Value,
    #[serde(default)]
    pub aliases: Value,
}

const APP_OWNED_COMMANDS: [&str; 29] = [
    "model",
    "models",
    "set-model",
    "effort",
    "set-effort",
    "reasoning-effort",
    "permission",
    "permissions",
    "permission-mode",
    "set-permission",
    "access",
    "access-mode",
    "set-access",
    "sandbox",
    "always-approve",
    "clear",
    "new",
    "reset",
    "resume",
    "rewind",
    "rollback",
    "fork",
    "consult",
    "handoff",
    "peer",
    "rename",
    "set-title",
    "fast-never-owned",
    "unused-sentinel",
];

fn app_owns(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase().replace('_', "-");
    APP_OWNED_COMMANDS.contains(&normalized.as_str())
}

fn command_name_str(value: &str) -> Option<String> {
    static COMMAND: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[a-z0-9][a-z0-9._:-]*$").expect("command regex"));
    if value.len() > PROVIDER_COMMAND_LIMITS.name + 8 {
        return None;
    }
    let name = value.trim_start_matches('/').trim();
    (!name.is_empty() && name.len() <= PROVIDER_COMMAND_LIMITS.name && COMMAND.is_match(name))
        .then(|| name.to_owned())
}

fn command_name(value: &Value) -> Option<String> {
    command_name_str(value.as_str()?)
}

fn bounded_text(value: &Value, max: usize) -> Option<String> {
    let source = value.as_str()?;
    let bounded = if source.chars().count() > max * 4 {
        source.chars().take(max * 4).collect::<String>()
    } else {
        source.to_owned()
    };
    let text = bounded.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        None
    } else if text.chars().count() <= max {
        Some(text)
    } else {
        let mut output = text.chars().take(max - 1).collect::<String>();
        while output.ends_with(char::is_whitespace) {
            output.pop();
        }
        output.push('…');
        Some(output)
    }
}

fn command_bytes(command: &ProviderCommand) -> usize {
    command.name.len()
        + command.description.len()
        + command.argument_hint.as_ref().map_or(0, String::len)
        + command
            .aliases
            .as_ref()
            .map_or(0, |aliases| aliases.iter().map(String::len).sum())
}

#[must_use]
pub fn normalize_provider_commands(
    candidates: &[ProviderCommandCandidate],
) -> Vec<ProviderCommand> {
    let bounded = candidates
        .iter()
        .take(PROVIDER_COMMAND_LIMITS.candidates)
        .collect::<Vec<_>>();
    let canonical_names = bounded
        .iter()
        .filter_map(|candidate| command_name(&candidate.name))
        .filter(|name| !app_owns(name))
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut claimed_names = HashSet::new();
    let mut claimed_aliases = HashSet::new();
    let mut total_bytes = 0;
    let mut output = Vec::new();
    for candidate in bounded {
        if output.len() >= PROVIDER_COMMAND_LIMITS.commands {
            break;
        }
        let Some(name) = command_name(&candidate.name) else {
            continue;
        };
        let name_key = name.to_ascii_lowercase();
        if app_owns(&name) || claimed_names.contains(&name_key) {
            continue;
        }
        let mut aliases = Vec::new();
        if let Some(raw_aliases) = candidate.aliases.as_array() {
            for raw in raw_aliases
                .iter()
                .take(PROVIDER_COMMAND_LIMITS.alias_candidates)
            {
                if aliases.len() >= PROVIDER_COMMAND_LIMITS.aliases {
                    break;
                }
                let Some(alias) = command_name(raw) else {
                    continue;
                };
                let key = alias.to_ascii_lowercase();
                if app_owns(&alias)
                    || key == name_key
                    || canonical_names.contains(&key)
                    || claimed_names.contains(&key)
                    || claimed_aliases.contains(&key)
                    || aliases
                        .iter()
                        .any(|value: &String| value.eq_ignore_ascii_case(&alias))
                {
                    continue;
                }
                aliases.push(alias);
            }
        }
        let command = ProviderCommand {
            name,
            description: bounded_text(&candidate.description, PROVIDER_COMMAND_LIMITS.description)
                .unwrap_or_else(|| "Provider command".to_owned()),
            argument_hint: bounded_text(
                &candidate.argument_hint,
                PROVIDER_COMMAND_LIMITS.argument_hint,
            ),
            aliases: (!aliases.is_empty()).then_some(aliases.clone()),
        };
        let next_bytes = command_bytes(&command);
        if total_bytes + next_bytes > PROVIDER_COMMAND_LIMITS.total_bytes {
            break;
        }
        total_bytes += next_bytes;
        claimed_names.insert(name_key);
        claimed_aliases.extend(aliases.into_iter().map(|alias| alias.to_ascii_lowercase()));
        output.push(command);
    }
    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedProviderCommand {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderCommandLike {
    pub name: String,
    pub arguments: String,
    pub multiline: bool,
}

#[must_use]
pub fn parse_provider_command_like(text: &str) -> Option<ProviderCommandLike> {
    let trimmed = text.trim();
    let after_slash = trimmed.strip_prefix('/')?;
    if after_slash.starts_with('/') || after_slash.is_empty() {
        return None;
    }
    let split = after_slash
        .find(char::is_whitespace)
        .unwrap_or(after_slash.len());
    let raw_name = &after_slash[..split];
    let name = command_name_str(raw_name)?;
    let arguments = after_slash[split..].trim().to_owned();
    Some(ProviderCommandLike {
        name,
        arguments,
        multiline: trimmed.contains(['\n', '\r']),
    })
}

#[must_use]
pub fn parse_provider_command_text(text: &str) -> Option<ParsedProviderCommand> {
    let parsed = parse_provider_command_like(text)?;
    if parsed.multiline || app_owns(&parsed.name) {
        return None;
    }
    Some(ParsedProviderCommand {
        name: parsed.name,
        arguments: parsed.arguments,
    })
}

#[must_use]
pub fn is_app_owned_provider_command(name: &str) -> bool {
    app_owns(name)
}

#[must_use]
pub fn app_owned_provider_command_error(text: &str) -> Option<String> {
    let parsed = parse_provider_command_like(text)?;
    app_owns(&parsed.name).then(|| {
        format!(
            "Interface owns /{}. Use its visible model, effort, access, title, or session controls instead; nothing was sent to the provider.",
            parsed.name
        )
    })
}

#[must_use]
pub fn provider_command_draft(command: &ProviderCommand) -> String {
    format!(
        "/{}{}",
        command.name,
        if command.argument_hint.is_some() {
            " "
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn candidate(name: Value, description: Value, aliases: Value) -> ProviderCommandCandidate {
        ProviderCommandCandidate {
            name,
            description,
            aliases,
            argument_hint: Value::Null,
        }
    }

    #[test]
    fn normalizes_and_preserves_canonical_names_over_borrowed_aliases() {
        let commands = normalize_provider_commands(&[
            candidate(json!("first"), json!(" One  command "), json!(["second"])),
            candidate(json!("second"), json!("Two"), json!([])),
            candidate(json!("model"), json!("owned"), json!([])),
        ]);
        assert_eq!(
            commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(commands[0].description, "One command");
        assert_eq!(commands[0].aliases, None);
    }

    #[test]
    fn only_complete_single_line_non_owned_invocations_dispatch() {
        assert_eq!(
            parse_provider_command_text(" /review src ").unwrap(),
            ParsedProviderCommand {
                name: "review".into(),
                arguments: "src".into()
            }
        );
        assert!(parse_provider_command_text("// comment").is_none());
        assert!(parse_provider_command_text("/model opus").is_none());
        assert!(parse_provider_command_text("/review\nnow").is_none());
    }

    #[test]
    fn draft_matches_the_palette_contract() {
        let command = ProviderCommand {
            name: "review".into(),
            description: "Inspect".into(),
            argument_hint: Some("<path>".into()),
            aliases: Some(vec!["inspect".into()]),
        };
        assert_eq!(provider_command_draft(&command), "/review ");
    }
}
