//! Orchestration commands (/agents, /orchestrate, /spawn, /send, /inbox,
//! /monitor) over the shared orchestration host.
use interface_contracts::ipc::{
    OrchestrationAgentSteerRequest, OrchestrationAgentStopRequest, OrchestrationInboxRequest,
    OrchestrationTimelineRequest,
};
use interface_contracts::orchestration::MailboxMessageV1;
use interface_contracts::orchestration_control::{
    OrchestrationAgentCreateRequestV1, OrchestrationAgentPageV1, OrchestrationAgentSendRequestV1,
    OrchestrationControlAgentV1, OrchestrationControlMethod, OrchestrationControlSnapshotV1,
    OrchestrationEventPageV1, OrchestrationInboxPageV1,
};
use serde::{Serialize, de::DeserializeOwned};

use super::commands::{self, parse_send_argument, parse_spawn_argument};
use super::{
    CHANNELS, OrchestrationHostControl, TranscriptCell, TuiApp, TuiController, for_display,
    read_lock,
};
use crate::argv::is_engine_id;

impl TuiController {
    async fn dispatch_orchestration<Request, Response>(
        &self,
        operation: &str,
        request: &Request,
    ) -> Result<Response, String>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        self.orchestration_control()?
            .invoke_channel(operation, request)
            .await
    }

    async fn invoke_orchestration<Request, Response>(
        &self,
        method: OrchestrationControlMethod,
        request: &Request,
    ) -> Result<Response, String>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        self.orchestration_control()?.invoke(method, request).await
    }

    /// The first page of orchestrated agents, which /agents orchestration,
    /// /spawn and /inbox each read.
    async fn orchestration_agents(&self) -> Result<OrchestrationAgentPageV1, String> {
        self.invoke_orchestration(
            OrchestrationControlMethod::AgentList,
            &serde_json::json!({ "limit": 32 }),
        )
        .await
    }

    fn orchestration_control(&self) -> Result<&OrchestrationHostControl, String> {
        self.orchestration
            .as_ref()
            .ok_or_else(|| "The shared orchestration host did not start.".to_owned())
    }
}

impl TuiApp {
    #[allow(clippy::too_many_lines)]
    pub(super) async fn agents_command(&mut self, argument: &str) {
        if argument == "orchestration" || argument.starts_with("orchestration ") {
            self.orchestration_agents_command(
                argument
                    .strip_prefix("orchestration")
                    .unwrap_or_default()
                    .trim(),
            )
            .await;
            return;
        }
        let Some(key) = self.controller.active_key() else {
            self.notice = Some("Select a session before listing its children.".into());
            return;
        };
        let state = read_lock(&self.controller.state);
        let Some(session) = state.sessions.get(&key) else {
            return;
        };
        let mut rows = session
            .transcript
            .iter()
            .filter_map(|cell| match cell {
                TranscriptCell::Subagent {
                    agent_key,
                    label,
                    description,
                    result,
                    ..
                } => Some((agent_key, label, description, result)),
                _ => None,
            })
            .collect::<Vec<_>>();
        rows.dedup_by(|left, right| left.0 == right.0);
        if argument.is_empty() {
            let lines = if rows.is_empty() {
                vec!["(no live children)".into()]
            } else {
                rows.iter()
                    .map(|(id, label, description, result)| {
                        format!(
                            "{} {} {} {} [inspect] {}",
                            for_display(id),
                            for_display(label),
                            for_display(id),
                            if result.is_some() {
                                "finished"
                            } else {
                                "working"
                            },
                            for_display(description)
                        )
                    })
                    .collect()
            };
            drop(state);
            self.show_details(
                "Live children",
                lines
                    .into_iter()
                    .chain(std::iter::once(
                        "/agents <id> or /agents inspect <id>".into(),
                    ))
                    .collect(),
            );
            return;
        }
        let (verb, id) = argument
            .split_once(char::is_whitespace)
            .map_or(("inspect", argument), |(verb, id)| (verb, id.trim()));
        if matches!(verb, "steer" | "stop") {
            drop(state);
            self.notice = Some("The current bridge has no child-targeted steer or stop operation. Parent-session controls were not used as a substitute.".into());
            return;
        }
        let selected = rows.iter().find(|(candidate, ..)| candidate.as_str() == id);
        let Some((id, label, description, result)) = selected else {
            drop(state);
            self.notice = Some(format!("No child agent has id {id}."));
            return;
        };
        let title = (*id).clone();
        let lines = vec![
            format!(
                "{} {} {}",
                for_display(id),
                for_display(label),
                if result.is_some() {
                    "finished"
                } else {
                    "working"
                }
            ),
            for_display(description),
        ];
        drop(state);
        self.show_details(title, lines);
    }

    pub(super) async fn orchestration_status_command(&mut self) {
        let result = self
            .controller
            .invoke_orchestration::<_, OrchestrationControlSnapshotV1>(
                OrchestrationControlMethod::OrchestrationSnapshot,
                &serde_json::json!({}),
            )
            .await;
        match result {
            Ok(snapshot) => {
                let mut lines = vec![format!(
                    "revision {} · {} agents",
                    snapshot.revision,
                    snapshot.agents.len()
                )];
                lines.extend(snapshot.agents.iter().map(|agent| {
                    format!(
                        "{} {} {}",
                        agent.agent_id,
                        enum_text(agent.state),
                        agent.engine_id
                    )
                }));
                lines.push("/agents orchestration · /inbox".into());
                self.show_details("Orchestration agents", lines);
            }
            Err(error) => self.notice = Some(error),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn orchestration_agents_command(&mut self, argument: &str) {
        let tokens = argument.split_whitespace().collect::<Vec<_>>();
        if tokens.first() == Some(&"stop") {
            if tokens.len() != 2 {
                self.notice = Some("Use /agents orchestration stop <agent-id>.".into());
                return;
            }
            let result = self
                .controller
                .dispatch_orchestration::<_, OrchestrationControlAgentV1>(
                    CHANNELS.stop_orchestration_agent,
                    &OrchestrationAgentStopRequest {
                        agent_id: tokens[1].into(),
                    },
                )
                .await;
            self.notice = Some(agent_state_notice(result));
            return;
        }
        if tokens.first() == Some(&"steer") {
            let body = tokens.get(2..).unwrap_or_default().join(" ");
            let Some(agent_id) = tokens.get(1).filter(|_| !body.is_empty()) else {
                self.notice = Some("Use /agents orchestration steer <agent-id> <message>.".into());
                return;
            };
            let result = self
                .controller
                .dispatch_orchestration::<_, OrchestrationControlAgentV1>(
                    CHANNELS.steer_orchestration_agent,
                    &OrchestrationAgentSteerRequest {
                        agent_id: (*agent_id).into(),
                        body,
                        client_request_id: uuid::Uuid::new_v4().to_string(),
                    },
                )
                .await;
            self.notice = Some(agent_state_notice(result));
            return;
        }
        let result = self.controller.orchestration_agents().await;
        match result {
            Ok(page) => {
                let selected = (!argument.is_empty())
                    .then(|| page.agents.iter().find(|agent| agent.agent_id == argument))
                    .flatten();
                if !argument.is_empty() && selected.is_none() {
                    self.notice = Some(format!("No orchestrated agent has id {argument}."));
                    return;
                }
                let lines = selected.map_or_else(
                    || {
                        if page.agents.is_empty() {
                            vec!["(no orchestrated agents)".into()]
                        } else {
                            page.agents
                                .iter()
                                .map(|agent| {
                                    format!(
                                        "{} {} {} {}",
                                        agent.agent_id,
                                        enum_text(agent.state),
                                        agent.engine_id,
                                        for_display(agent.role.as_deref().unwrap_or("-"))
                                    )
                                })
                                .collect()
                        }
                    },
                    |agent| {
                        vec![
                            format!(
                                "{} {} {}",
                                agent.agent_id,
                                enum_text(agent.state),
                                agent.engine_id
                            ),
                            format!(
                                "session {}",
                                agent.session_key.as_deref().unwrap_or("(not assigned)")
                            ),
                            format!(
                                "role {}",
                                for_display(agent.role.as_deref().unwrap_or("(none)"))
                            ),
                            format!("cwd {}", for_display(&agent.scope.canonical_cwd)),
                            format!(
                                "capabilities {}",
                                agent
                                    .capabilities
                                    .iter()
                                    .filter(|item| enum_text(item.state) == "available")
                                    .map(|item| enum_text(item.facet))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        ]
                    },
                );
                self.show_details(
                    selected.map_or_else(
                        || "Orchestrated agents".into(),
                        |agent| agent.agent_id.clone(),
                    ),
                    lines
                        .into_iter()
                        .chain(std::iter::once(
                            "/agents orchestration <id> · stop <id> · steer <id> <text>".into(),
                        ))
                        .collect(),
                );
            }
            Err(error) => self.notice = Some(error),
        }
    }

    pub(super) async fn spawn_orchestration_agent(&mut self, argument: &str) {
        let Some(command) = parse_spawn_argument(argument) else {
            self.notice = Some(
                "Use /spawn <engine> <name> [--model <id>] [--effort <level>] <prompt>.".into(),
            );
            return;
        };
        let commands::SpawnArgument {
            engine,
            name,
            prompt,
            model,
            effort,
        } = command;
        if !is_engine_id(engine) {
            self.notice = Some("Engine must be claude, codex, or acp:<slug>.".into());
            return;
        }
        let agents = match self.controller.orchestration_agents().await {
            Ok(agents) => agents,
            Err(error) => {
                self.notice = Some(error);
                return;
            }
        };
        let active = self.controller.active_key();
        let parent = active.as_deref().and_then(|key| {
            agents
                .agents
                .iter()
                .find(|agent| agent.session_key.as_deref() == Some(key))
        });
        let launch = self.controller.launch();
        let provider_access = if engine == launch.engine {
            launch.access
        } else {
            self.config.get().access_mode_for_launch(engine)
        };
        let request = OrchestrationAgentCreateRequestV1 {
            agent_id: None,
            engine_id: engine.into(),
            name: name.into(),
            prompt: prompt.into(),
            canonical_cwd: parent.map_or_else(
                || launch.cwd.clone(),
                |agent| agent.scope.canonical_cwd.clone(),
            ),
            access_mode: parent.map_or_else(
                || {
                    if provider_access.as_deref() == Some("read-only") {
                        "read-only".into()
                    } else {
                        "project".into()
                    }
                },
                |agent| agent.scope.access_mode.clone(),
            ),
            model: model.map(Into::into),
            effort: effort.map(Into::into),
            parent_agent_id: parent.map(|agent| agent.agent_id.clone()),
            role: Some(name.into()),
            required_capabilities: None,
            client_request_id: uuid::Uuid::new_v4().to_string(),
        };
        let result = self
            .controller
            .dispatch_orchestration::<_, OrchestrationControlAgentV1>(
                CHANNELS.create_orchestration_agent,
                &request,
            )
            .await;
        self.notice = Some(agent_state_notice(result));
    }

    pub(super) async fn send_orchestration_message(&mut self, argument: &str) {
        let Some(command) = parse_send_argument(argument) else {
            self.notice = Some(
                "Use /send [--from <agent-id>] <recipient-agent-id> [--expect-reply] <message>."
                    .into(),
            );
            return;
        };
        let result = self
            .controller
            .dispatch_orchestration::<_, MailboxMessageV1>(
                CHANNELS.send_orchestration_message,
                &OrchestrationAgentSendRequestV1 {
                    recipient_agent_id: command.recipient_agent_id,
                    body: command.body,
                    expect_reply: command.expect_reply,
                    client_request_id: uuid::Uuid::new_v4().to_string(),
                    sender_agent_id: command.sender_agent_id,
                    thread_id: None,
                    parent_message_id: None,
                },
            )
            .await;
        self.notice = Some(match result {
            Ok(message) => {
                let receipt = message
                    .receipts
                    .last()
                    .map_or_else(|| "requested".into(), |row| enum_text(row.state));
                format!("{} is {receipt}.", message.message_id)
            }
            Err(error) => for_display(&error),
        });
    }

    pub(super) async fn orchestration_inbox_command(&mut self, argument: &str) {
        let agents = self.controller.orchestration_agents().await;
        let Ok(agents) = agents else {
            self.notice = agents.err().map(|error| for_display(&error));
            return;
        };
        let requested = argument.trim();
        let active = self.controller.active_key();
        let selected = if requested.is_empty() {
            agents
                .agents
                .iter()
                .find(|agent| agent.session_key.as_deref() == active.as_deref())
                .or_else(|| (agents.agents.len() == 1).then(|| &agents.agents[0]))
        } else {
            agents
                .agents
                .iter()
                .find(|agent| agent.agent_id == requested)
        };
        let Some(selected) = selected else {
            self.notice = Some(
                "Use /inbox <agent-id>, or select the provider session for one orchestrated agent."
                    .into(),
            );
            return;
        };
        let agent_id = selected.agent_id.clone();
        let result = self
            .controller
            .invoke_orchestration::<_, OrchestrationInboxPageV1>(
                OrchestrationControlMethod::AgentInbox,
                &OrchestrationInboxRequest {
                    agent_id: agent_id.clone(),
                    thread_id: None,
                    before_message_id: None,
                    limit: Some(4),
                },
            )
            .await;
        match result {
            Ok(page) => {
                let lines = if page.messages.is_empty() {
                    vec!["(inbox empty)".into()]
                } else {
                    page.messages
                        .iter()
                        .map(|message| {
                            let state = message
                                .receipts
                                .last()
                                .map_or_else(|| "requested".into(), |row| enum_text(row.state));
                            format!(
                                "{} [{state}] {}",
                                message.message_id,
                                for_display(message.body.as_deref().unwrap_or("(body pruned)"))
                            )
                        })
                        .collect()
                };
                self.show_details(format!("Inbox · {agent_id}"), lines);
            }
            Err(error) => self.notice = Some(error),
        }
    }

    pub(super) async fn monitor_orchestration(&mut self, argument: &str) {
        let sequence = if argument.trim().is_empty() {
            Some(0)
        } else {
            argument.trim().parse::<u64>().ok()
        };
        let Some(sequence) = sequence else {
            self.notice = Some("Use /monitor [non-negative-sequence].".into());
            return;
        };
        let result = self
            .controller
            .invoke_orchestration::<_, OrchestrationEventPageV1>(
                OrchestrationControlMethod::EventsSince,
                &OrchestrationTimelineRequest {
                    sequence,
                    limit: Some(64),
                },
            )
            .await;
        match result {
            Ok(page) => {
                let mut lines = if page.events.is_empty() {
                    vec!["(no newer events)".into()]
                } else {
                    page.events
                        .iter()
                        .map(|event| {
                            format!(
                                "{} {} {} {} · {}",
                                event.sequence,
                                event.at,
                                event.kind,
                                event.entity_id,
                                for_display(&event.summary)
                            )
                        })
                        .collect()
                };
                if page.truncated {
                    lines.insert(
                        0,
                        format!("(history compacted through {})", page.floor_sequence),
                    );
                }
                if page.has_more {
                    lines.push(format!("(more events after {})", page.next_sequence));
                }
                lines.push("/monitor <last-sequence> shows only newer events".into());
                self.show_details("Orchestration timeline", lines);
            }
            Err(error) => self.notice = Some(error),
        }
    }
}

fn agent_state_notice(result: Result<OrchestrationControlAgentV1, String>) -> String {
    match result {
        Ok(agent) => format!("{} is {}.", agent.agent_id, enum_text(agent.state)),
        Err(error) => for_display(&error),
    }
}

fn enum_text(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
