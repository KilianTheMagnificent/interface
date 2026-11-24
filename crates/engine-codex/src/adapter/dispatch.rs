//! Running a provider command typed into a session: the immediate commands,
//! skills, `/review` and `/compact`, and their completion notifications.

use std::sync::Arc;

use interface_common::sync::lock;
use interface_contracts::{
    engine::{
        ContextCompactTrigger, ContextUsage, EngineEvent, EngineEventKind, NoticeTone,
        SessionStatus,
    },
    provider_commands::parse_provider_command_text,
};
use interface_engine::{DeliveryOutcome, EngineError, EngineErrorKind};
use serde_json::{Map, Value, json};

use crate::protocol::{CodexMethod, TextInput};

use super::{
    commands::{
        CodexSkillCommand, is_codex_builtin_command, refresh_commands, spawn_command_refresh,
    },
    map_text, safe_line,
    session::{SessionShared, emit_notice, emit_status},
    turns::{report_control_failure, start_new_turn, thread_request_target},
};

const COMMAND_LIST_PREVIEW: usize = 50;

pub(super) fn command_notice(
    shared: &SessionShared,
    working_already: bool,
    message: impl Into<String>,
    tone: NoticeTone,
) {
    if !working_already {
        emit_status(shared, SessionStatus::Working);
    }
    emit_notice(shared, message, tone);
    emit_status(shared, SessionStatus::Idle);
}

async fn status_message(shared: &SessionShared) -> String {
    let mut account_text = "unknown".to_owned();
    if let Ok(transport) = shared.core.transport()
        && let Ok(response) = transport
            .request(CodexMethod::ACCOUNT_READ, &json!({}))
            .await
            .into_result()
    {
        let account = response.get("account").and_then(Value::as_object);
        account_text = match account.and_then(|account| map_text(account, "type")) {
            None => "not logged in".into(),
            Some("chatgpt") => {
                let plan = account
                    .and_then(|account| map_text(account, "planType"))
                    .map(|plan| format!(" {}", safe_line(plan, 240)))
                    .unwrap_or_default();
                format!("ChatGPT{plan}")
            }
            Some("apiKey") => "API key configured".into(),
            Some("amazonBedrock") => "Amazon Bedrock".into(),
            Some(kind) => safe_line(kind, 240),
        };
    }
    let (model, effort, access) = {
        let config = lock(&shared.config);
        (
            config
                .model
                .as_deref()
                .or(config.resolved_model.as_deref())
                .unwrap_or("provider default")
                .to_owned(),
            config
                .effort
                .as_deref()
                .unwrap_or("provider default")
                .to_owned(),
            config
                .approval_display
                .as_deref()
                .unwrap_or("thread policy")
                .to_owned(),
        )
    };
    let session_id = lock(&shared.session_id).clone();
    format!(
        "Codex session status:\n- Model: {}\n- Effort: {}\n- Access: {}\n- Directory: {}\n- Account: {}\n- Session: {}",
        safe_line(&model, 240),
        safe_line(&effort, 240),
        safe_line(&access, 240),
        safe_line(&shared.cwd, 240),
        account_text,
        session_id
    )
}

async fn mcp_message(shared: &SessionShared) -> Result<String, EngineError> {
    let transport = shared.core.transport()?;
    let response = transport
        .request(CodexMethod::MCP_SERVER_STATUS_LIST, &json!({}))
        .await
        .into_result()?;
    let Some(servers) = response.get("data").and_then(Value::as_array) else {
        return Ok("No MCP servers configured.".into());
    };
    let servers = servers
        .iter()
        .filter_map(Value::as_object)
        .collect::<Vec<_>>();
    if servers.is_empty() {
        return Ok("No MCP servers configured.".into());
    }
    Ok(preview_list(
        "Configured MCP servers:",
        &servers,
        |server| {
            let tool_count = server
                .get("tools")
                .and_then(Value::as_object)
                .map_or(0, Map::len);
            let resource_count = server
                .get("resources")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let name = map_text(server, "name").unwrap_or("unnamed");
            let auth = map_text(server, "authStatus").unwrap_or("unknown");
            format!(
                "- {}: {tool_count} tools, {resource_count} resources, auth={}",
                safe_line(name, 240),
                safe_line(auth, 240)
            )
        },
    ))
}

/// One header plus at most `COMMAND_LIST_PREVIEW` rendered items, with a
/// trailing "…and N more" when the list was cut.
fn preview_list<T>(header: &str, items: &[T], render: impl Fn(&T) -> String) -> String {
    let mut lines = items
        .iter()
        .take(COMMAND_LIST_PREVIEW)
        .map(render)
        .collect::<Vec<_>>();
    if items.len() > lines.len() {
        lines.push(format!("- …and {} more", items.len() - lines.len()));
    }
    std::iter::once(header.to_owned())
        .chain(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn skills_message(shared: &SessionShared) -> String {
    let commands = lock(&shared.commands);
    let skills = commands
        .commands
        .iter()
        .filter(|command| command.name.starts_with("skill:"))
        .collect::<Vec<_>>();
    if skills.is_empty() {
        return "No Codex skills are available for this directory.".into();
    }
    preview_list("Available Codex skills:", &skills, |command| {
        format!("- /{}: {}", command.name, command.description)
    })
}

async fn run_immediate_command(shared: &Arc<SessionShared>, command: &str, working_already: bool) {
    if !working_already {
        emit_status(shared, SessionStatus::Working);
    }
    let (result, failure) = match command {
        "status" => (
            Ok(status_message(shared).await),
            "Codex could not read the session status.",
        ),
        "mcp" => (
            mcp_message(shared).await,
            "Codex could not list its MCP servers.",
        ),
        "skills" => {
            let refreshed = refresh_commands(shared, false).await;
            (
                refreshed.then(|| skills_message(shared)).ok_or_else(|| {
                    EngineError::definitely_unsent(
                        EngineErrorKind::Unavailable,
                        "skills/list failed",
                        true,
                    )
                }),
                "Codex could not list its available skills.",
            )
        }
        "logout" => {
            let result = match shared.core.transport() {
                Ok(transport) => transport
                    .request_without_params(CodexMethod::ACCOUNT_LOGOUT)
                    .await
                    .into_result()
                    .map(|_| "Logged out from the Codex account.".to_owned()),
                Err(error) => Err(error),
            };
            (result, "Codex could not sign out.")
        }
        _ => unreachable!("only immediate Codex commands reach this helper"),
    };
    match result {
        Ok(message) => emit_notice(shared, message, NoticeTone::Info),
        Err(_) => emit_notice(shared, failure, NoticeTone::Warning),
    }
    emit_status(shared, SessionStatus::Idle);
}

async fn start_skill_command(
    shared: &Arc<SessionShared>,
    skill: CodexSkillCommand,
    instructions: String,
    working_already: bool,
) {
    if !working_already {
        emit_status(shared, SessionStatus::Working);
    }
    let mut input = vec![TextInput::Skill {
        name: skill.name,
        path: skill.path,
    }];
    if !instructions.is_empty() {
        input.push(TextInput::Text { text: instructions });
    }
    let outcome = start_new_turn(shared, input).await;
    report_control_failure(
        shared,
        &outcome,
        format!(
            "Codex did not confirm whether /{} started. Interface will not retry it automatically.",
            skill.command_name
        ),
        format!("Codex could not start /{}.", skill.command_name),
    );
}

async fn start_review_command(shared: &Arc<SessionShared>, target: Value, working_already: bool) {
    if !working_already {
        emit_status(shared, SessionStatus::Working);
    }
    let outcome = match thread_request_target(shared) {
        Ok((transport, thread_id)) => {
            transport
                .request(
                    CodexMethod::REVIEW_START,
                    &json!({
                        "threadId": thread_id,
                        "target": target,
                        "delivery": "inline"
                    }),
                )
                .await
        }
        Err(error) => DeliveryOutcome::from_error(error),
    };
    lock(&shared.protocol).note_turn_start_outcome(&outcome);
    report_control_failure(
        shared,
        &outcome,
        "Codex did not confirm whether the review started. Interface will not retry it automatically.",
        "Codex could not start the requested review.",
    );
}

async fn start_compact_command(shared: &Arc<SessionShared>, working_already: bool) {
    if !working_already {
        emit_status(shared, SessionStatus::Working);
    }
    {
        let mut commands = lock(&shared.commands);
        commands.compacting = true;
        commands.compact_completion_fence = false;
    }
    lock(&shared.protocol).note_unidentified_control_started();
    emit_notice(
        shared,
        "Codex is compacting this thread's conversation context.",
        NoticeTone::Info,
    );
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::ContextUsage {
            usage: ContextUsage {
                used: None,
                size: None,
                percent: None,
                categories: None,
                auto_compact: None,
                auto_compact_threshold: None,
                compacting: Some(true),
            },
        },
    });
    let outcome = match thread_request_target(shared) {
        Ok((transport, thread_id)) => {
            transport
                .request(
                    CodexMethod::THREAD_COMPACT_START,
                    &json!({"threadId": thread_id}),
                )
                .await
        }
        Err(error) => DeliveryOutcome::from_error(error),
    };
    if outcome.error().is_some() {
        if !matches!(outcome, DeliveryOutcome::OutcomeUnknown(_)) {
            lock(&shared.commands).compacting = false;
            lock(&shared.protocol).note_unidentified_control_completed();
        }
        report_control_failure(
            shared,
            &outcome,
            "Codex did not confirm whether compaction started. Interface will not retry it automatically.",
            "Codex could not start compaction.",
        );
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn dispatch_provider_command(
    shared: &Arc<SessionShared>,
    text: &str,
    working_already: bool,
) -> bool {
    let Some(parsed) = parse_provider_command_text(text) else {
        return false;
    };
    let key = parsed.name.to_ascii_lowercase();
    let builtin = is_codex_builtin_command(&key);
    let (skill, stale, compacting) = {
        let commands = lock(&shared.commands);
        (
            commands.skills.get(&key).cloned(),
            !builtin
                && !commands.skills.contains_key(&key)
                && commands.advertised_names.contains(&key),
            commands.compacting,
        )
    };
    if !builtin && skill.is_none() && !stale {
        return false;
    }
    if lock(&shared.protocol).active_turn_id().is_some() || compacting {
        emit_notice(
            shared,
            format!(
                "Codex did not run /{}: wait for the active turn to finish first.",
                parsed.name
            ),
            NoticeTone::Warning,
        );
        return true;
    }
    if stale {
        command_notice(
            shared,
            working_already,
            format!(
                "/{} is no longer available for this directory.",
                parsed.name
            ),
            NoticeTone::Warning,
        );
        return true;
    }
    if let Some(skill) = skill {
        start_skill_command(shared, skill, parsed.arguments, working_already).await;
        return true;
    }
    let no_arguments = matches!(
        key.as_str(),
        "status" | "mcp" | "skills" | "logout" | "compact"
    );
    if no_arguments && !parsed.arguments.is_empty() {
        command_notice(
            shared,
            working_already,
            format!("Command \"/{}\" takes no arguments.", parsed.name),
            NoticeTone::Warning,
        );
        return true;
    }
    match key.as_str() {
        "status" | "mcp" | "skills" | "logout" => {
            run_immediate_command(shared, &key, working_already).await;
        }
        "compact" => start_compact_command(shared, working_already).await,
        "review" => {
            let target = if parsed.arguments.is_empty() {
                json!({"type": "uncommittedChanges"})
            } else {
                json!({"type": "custom", "instructions": parsed.arguments})
            };
            start_review_command(shared, target, working_already).await;
        }
        "review-branch" => {
            if parsed.arguments.is_empty() {
                command_notice(
                    shared,
                    working_already,
                    "Command \"/review-branch\" requires a branch name.",
                    NoticeTone::Warning,
                );
            } else {
                start_review_command(
                    shared,
                    json!({"type": "baseBranch", "branch": parsed.arguments}),
                    working_already,
                )
                .await;
            }
        }
        "review-commit" => {
            if parsed.arguments.is_empty() {
                command_notice(
                    shared,
                    working_already,
                    "Command \"/review-commit\" requires a commit sha.",
                    NoticeTone::Warning,
                );
            } else {
                start_review_command(
                    shared,
                    json!({"type": "commit", "sha": parsed.arguments, "title": null}),
                    working_already,
                )
                .await;
            }
        }
        _ => return false,
    }
    true
}

pub(super) fn handle_command_notification(
    shared: &Arc<SessionShared>,
    method: &str,
    params: Option<&Value>,
) {
    if method == "skills/changed" {
        spawn_command_refresh(Arc::clone(shared), true);
    }
    let item = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("item"))
        .and_then(Value::as_object);
    let compact_completed = method == "thread/compacted"
        || (method == "item/completed"
            && item.and_then(|item| map_text(item, "type")) == Some("contextCompaction"));
    let manual = {
        let mut commands = lock(&shared.commands);
        if !compact_completed {
            commands.compact_completion_fence = false;
            return;
        }
        if commands.compact_completion_fence {
            return;
        }
        commands.compact_completion_fence = true;
        let manual = commands.compacting;
        commands.compacting = false;
        manual
    };
    if manual {
        lock(&shared.protocol).note_unidentified_control_completed();
    }
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::ContextCompact {
            trigger: if manual {
                ContextCompactTrigger::Manual
            } else {
                ContextCompactTrigger::Unknown
            },
            pre_tokens: None,
            post_tokens: None,
            ok: Some(true),
        },
    });
    if manual {
        emit_status(shared, SessionStatus::Idle);
    }
}
