//! Agent-socket commands against the single shared TUI controller.
use super::{
    AgentController, AgentDelivery, AgentErrorCode, AgentProtocolError, AgentSessionState,
    AgentStatus, LaunchScope, NewSession, QUEUED_PROMPT_LIMIT, StartAgentInput, TuiController,
    agent_text_tail, async_trait, definitely_unsent, delivery_from_runtime, protocol, read_lock,
    require_confirmed, watch, write_lock,
};

#[async_trait]
impl AgentController for TuiController {
    fn agent_sessions(&self) -> Vec<AgentSessionState> {
        read_lock(&self.state)
            .sessions
            .values()
            .map(|row| row.state.clone())
            .collect()
    }

    fn agent_session(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
        read_lock(&self.state)
            .sessions
            .get(key)
            .map(|row| row.state.clone())
            .ok_or_else(|| missing(key))
    }

    fn subscribe_agent_state(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn read_agent(&self, key: &str, lines: usize) -> Result<Vec<String>, AgentProtocolError> {
        let state = read_lock(&self.state);
        let row = state.sessions.get(key).ok_or_else(|| {
            AgentProtocolError::new(AgentErrorCode::NotFound, "Agent session was not found.")
        })?;
        Ok(agent_text_tail(&row.transcript, lines))
    }

    async fn send_agent(
        &self,
        key: &str,
        text: String,
    ) -> Result<AgentDelivery, AgentProtocolError> {
        if send_route(&self.agent_session(key)?)? == SendRoute::Queue {
            // Project the waiting draft before anything is queued: a refused
            // projection then leaves no prompt behind that could still send.
            self.project_draft(key, false)
                .map_err(|message| protocol(AgentErrorCode::Failed, message))?;
            if let Some(delivery) = self.queue_behind_turn(key, &text)? {
                return Ok(delivery);
            }
        }
        Ok(self.send_now(key, text).await)
    }

    async fn steer_agent(
        &self,
        key: &str,
        text: String,
    ) -> Result<AgentDelivery, AgentProtocolError> {
        self.steer_with_history(key, text)
            .await
            .map(|(delivery, _warning)| delivery)
    }

    async fn stop_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
        if !self.agent_session(key)?.live {
            return Err(protocol(
                AgentErrorCode::NotLive,
                "Agent session is not live.",
            ));
        }
        if let Some(collaboration) = &self.collaboration {
            collaboration
                .guard_session_mutation(key, "interrupt", true)
                .await
                .map_err(|message| protocol(AgentErrorCode::Blocked, message))?;
        }
        require_confirmed(delivery_from_runtime(
            self.runtime.cancel_session(key).await,
        ))
        .map_err(|message| protocol(AgentErrorCode::Failed, message))?;
        if let Some(row) = write_lock(&self.state).sessions.get_mut(key) {
            row.state.status = AgentStatus::Idle;
            row.queued.clear();
        }
        self.changed();
        self.agent_session(key)
    }

    async fn resume_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
        self.resume_key(key).await
    }

    async fn start_agent(
        &self,
        input: StartAgentInput,
    ) -> Result<AgentSessionState, AgentProtocolError> {
        let launch = self.launch();
        let engine = input.engine.unwrap_or_else(|| launch.engine.clone());
        let same_engine = engine == launch.engine;
        let cwd = input.cwd.unwrap_or(launch.cwd);
        let model = input
            .model
            .or(if same_engine { launch.model } else { None });
        let effort = input
            .effort
            .or(if same_engine { launch.effort } else { None });
        let scope = input.scope.unwrap_or(if same_engine {
            launch.scope
        } else {
            LaunchScope::Project
        });
        self.open_new(NewSession {
            engine,
            cwd,
            prompt: input.prompt.unwrap_or_default(),
            model,
            effort,
            access: if same_engine { launch.access } else { None },
            fast_mode: false,
            scope,
            name: Some(input.name),
            task: input.task,
        })
        .await
    }

    async fn focus_agent(&self, key: &str) -> Result<AgentSessionState, AgentProtocolError> {
        let state = self.agent_session(key)?;
        if !state.live {
            return Err(protocol(
                AgentErrorCode::NotLive,
                "Agent session is not live.",
            ));
        }
        self.set_active(Some(key.to_owned()));
        Ok(state)
    }
}

impl TuiController {
    /// Queues `text` behind the running turn, deciding the route again under
    /// the lock that pushes: a turn that ended since the caller's read has
    /// already drained the queue, and a prompt pushed then would wait for a
    /// turn that never comes. `None` means the session is idle again and the
    /// prompt goes now.
    pub(super) fn queue_behind_turn(
        &self,
        key: &str,
        text: &str,
    ) -> Result<Option<AgentDelivery>, AgentProtocolError> {
        let mut state = write_lock(&self.state);
        let row = state.sessions.get_mut(key).ok_or_else(|| missing(key))?;
        if send_route(&row.state)? == SendRoute::Now {
            return Ok(None);
        }
        if row.queued.len() >= QUEUED_PROMPT_LIMIT {
            return Ok(Some(definitely_unsent(format!(
                "Agent session already has {QUEUED_PROMPT_LIMIT} queued prompts."
            ))));
        }
        row.queued.push_back(text.to_owned());
        drop(state);
        self.changed();
        Ok(Some(AgentDelivery::accepted(true)))
    }
}

fn missing(key: &str) -> AgentProtocolError {
    AgentProtocolError::new(
        AgentErrorCode::NotFound,
        format!("No agent session matches {key}."),
    )
}

#[derive(Debug, Eq, PartialEq)]
enum SendRoute {
    Now,
    Queue,
}

/// Where a prompt for this session goes: to the provider now, behind the
/// running turn, or nowhere.
fn send_route(state: &AgentSessionState) -> Result<SendRoute, AgentProtocolError> {
    if !state.live {
        return Err(protocol(
            AgentErrorCode::NotLive,
            "Agent session is not live.",
        ));
    }
    match state.status {
        AgentStatus::Blocked => Err(protocol(
            AgentErrorCode::Blocked,
            "Agent session is blocked on an approval.",
        )),
        AgentStatus::Failed => Err(protocol(
            AgentErrorCode::Failed,
            "Agent session has failed.",
        )),
        AgentStatus::Working => Ok(SendRoute::Queue),
        AgentStatus::Idle => Ok(SendRoute::Now),
        AgentStatus::NotLive => unreachable!("live checked above"),
    }
}
