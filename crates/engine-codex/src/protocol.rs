use std::borrow::Cow;

use interface_contracts::engine::SandboxMode;
use interface_engine::{DeliveryOutcome, EngineError, EngineErrorKind};
use serde::Serialize;
use serde_json::Value;

pub struct CodexMethod;

impl CodexMethod {
    pub const INITIALIZE: &'static str = "initialize";
    pub const MODEL_LIST: &'static str = "model/list";
    pub const ACCOUNT_READ: &'static str = "account/read";
    pub const ACCOUNT_LOGOUT: &'static str = "account/logout";
    pub const ACCOUNT_RATE_LIMITS_READ: &'static str = "account/rateLimits/read";
    pub const ACCOUNT_USAGE_READ: &'static str = "account/usage/read";
    pub const MCP_SERVER_STATUS_LIST: &'static str = "mcpServerStatus/list";
    pub const CONFIG_READ: &'static str = "config/read";
    pub const SKILLS_LIST: &'static str = "skills/list";
    pub const THREAD_START: &'static str = "thread/start";
    pub const THREAD_RESUME: &'static str = "thread/resume";
    pub const THREAD_LIST: &'static str = "thread/list";
    pub const THREAD_READ: &'static str = "thread/read";
    pub const THREAD_SET_NAME: &'static str = "thread/name/set";
    pub const THREAD_DELETE: &'static str = "thread/delete";
    pub const THREAD_FORK: &'static str = "thread/fork";
    pub const THREAD_ARCHIVE: &'static str = "thread/archive";
    pub const THREAD_UNARCHIVE: &'static str = "thread/unarchive";
    pub const THREAD_COMPACT_START: &'static str = "thread/compact/start";
    pub const REVIEW_START: &'static str = "review/start";
    pub const TURN_START: &'static str = "turn/start";
    pub const TURN_STEER: &'static str = "turn/steer";
    pub const TURN_INTERRUPT: &'static str = "turn/interrupt";
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_info: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
}

impl InitializeParams {
    #[must_use]
    pub fn interface(version: impl Into<String>) -> Self {
        Self {
            capabilities: None,
            client_info: ClientInfo {
                name: "Interface".into(),
                version: version.into(),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    /// `config.toml` overrides; set only for a gated agent launch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    pub thread_id: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub developer_instructions: Option<String>,
    /// Gated resumes explicitly clear persisted runtime roots; current extra roots
    /// come only from the separately verified sandbox configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_workspace_roots: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxMode>,
    /// `config.toml` overrides; set only for a gated agent resume.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TextInput {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "skill")]
    Skill { name: String, path: String },
}

/// Model-visible marker for a host-injected turn part. The value identifies a
/// collaboration source and authorizes nothing; the marker exists so a
/// replayed transcript never renders the block as the user's own text.
pub(crate) const COLLABORATION_HOST_CONTEXT_PREFIX: &str =
    "[Interface host context — not user text]";

/// The exact host-context part sent as the leading input of a turn.
pub(crate) fn host_context_block(instruction: &str) -> String {
    format!("{COLLABORATION_HOST_CONTEXT_PREFIX}\n{instruction}")
}

/// Provider echoes of the host-context part are not user text.
pub(crate) fn is_collaboration_host_context(text: &str) -> bool {
    text.trim_start()
        .starts_with(COLLABORATION_HOST_CONTEXT_PREFIX)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams<'a> {
    pub thread_id: String,
    pub input: Cow<'a, [TextInput]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams<'a> {
    pub thread_id: String,
    pub expected_turn_id: String,
    /// Borrowed for a steer so the same input can still be moved into a
    /// `turn/start` fallback without a second copy.
    pub input: Cow<'a, [TextInput]>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodexSessionState {
    thread_id: Option<String>,
    active_turn_id: Option<String>,
    /// The most recent terminal notification can race ahead of the matching
    /// `turn/start` response on the independent inbound task. Remember it so
    /// adopting that response cannot resurrect an already-completed turn.
    last_completed_turn_id: Option<String>,
    turn_start_unknown: bool,
}

impl CodexSessionState {
    #[must_use]
    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    #[must_use]
    pub fn active_turn_id(&self) -> Option<&str> {
        self.active_turn_id.as_deref()
    }

    #[must_use]
    pub const fn turn_start_unknown(&self) -> bool {
        self.turn_start_unknown
    }

    /// Promote a provisional session to the provider id in a start/resume response.
    ///
    /// # Errors
    ///
    /// Returns a definite protocol rejection when the response contains no thread id.
    pub fn adopt_thread_response(&mut self, response: &Value) -> Result<&str, EngineError> {
        let id = thread_id_from_response(response).ok_or_else(|| {
            EngineError::definitely_rejected(
                EngineErrorKind::Protocol,
                "codex app-server started a thread without an id",
                false,
            )
        })?;
        Ok(self.thread_id.insert(id.to_owned()))
    }

    pub fn note_turn_started(&mut self, turn_id: impl Into<String>) {
        let turn_id = turn_id.into();
        if self.last_completed_turn_id.as_deref() == Some(turn_id.as_str()) {
            if self.active_turn_id.as_deref() == Some(turn_id.as_str()) {
                self.active_turn_id = None;
            }
            self.turn_start_unknown = false;
            return;
        }
        self.active_turn_id = Some(turn_id);
        self.turn_start_unknown = false;
    }

    pub fn note_turn_completed(&mut self, completed_id: &str) {
        self.last_completed_turn_id = Some(completed_id.to_owned());
        if self.active_turn_id.as_deref() == Some(completed_id) {
            self.active_turn_id = None;
        }
    }

    /// Clear only the exact turn whose interrupt the provider accepted.
    pub fn note_turn_interrupted(&mut self, interrupted_id: &str) {
        if self.active_turn_id.as_deref() == Some(interrupted_id) {
            self.active_turn_id = None;
        }
    }

    pub fn note_turn_start_outcome(&mut self, outcome: &DeliveryOutcome<Value>) {
        match outcome {
            DeliveryOutcome::Confirmed(response) => {
                if let Some(turn_id) = turn_id_from_response(response) {
                    self.note_turn_started(turn_id);
                }
                self.turn_start_unknown = false;
            }
            DeliveryOutcome::OutcomeUnknown(_) if self.active_turn_id.is_none() => {
                self.turn_start_unknown = true;
            }
            DeliveryOutcome::DefinitelyUnsent(_) | DeliveryOutcome::DefinitelyRejected(_) => {
                self.turn_start_unknown = false;
            }
            DeliveryOutcome::OutcomeUnknown(_) => {}
        }
    }

    /// Fence a provider control that may be running but has no turn id yet.
    /// Manual compaction is confirmed by its own notification, not its empty
    /// start response, so sends and Stop must remain fail-closed meanwhile.
    pub fn note_unidentified_control_started(&mut self) {
        if self.active_turn_id.is_none() {
            self.turn_start_unknown = true;
        }
    }

    /// Release the unidentified-control fence after definite refusal or the
    /// provider's independent completion notification.
    pub fn note_unidentified_control_completed(&mut self) {
        self.turn_start_unknown = false;
    }

    /// Refuse a new prompt while an earlier turn may already be running.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error for a quarantined turn.
    pub fn require_new_prompt_safe(&self) -> Result<(), EngineError> {
        if self.turn_start_unknown {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex may already be running the previous prompt; no new prompt was sent",
                false,
            ));
        }
        Ok(())
    }

    /// Build an interrupt only when both exact provider ids are known.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error when an accepted turn may lack an id.
    pub fn interrupt_params(&self) -> Result<Option<TurnInterruptParams>, EngineError> {
        if self.turn_start_unknown && self.active_turn_id.is_none() {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex may have accepted a turn but has not supplied its turn id; no interrupt was sent",
                true,
            ));
        }
        let Some(turn_id) = &self.active_turn_id else {
            return Ok(None);
        };
        let thread_id = self.thread_id.clone().ok_or_else(|| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex thread is not ready; no interrupt was sent",
                true,
            )
        })?;
        Ok(Some(TurnInterruptParams {
            thread_id,
            turn_id: turn_id.clone(),
        }))
    }
}

#[must_use]
pub fn thread_id_from_response(response: &Value) -> Option<&str> {
    response
        .get("thread")?
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty())
}

#[must_use]
pub fn turn_id_from_response(response: &Value) -> Option<&str> {
    response
        .get("turn")?
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use interface_engine::{
        EffectCertainty,
        jsonrpc::{
            PendingRouter, RouteEvent, RpcCallOutcome, RpcId, RpcRemoteError, delivery_from_rpc,
        },
        supervised::Wire,
    };
    use serde_json::json;

    use super::*;
    use crate::transport::CodexTransportEvent;

    #[test]
    fn typed_turn_start_keeps_exact_wire_field_names() {
        let value = serde_json::to_value(TurnStartParams {
            thread_id: "thread-1".into(),
            input: Cow::Owned(vec![TextInput::Text {
                text: "hello".into(),
            }]),
            model: Some("gpt-test".into()),
            effort: Some("high".into()),
            service_tier: Some(Some("priority".into())),
            approval_policy: Some("never".into()),
        })
        .unwrap();
        assert_eq!(
            value,
            json!({
                "threadId": "thread-1",
                "input": [{"type": "text", "text": "hello"}],
                "model": "gpt-test",
                "effort": "high",
                "serviceTier": "priority",
                "approvalPolicy": "never"
            })
        );
    }

    #[test]
    fn completion_arriving_before_the_start_response_does_not_resurrect_the_turn() {
        let mut state = CodexSessionState::default();
        state.note_turn_completed("turn-raced");
        state.note_turn_start_outcome(&DeliveryOutcome::Confirmed(json!({
            "turn": {"id": "turn-raced"}
        })));
        assert_eq!(state.active_turn_id(), None);
        assert!(!state.turn_start_unknown());
    }

    #[test]
    fn accepted_interrupt_clears_only_the_exact_active_turn() {
        let mut state = CodexSessionState::default();
        state
            .adopt_thread_response(&json!({"thread": {"id": "thread-one"}}))
            .unwrap();
        state.note_turn_started("turn-one");
        state.note_turn_interrupted("turn-other");
        assert_eq!(state.active_turn_id(), Some("turn-one"));
        state.note_turn_interrupted("turn-one");
        assert_eq!(state.active_turn_id(), None);
    }

    #[tokio::test]
    async fn fake_app_server_response_settles_the_matching_request() {
        let mut wire =
            Wire::<CodexTransportEvent>::with_router(PendingRouter::new(8, 4_096), 4_096);
        let request = wire
            .router()
            .prepare_request(
                CodexMethod::INITIALIZE,
                Some(serde_json::to_value(InitializeParams::interface("0.2.0")).unwrap()),
            )
            .unwrap();
        assert_eq!(request.frame.last(), Some(&b'\n'));
        assert!(wire.router().mark_write_started(&request.id));
        assert!(wire.router().mark_frame_accepted(&request.id));
        let mut inbound = Vec::new();
        wire.push_stdout(
            b"{\"id\":1,\"result\":{\"server\":\"fake\"}}\n",
            &mut inbound,
        );
        assert_eq!(
            inbound,
            vec![CodexTransportEvent::Route(RouteEvent::ResponseSettled {
                id: RpcId::Number(1)
            })]
        );
        assert_eq!(
            delivery_from_rpc(request.completion.outcome().await),
            DeliveryOutcome::Confirmed(json!({"server": "fake"}))
        );
    }

    #[test]
    fn unknown_turn_outcome_blocks_replay_and_stop_without_an_id() {
        let mut session = CodexSessionState::default();
        session
            .adopt_thread_response(&json!({"thread": {"id": "thread-1"}}))
            .unwrap();
        let unknown = DeliveryOutcome::OutcomeUnknown(EngineError::outcome_unknown(
            EngineErrorKind::Transport,
            "fake response lost",
            true,
        ));
        session.note_turn_start_outcome(&unknown);
        let send_error = session.require_new_prompt_safe().unwrap_err();
        assert_eq!(send_error.certainty, EffectCertainty::DefinitelyUnsent);
        assert!(session.interrupt_params().is_err());
    }

    #[test]
    fn unidentified_compaction_fences_send_and_stop_until_completion() {
        let mut session = CodexSessionState::default();
        session
            .adopt_thread_response(&json!({"thread": {"id": "thread-1"}}))
            .unwrap();
        session.note_unidentified_control_started();
        assert!(session.require_new_prompt_safe().is_err());
        assert!(session.interrupt_params().is_err());

        session.note_unidentified_control_completed();
        assert!(session.require_new_prompt_safe().is_ok());
        assert_eq!(session.interrupt_params().unwrap(), None);
    }

    #[test]
    fn structured_provider_error_is_definite_rejection_not_unknown() {
        let outcome = delivery_from_rpc(RpcCallOutcome::Rejected(RpcRemoteError {
            code: -32_600,
            message: "turn/start: invalid thread".into(),
            data: None,
        }));
        assert!(matches!(outcome, DeliveryOutcome::DefinitelyRejected(_)));
    }
}
