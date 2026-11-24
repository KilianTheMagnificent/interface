//! The provider command catalogue: Codex's built-in commands, the skills
//! `skills/list` reports, and refreshing the advertised snapshot.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use interface_common::sync::lock;
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, NoticeTone, ProviderCommand},
    provider_commands::{
        PROVIDER_COMMAND_LIMITS, ProviderCommandCandidate, normalize_provider_commands,
        parse_provider_command_text,
    },
};
use interface_engine::EngineError;
use serde_json::{Value, json};

use crate::protocol::CodexMethod;

use super::{
    map_text,
    session::{SessionShared, emit_notice},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CodexSkillCommand {
    pub(super) command_name: String,
    pub(super) name: String,
    pub(super) path: String,
    description: String,
}

struct CodexSkillRead {
    skills: Vec<CodexSkillCommand>,
    complete: bool,
}

#[derive(Default)]
pub(super) struct SessionCommands {
    pub(super) commands: Vec<ProviderCommand>,
    pub(super) skills: HashMap<String, CodexSkillCommand>,
    /// Every name ever advertised remains a refusal fence after removal.
    pub(super) advertised_names: HashSet<String>,
    refresh_revision: u64,
    pub(super) compacting: bool,
    pub(super) compact_completion_fence: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CommandRefreshState {
    revision: u64,
    running: bool,
    succeeded: Option<bool>,
}

fn skill_candidate(skill: &CodexSkillCommand) -> ProviderCommandCandidate {
    candidate(
        &skill.command_name,
        &skill.description,
        Some("[instructions]"),
    )
}

fn command_candidates(skills: &[CodexSkillCommand]) -> Vec<ProviderCommandCandidate> {
    let mut candidates = codex_builtin_command_candidates();
    candidates.extend(skills.iter().map(skill_candidate));
    candidates
}

fn replace_command_snapshot(
    shared: &SessionShared,
    skills: Vec<CodexSkillCommand>,
    force: bool,
    expected_revision: Option<u64>,
) -> bool {
    let commands = normalize_provider_commands(&command_candidates(&skills));
    let visible_names = commands
        .iter()
        .map(|command| command.name.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut executable_skills = HashMap::new();
    for skill in skills {
        let key = skill.command_name.to_ascii_lowercase();
        if visible_names.contains(&key)
            && !is_codex_builtin_command(&key)
            && !executable_skills.contains_key(&key)
        {
            executable_skills.insert(key, skill);
        }
    }
    let publish = {
        let mut state = lock(&shared.commands);
        if expected_revision.is_some_and(|revision| state.refresh_revision != revision) {
            return false;
        }
        state.skills = executable_skills;
        for command in &commands {
            state
                .advertised_names
                .insert(command.name.to_ascii_lowercase());
            for alias in command.aliases.iter().flatten() {
                state.advertised_names.insert(alias.to_ascii_lowercase());
            }
        }
        let publish = force || state.commands != commands;
        if publish {
            state.commands.clone_from(&commands);
        }
        publish
    };
    if publish {
        let _ = shared.core.emit(EngineEvent {
            at: None,
            event: EngineEventKind::CommandsAvailable { commands },
        });
    }
    true
}

pub(super) fn publish_command_snapshot(
    shared: &SessionShared,
    skills: Vec<CodexSkillCommand>,
    force: bool,
) {
    let _ = replace_command_snapshot(shared, skills, force, None);
}

fn begin_command_refresh(shared: &SessionShared) -> u64 {
    let revision = {
        let mut commands = lock(&shared.commands);
        commands.refresh_revision = commands.refresh_revision.saturating_add(1);
        commands.refresh_revision
    };
    let _ = shared.command_refresh.send(CommandRefreshState {
        revision,
        running: true,
        succeeded: None,
    });
    revision
}

async fn read_live_skill_commands(shared: &SessionShared) -> Result<CodexSkillRead, EngineError> {
    let transport = shared.core.transport()?;
    let params = if shared.cwd.is_empty() {
        json!({})
    } else {
        json!({"cwds": [&shared.cwd]})
    };
    let response = transport
        .request(CodexMethod::SKILLS_LIST, &params)
        .await
        .into_result()?;
    Ok(read_skill_commands(&response, &shared.cwd))
}

async fn finish_command_refresh(
    shared: Arc<SessionShared>,
    revision: u64,
    report_failure: bool,
) -> bool {
    let result = read_live_skill_commands(&shared).await;
    if shared.core.is_closed() {
        return false;
    }
    let succeeded = if let Ok(found) = result {
        replace_command_snapshot(&shared, found.skills, false, Some(revision))
    } else {
        if lock(&shared.commands).refresh_revision != revision {
            return false;
        }
        if report_failure {
            emit_notice(
                &shared,
                "Codex could not refresh its available skills.",
                NoticeTone::Warning,
            );
        }
        false
    };
    if lock(&shared.commands).refresh_revision == revision {
        let _ = shared.command_refresh.send(CommandRefreshState {
            revision,
            running: false,
            succeeded: Some(succeeded),
        });
    }
    succeeded
}

pub(super) fn spawn_command_refresh(shared: Arc<SessionShared>, report_failure: bool) {
    let revision = begin_command_refresh(&shared);
    tokio::spawn(async move {
        let _ = finish_command_refresh(shared, revision, report_failure).await;
    });
}

pub(super) async fn refresh_commands(shared: &Arc<SessionShared>, report_failure: bool) -> bool {
    let revision = begin_command_refresh(shared);
    finish_command_refresh(Arc::clone(shared), revision, report_failure).await
}

async fn wait_for_command_refresh(shared: &SessionShared) -> bool {
    let mut receiver = shared.command_refresh.subscribe();
    loop {
        let state = *receiver.borrow_and_update();
        if !state.running {
            return state.succeeded.unwrap_or(false);
        }
        if receiver.changed().await.is_err() {
            return false;
        }
    }
}

pub(super) fn skill_command_key(text: &str) -> Option<String> {
    let parsed = parse_provider_command_text(text)?;
    let key = parsed.name.to_ascii_lowercase();
    key.starts_with("skill:").then_some(key)
}

fn has_current_or_stale_skill(shared: &SessionShared, key: &str) -> bool {
    let commands = lock(&shared.commands);
    commands.skills.contains_key(key) || commands.advertised_names.contains(key)
}

pub(super) async fn ensure_skill_catalogue(shared: &Arc<SessionShared>, key: &str) -> bool {
    if has_current_or_stale_skill(shared, key) {
        return true;
    }
    let refresh = *shared.command_refresh.borrow();
    let refreshed = if refresh.running {
        wait_for_command_refresh(shared).await
    } else if refresh.revision == 0 {
        refresh_commands(shared, false).await
    } else {
        refresh.succeeded.unwrap_or(false)
    };
    if !refreshed && lock(&shared.commands).skills.is_empty() {
        return false;
    }
    true
}

fn candidate(
    name: &str,
    description: &str,
    argument_hint: Option<&str>,
) -> ProviderCommandCandidate {
    ProviderCommandCandidate {
        name: Value::String(name.into()),
        description: Value::String(description.into()),
        argument_hint: argument_hint.map_or(Value::Null, |value| Value::String(value.into())),
        aliases: Value::Null,
    }
}

/// The builtin command roster: name, description, argument hint. One table so a
/// name and its prose can never drift apart.
const CODEX_BUILTIN_COMMANDS: [(&str, &str, Option<&str>); 8] = [
    ("status", "Display the current session configuration.", None),
    (
        "mcp",
        "List configured Model Context Protocol (MCP) tools.",
        None,
    ),
    ("skills", "List available skills.", None),
    (
        "review",
        "Review uncommitted changes, or review with custom instructions.",
        Some("optional review instructions"),
    ),
    (
        "review-branch",
        "Review changes relative to a base branch.",
        Some("branch name"),
    ),
    (
        "review-commit",
        "Review a specific commit.",
        Some("commit sha"),
    ),
    (
        "compact",
        "Summarize conversation to avoid hitting the context limit.",
        None,
    ),
    (
        "logout",
        "Sign out of Codex. This option is available when you are logged in via ChatGPT.",
        None,
    ),
];

pub(super) fn is_codex_builtin_command(key: &str) -> bool {
    CODEX_BUILTIN_COMMANDS.iter().any(|(name, ..)| *name == key)
}

pub(super) fn codex_builtin_command_candidates() -> Vec<ProviderCommandCandidate> {
    CODEX_BUILTIN_COMMANDS
        .iter()
        .map(|(name, description, hint)| candidate(name, description, *hint))
        .collect()
}

fn read_skill_commands(response: &Value, cwd: &str) -> CodexSkillRead {
    let mut found = Vec::new();
    let mut scanned = 0_usize;
    let mut matched = cwd.is_empty();
    let mut complete = true;
    for entry in response
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(PROVIDER_COMMAND_LIMITS.candidates)
        .filter_map(Value::as_object)
    {
        if !cwd.is_empty() && map_text(entry, "cwd") != Some(cwd) {
            continue;
        }
        matched = true;
        if entry
            .get("errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| !errors.is_empty())
        {
            complete = false;
        }
        for skill in entry
            .get("skills")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            if scanned >= PROVIDER_COMMAND_LIMITS.candidates {
                break;
            }
            scanned += 1;
            let Some(name) = map_text(skill, "name") else {
                continue;
            };
            let Some(path) = map_text(skill, "path") else {
                continue;
            };
            if skill.get("enabled").and_then(Value::as_bool) == Some(false)
                || name.len() > PROVIDER_COMMAND_LIMITS.name
                || path.len() > 4_096
            {
                continue;
            }
            let raw_description = map_text(skill, "shortDescription")
                .or_else(|| {
                    skill
                        .get("interface")
                        .and_then(Value::as_object)
                        .and_then(|value| map_text(value, "shortDescription"))
                })
                .or_else(|| map_text(skill, "description"))
                .unwrap_or(name);
            let description =
                if raw_description.len() > PROVIDER_COMMAND_LIMITS.description.saturating_mul(4) {
                    name.to_owned()
                } else {
                    raw_description.replace(path, name)
                };
            found.push(CodexSkillCommand {
                command_name: format!("skill:{name}"),
                name: name.to_owned(),
                path: path.to_owned(),
                description,
            });
        }
    }
    CodexSkillRead {
        skills: found,
        complete: complete && matched,
    }
}

pub(super) fn skill_command_candidates(
    response: &Value,
    cwd: &str,
) -> Option<Vec<ProviderCommandCandidate>> {
    let found = read_skill_commands(response, cwd);
    found
        .complete
        .then(|| found.skills.iter().map(skill_candidate).collect())
}
