//! Pure slash-command grammar shared by the human controller and agent paths.
use super::APP_COMMANDS;

pub(super) fn parsed_app_command(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim();
    let command = trimmed.strip_prefix('/')?;
    let (raw_name, argument) = command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(head, tail)| (head, tail.trim()));
    if raw_name.is_empty()
        || !raw_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some((
        raw_name.to_ascii_lowercase().replace('_', "-"),
        argument.to_owned(),
    ))
}

pub(super) fn split_command_word(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_start();
    if value.is_empty() {
        return None;
    }
    Some(
        value
            .split_once(char::is_whitespace)
            .map_or((value, ""), |(head, tail)| (head, tail.trim_start())),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct SpawnArgument<'a> {
    pub engine: &'a str,
    pub name: &'a str,
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
}
pub(super) fn parse_spawn_argument(argument: &str) -> Option<SpawnArgument<'_>> {
    let (engine, rest) = split_command_word(argument.trim())?;
    let (name, mut prompt) = split_command_word(rest)?;
    let mut model = None;
    let mut effort = None;
    while prompt.starts_with("--") {
        let (flag, rest) = split_command_word(prompt)?;
        if flag == "--" {
            prompt = rest;
            break;
        }
        let (value, rest) = split_command_word(rest)?;
        if value.starts_with("--") {
            return None;
        }
        match flag {
            "--model" if model.is_none() => model = Some(value),
            "--effort" if effort.is_none() => effort = Some(value),
            _ => return None,
        }
        prompt = rest;
    }
    (!prompt.trim().is_empty()).then_some(SpawnArgument {
        engine,
        name,
        prompt,
        model,
        effort,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ParsedSendArgument {
    pub(super) recipient_agent_id: String,
    pub(super) body: String,
    pub(super) expect_reply: bool,
    pub(super) sender_agent_id: Option<String>,
}

pub(super) fn parse_send_argument(argument: &str) -> Option<ParsedSendArgument> {
    let mut parts = argument
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let from_index = parts.iter().position(|part| part == "--from");
    let sender_agent_id = from_index.and_then(|index| parts.get(index + 1).cloned());
    if let Some(index) = from_index {
        if sender_agent_id
            .as_deref()
            .is_none_or(|sender| sender.starts_with("--"))
        {
            return None;
        }
        parts.drain(index..=index + 1);
    }
    if parts.is_empty() {
        return None;
    }
    let recipient_agent_id = parts.remove(0);
    let expect_reply = parts
        .iter()
        .position(|part| part == "--expect-reply")
        .map(|index| {
            parts.remove(index);
        })
        .is_some();
    let body = parts.join(" ");
    (!body.is_empty()).then_some(ParsedSendArgument {
        recipient_agent_id,
        body,
        expect_reply,
        sender_agent_id,
    })
}

pub(super) fn is_handled_app_command(name: &str) -> bool {
    APP_COMMANDS.iter().any(|command| command.name == name)
        || matches!(
            name,
            "models"
                | "set-model"
                | "set-effort"
                | "reasoning-effort"
                | "fast-mode"
                | "access-mode"
                | "permission"
                | "permissions"
                | "permission-mode"
                | "set-access"
                | "set-permission"
        )
}

pub(super) fn app_command_takes_argument(name: &str) -> bool {
    matches!(name, "model" | "effort" | "fast" | "access")
}
