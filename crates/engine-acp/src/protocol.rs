use interface_engine::{
    DeliveryOutcome, EngineError, EngineErrorKind,
    jsonrpc::{FrameError, RouteEvent, RpcDiagnostic, RpcId, RpcNotification, RpcServerRequest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, from_value};

pub const ACP_PROTOCOL_VERSION: u32 = 1;

pub struct AcpMethod;

impl AcpMethod {
    pub const INITIALIZE: &'static str = "initialize";
    pub const SESSION_NEW: &'static str = "session/new";
    pub const SESSION_LOAD: &'static str = "session/load";
    pub const SESSION_RESUME: &'static str = "session/resume";
    pub const SESSION_PROMPT: &'static str = "session/prompt";
    pub const SESSION_CANCEL: &'static str = "session/cancel";
    pub const SESSION_UPDATE: &'static str = "session/update";
    pub const SESSION_REQUEST_PERMISSION: &'static str = "session/request_permission";
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCapabilities {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitializeParams {
    pub protocol_version: u32,
    pub client_capabilities: ClientCapabilities,
    pub client_info: ClientInfo,
}

impl InitializeParams {
    #[must_use]
    pub fn interface(version: impl Into<String>) -> Self {
        Self {
            protocol_version: ACP_PROTOCOL_VERSION,
            client_capabilities: ClientCapabilities {},
            client_info: ClientInfo {
                name: "interface".into(),
                title: "Interface".into(),
                version: version.into(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpEnvironmentVariable {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcpMcpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<AcpEnvironmentVariable>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo_mode: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionNewParams {
    pub cwd: String,
    pub mcp_servers: Vec<AcpMcpServer>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<SessionMetadata>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionOpenParams {
    pub session_id: String,
    pub cwd: String,
    pub mcp_servers: Vec<AcpMcpServer>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpContentBlock {
    Text { text: String },
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo_mode: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionPromptParams {
    pub session_id: String,
    pub prompt: Vec<AcpContentBlock>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<PromptMetadata>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CancelMetadata {
    pub cancelled_by: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SessionCancelParams {
    pub session_id: String,
    #[serde(rename = "_meta")]
    pub metadata: CancelMetadata,
}

impl SessionCancelParams {
    #[must_use]
    pub fn mouse(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            metadata: CancelMetadata {
                cancelled_by: "mouse".into(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequestParams {
    pub session_id: String,
    pub tool_call: Value,
    pub options: Vec<PermissionOption>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedPermissionOutcome {
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionResponse {
    pub outcome: SelectedPermissionOutcome,
}

/// Select only an option actually offered by the ACP agent.
#[must_use]
pub fn permission_response(
    options: &[PermissionOption],
    allow: bool,
    persistent_allow: bool,
) -> PermissionResponse {
    let preferred: &[&str] = if allow {
        if persistent_allow {
            &["allow_once", "allow_always"]
        } else {
            &["allow_once"]
        }
    } else {
        &["reject_once", "reject_always"]
    };
    for kind in preferred {
        if let Some(option) = options.iter().find(|option| option.kind == *kind) {
            return PermissionResponse {
                outcome: SelectedPermissionOutcome {
                    outcome: "selected".into(),
                    option_id: Some(option.option_id.clone()),
                },
            };
        }
    }
    PermissionResponse {
        outcome: SelectedPermissionOutcome {
            outcome: "cancelled".into(),
            option_id: None,
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateKind {
    AgentMessageChunk,
    UserMessageChunk,
    AgentThoughtChunk,
    ToolCall,
    ToolCallUpdate,
    Plan,
    AvailableCommandsUpdate,
    UsageUpdate,
    Unknown,
}

/// Lossless ACP update envelope. Known kinds are classified without dropping
/// provider extension fields needed by later adapters.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AcpSessionUpdate {
    #[serde(rename = "sessionUpdate")]
    pub session_update: String,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

impl AcpSessionUpdate {
    #[must_use]
    pub fn kind(&self) -> UpdateKind {
        match self.session_update.as_str() {
            "agent_message_chunk" => UpdateKind::AgentMessageChunk,
            "user_message_chunk" => UpdateKind::UserMessageChunk,
            "agent_thought_chunk" => UpdateKind::AgentThoughtChunk,
            "tool_call" => UpdateKind::ToolCall,
            "tool_call_update" => UpdateKind::ToolCallUpdate,
            "plan" => UpdateKind::Plan,
            "available_commands_update" => UpdateKind::AvailableCommandsUpdate,
            "usage_update" => UpdateKind::UsageUpdate,
            _ => UpdateKind::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AcpInbound {
    SessionUpdate {
        session_id: String,
        update: AcpSessionUpdate,
    },
    PermissionRequest {
        id: RpcId,
        request: PermissionRequestParams,
    },
    Notification(RpcNotification),
    ServerRequest(RpcServerRequest),
    ResponseSettled {
        id: RpcId,
    },
    Diagnostic(RpcDiagnostic),
    FramingError(FrameError),
    InvalidMessage(String),
}

/// Decodes `session/update` params by moving the owned fields out, instead of
/// a serde `flatten` derive that buffers and re-copies every update (token
/// chunks, whole tool-call diffs). Accepts and rejects exactly what that
/// derive did: unknown params keys are ignored, wrong types are rejected,
/// and positional `[sessionId, update]` params still decode.
fn session_update(params: Option<Value>) -> Option<AcpInbound> {
    let (session_id, update) = match params? {
        Value::Object(mut params) => (params.remove("sessionId")?, params.remove("update")?),
        Value::Array(params) => {
            let [session_id, update] = <[Value; 2]>::try_from(params).ok()?;
            (session_id, update)
        }
        _ => return None,
    };
    let Value::String(session_id) = session_id else {
        return None;
    };
    let Value::Object(mut fields) = update else {
        return None;
    };
    let Some(Value::String(session_update)) = fields.remove("sessionUpdate") else {
        return None;
    };
    Some(AcpInbound::SessionUpdate {
        session_id,
        update: AcpSessionUpdate {
            session_update,
            fields,
        },
    })
}

pub(crate) fn classify_route(route: RouteEvent) -> AcpInbound {
    match route {
        RouteEvent::Notification(notification)
            if notification.method == AcpMethod::SESSION_UPDATE =>
        {
            session_update(notification.params).unwrap_or_else(|| {
                AcpInbound::InvalidMessage("invalid session/update params".into())
            })
        }
        RouteEvent::ServerRequest(request)
            if request.method == AcpMethod::SESSION_REQUEST_PERMISSION =>
        {
            match request.params.map(from_value::<PermissionRequestParams>) {
                Some(Ok(params)) => AcpInbound::PermissionRequest {
                    id: request.id,
                    request: params,
                },
                _ => AcpInbound::InvalidMessage("invalid session/request_permission params".into()),
            }
        }
        RouteEvent::Notification(notification) => AcpInbound::Notification(notification),
        RouteEvent::ServerRequest(request) => AcpInbound::ServerRequest(request),
        RouteEvent::ResponseSettled { id } => AcpInbound::ResponseSettled { id },
        RouteEvent::Diagnostic(diagnostic) => AcpInbound::Diagnostic(diagnostic),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpPhase {
    New,
    Initializing,
    Initialized,
    Opening,
    Ready,
    Prompting,
    PromptOutcomeUnknown,
    Closing,
    Closed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcpResumeMethod {
    Resume,
    Load,
}

impl AcpResumeMethod {
    #[must_use]
    pub const fn method(self) -> &'static str {
        match self {
            Self::Resume => AcpMethod::SESSION_RESUME,
            Self::Load => AcpMethod::SESSION_LOAD,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AcpSessionState {
    phase: AcpPhase,
    session_id: Option<String>,
    agent_capabilities: Option<Value>,
}

impl Default for AcpSessionState {
    fn default() -> Self {
        Self {
            phase: AcpPhase::New,
            session_id: None,
            agent_capabilities: None,
        }
    }
}

impl AcpSessionState {
    #[must_use]
    pub const fn phase(&self) -> AcpPhase {
        self.phase
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Start the mandatory first ACP request.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error unless the wire is new.
    pub fn begin_initialize(&mut self) -> Result<(), EngineError> {
        self.require_phase(AcpPhase::New, "initialize")?;
        self.phase = AcpPhase::Initializing;
        Ok(())
    }

    /// Validate the ACP v1 selection and retain advertised capabilities.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle error out of order or a definite protocol rejection for non-v1.
    pub fn accept_initialize(&mut self, result: &Value) -> Result<(), EngineError> {
        self.require_phase(AcpPhase::Initializing, "accept initialize")?;
        if !selected_protocol_version_is_supported(result) {
            self.phase = AcpPhase::Failed;
            return Err(EngineError::definitely_rejected(
                EngineErrorKind::Protocol,
                "ACP agent selected an unsupported protocol version",
                false,
            ));
        }
        self.agent_capabilities = result.get("agentCapabilities").cloned();
        self.phase = AcpPhase::Initialized;
        Ok(())
    }

    /// Permit `session/new`, `session/resume`, or `session/load` after initialization.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error unless initialization succeeded.
    pub fn begin_open(&mut self) -> Result<(), EngineError> {
        self.require_phase(AcpPhase::Initialized, "open a session")?;
        self.phase = AcpPhase::Opening;
        Ok(())
    }

    /// Adopt the definitive ACP session id returned by the open request.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle error out of order or a protocol error when the id is absent.
    pub fn accept_open(&mut self, result: &Value) -> Result<&str, EngineError> {
        self.require_phase(AcpPhase::Opening, "accept session open")?;
        let Some(session_id) = result
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.is_empty())
        else {
            self.phase = AcpPhase::Failed;
            return Err(EngineError::definitely_rejected(
                EngineErrorKind::Protocol,
                "ACP agent created a session without an id",
                false,
            ));
        };
        self.phase = AcpPhase::Ready;
        Ok(self.session_id.insert(session_id.into()))
    }

    /// Complete a resume/load handshake whose authoritative id was caller-owned.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle error out of order, or a protocol error if the agent
    /// contradicts the requested session id.
    pub fn accept_resume(
        &mut self,
        requested_session_id: &str,
        result: &Value,
    ) -> Result<&str, EngineError> {
        self.require_phase(AcpPhase::Opening, "accept session resume")?;
        if requested_session_id.is_empty() {
            self.phase = AcpPhase::Failed;
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                "ACP resume session id must not be empty",
                false,
            ));
        }
        if let Some(reported) = result.get("sessionId").and_then(Value::as_str) {
            if reported != requested_session_id {
                self.phase = AcpPhase::Failed;
                return Err(EngineError::definitely_rejected(
                    EngineErrorKind::Protocol,
                    "ACP agent resumed a different session id",
                    false,
                ));
            }
        }
        self.phase = AcpPhase::Ready;
        Ok(self.session_id.insert(requested_session_id.into()))
    }

    /// Begin exactly one ACP prompt turn.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error unless the session is ready.
    pub fn begin_prompt(&mut self) -> Result<(), EngineError> {
        self.require_phase(AcpPhase::Ready, "send a prompt")?;
        self.phase = AcpPhase::Prompting;
        Ok(())
    }

    pub fn finish_prompt(&mut self, outcome: &DeliveryOutcome<Value>) {
        if self.phase != AcpPhase::Prompting {
            return;
        }
        self.phase = if matches!(outcome, DeliveryOutcome::OutcomeUnknown(_)) {
            AcpPhase::PromptOutcomeUnknown
        } else {
            AcpPhase::Ready
        };
    }

    /// A notification carrying a matching session id is independent proof the
    /// ambiguous prompt reached this ACP process.
    pub fn observe_session_update(&mut self, session_id: &str) {
        if self.phase == AcpPhase::PromptOutcomeUnknown
            && self.session_id.as_deref() == Some(session_id)
        {
            self.phase = AcpPhase::Prompting;
        }
    }

    /// Record a confirmed `session/cancel` notification locally. The ACP
    /// notification has no response, so writer acceptance is the strongest
    /// acknowledgement available to the client.
    pub fn note_cancelled(&mut self) {
        if matches!(
            self.phase,
            AcpPhase::Prompting | AcpPhase::PromptOutcomeUnknown
        ) {
            self.phase = AcpPhase::Ready;
        }
    }

    pub fn begin_close(&mut self) {
        if matches!(self.phase, AcpPhase::Closed | AcpPhase::Closing) {
            return;
        }
        self.phase = AcpPhase::Closing;
    }

    /// Complete teardown after owned transport closure.
    ///
    /// # Errors
    ///
    /// Returns a definitely-unsent lifecycle error unless close has begun.
    pub fn mark_closed(&mut self) -> Result<(), EngineError> {
        self.require_phase(AcpPhase::Closing, "mark closed")?;
        self.phase = AcpPhase::Closed;
        Ok(())
    }

    /// Prefer the non-replaying ACP resume extension over transcript-loading fallback.
    #[must_use]
    pub fn resume_method(&self) -> Option<AcpResumeMethod> {
        self.agent_capabilities.as_ref().and_then(resume_method_for)
    }

    fn require_phase(&self, expected: AcpPhase, operation: &str) -> Result<(), EngineError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(local_lifecycle_error(operation, self.phase))
        }
    }
}

/// Classify advertised ACP session resume capabilities.
#[must_use]
pub fn resume_method_for(capabilities: &Value) -> Option<AcpResumeMethod> {
    let portable = capabilities
        .get("sessionCapabilities")
        .and_then(|session| session.get("resume"))
        .is_some_and(|resume| !resume.is_null() && resume != &Value::Bool(false));
    if portable {
        Some(AcpResumeMethod::Resume)
    } else if capabilities.get("loadSession") == Some(&Value::Bool(true)) {
        Some(AcpResumeMethod::Load)
    } else {
        None
    }
}

#[must_use]
pub fn selected_protocol_version_is_supported(result: &Value) -> bool {
    result.get("protocolVersion").and_then(Value::as_u64) == Some(u64::from(ACP_PROTOCOL_VERSION))
}

fn local_lifecycle_error(operation: &str, phase: AcpPhase) -> EngineError {
    EngineError::definitely_unsent(
        EngineErrorKind::Lifecycle,
        format!("cannot {operation} while ACP session is {phase:?}"),
        false,
    )
}

#[cfg(test)]
mod tests {
    use interface_engine::{
        EffectCertainty,
        jsonrpc::{PendingRouter, delivery_from_rpc, notification_frame, server_result_frame},
        supervised::Wire,
    };
    use serde_json::json;

    use super::*;
    use crate::transport::AcpTransportEvent;

    fn test_wire(max_pending: usize, max_frame_bytes: usize) -> Wire<AcpTransportEvent> {
        Wire::with_router(
            PendingRouter::new(max_pending, max_frame_bytes),
            max_frame_bytes,
        )
    }

    #[test]
    fn initialize_and_prompt_frames_match_acp_v1_names() {
        assert_eq!(
            serde_json::to_value(InitializeParams::interface("0.2.0")).unwrap(),
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "interface",
                    "title": "Interface",
                    "version": "0.2.0"
                }
            })
        );
        assert_eq!(
            serde_json::to_value(SessionPromptParams {
                session_id: "session-1".into(),
                prompt: vec![AcpContentBlock::Text {
                    text: "hello".into()
                }],
                metadata: Some(PromptMetadata {
                    reasoning_effort: Some("high".into()),
                    ..PromptMetadata::default()
                }),
            })
            .unwrap(),
            json!({
                "sessionId": "session-1",
                "prompt": [{"type": "text", "text": "hello"}],
                "_meta": {"reasoningEffort": "high"}
            })
        );
    }

    #[test]
    fn session_update_decoder_matches_the_derived_decoder() {
        // The serde derive this decoder replaced, kept here as the oracle.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Derived {
            session_id: String,
            update: AcpSessionUpdate,
        }
        let update = json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"},"_meta":{"x":1}});
        for params in [
            json!({"sessionId":"s-1","update":update}),
            json!({"sessionId":"s-1","update":update,"_meta":{"extra":true}}),
            json!({"sessionId":"s-1","update":{"sessionUpdate":"plan"}}),
            json!(["s-1", update]),
            json!(["s-1"]),
            json!(["s-1", update, 3]),
            json!({"update":update}),
            json!({"sessionId":"s-1"}),
            json!({"sessionId":7,"update":update}),
            json!({"sessionId":null,"update":update}),
            json!({"sessionId":"s-1","update":["agent_message_chunk"]}),
            json!({"sessionId":"s-1","update":{"content":{}}}),
            json!({"sessionId":"s-1","update":{"sessionUpdate":5}}),
            json!("s-1"),
            json!(null),
        ] {
            let expected = from_value::<Derived>(params.clone()).ok().map(|derived| {
                AcpInbound::SessionUpdate {
                    session_id: derived.session_id,
                    update: derived.update,
                }
            });
            assert_eq!(session_update(Some(params.clone())), expected, "{params}");
        }
        assert_eq!(session_update(None), None);
    }

    #[test]
    fn fake_update_and_permission_request_are_classified_losslessly() {
        let mut wire = test_wire(8, 4_096);
        let mut inbound = Vec::new();
        wire.push_stdout(
            b"{\"method\":\"session/update\",\"params\":{\"sessionId\":\"s-1\",\"update\":{\"sessionUpdate\":\"plan\",\"entries\":[{\"content\":\"x\"}],\"extension\":7}}}\n",
            &mut inbound,
        );
        let [AcpTransportEvent::Inbound(AcpInbound::SessionUpdate { session_id, update })] =
            inbound.as_slice()
        else {
            panic!("expected one session update");
        };
        assert_eq!(session_id, "s-1");
        assert_eq!(update.kind(), UpdateKind::Plan);
        assert_eq!(update.fields.get("extension"), Some(&json!(7)));

        // The reused buffer is cleared by the wire, not by the caller.
        wire.push_stdout(
            b"{\"id\":\"p-1\",\"method\":\"session/request_permission\",\"params\":{\"sessionId\":\"s-1\",\"toolCall\":{},\"options\":[{\"optionId\":\"allow\",\"kind\":\"allow_once\"}]}}\n",
            &mut inbound,
        );
        assert!(matches!(
            inbound.as_slice(),
            [AcpTransportEvent::Inbound(
                AcpInbound::PermissionRequest { .. }
            )]
        ));
    }

    #[test]
    fn permission_response_never_invents_an_unoffered_verdict() {
        let options = vec![PermissionOption {
            option_id: "reject".into(),
            kind: "reject_once".into(),
            name: None,
        }];
        assert_eq!(
            permission_response(&options, true, false).outcome.outcome,
            "cancelled"
        );
        assert_eq!(
            permission_response(&options, false, false)
                .outcome
                .option_id,
            Some("reject".into())
        );
    }

    #[tokio::test]
    async fn fake_handshake_response_routes_without_a_live_agent() {
        let router = PendingRouter::new(8, 4_096);
        let request = router
            .prepare_request(
                AcpMethod::INITIALIZE,
                Some(serde_json::to_value(InitializeParams::interface("0.2.0")).unwrap()),
            )
            .unwrap();
        assert!(router.mark_write_started(&request.id));
        assert!(router.mark_frame_accepted(&request.id));
        let mut wire = Wire::<AcpTransportEvent>::with_router(router, 4_096);
        let response = server_result_frame(
            &request.id,
            json!({"protocolVersion": 1, "agentCapabilities": {}}),
            4_096,
        )
        .unwrap();
        let mut inbound = Vec::new();
        wire.push_stdout(&response, &mut inbound);
        assert!(matches!(
            inbound.as_slice(),
            [AcpTransportEvent::Inbound(
                AcpInbound::ResponseSettled { .. }
            )]
        ));
        assert!(matches!(
            delivery_from_rpc(request.completion.outcome().await),
            DeliveryOutcome::Confirmed(_)
        ));
    }

    #[test]
    fn unknown_prompt_outcome_quarantines_the_session() {
        let mut state = AcpSessionState::default();
        state.begin_initialize().unwrap();
        state
            .accept_initialize(&json!({
                "protocolVersion": 1,
                "agentCapabilities": {"sessionCapabilities": {"resume": {}}}
            }))
            .unwrap();
        assert_eq!(state.resume_method(), Some(AcpResumeMethod::Resume));
        assert_eq!(
            resume_method_for(&json!({"loadSession": true})),
            Some(AcpResumeMethod::Load)
        );
        state.begin_open().unwrap();
        state.accept_open(&json!({"sessionId": "s-1"})).unwrap();
        state.begin_prompt().unwrap();
        let unknown = DeliveryOutcome::OutcomeUnknown(EngineError::outcome_unknown(
            EngineErrorKind::Transport,
            "fake agent closed",
            true,
        ));
        state.finish_prompt(&unknown);
        assert_eq!(state.phase(), AcpPhase::PromptOutcomeUnknown);
        let retry = state.begin_prompt().unwrap_err();
        assert_eq!(retry.certainty, EffectCertainty::DefinitelyUnsent);
        state.observe_session_update("s-1");
        assert_eq!(state.phase(), AcpPhase::Prompting);
    }

    #[test]
    fn resume_keeps_the_requested_id_and_rejects_a_contradiction() {
        let mut state = AcpSessionState::default();
        state.begin_initialize().unwrap();
        state
            .accept_initialize(&json!({
                "protocolVersion": 1,
                "agentCapabilities": {"loadSession": true}
            }))
            .unwrap();
        assert_eq!(state.resume_method(), Some(AcpResumeMethod::Load));
        state.begin_open().unwrap();
        assert_eq!(state.accept_resume("s-1", &json!({})).unwrap(), "s-1");

        let mut mismatch = AcpSessionState::default();
        mismatch.begin_initialize().unwrap();
        mismatch
            .accept_initialize(&json!({"protocolVersion": 1, "agentCapabilities": {}}))
            .unwrap();
        mismatch.begin_open().unwrap();
        assert!(
            mismatch
                .accept_resume("s-1", &json!({"sessionId": "s-2"}))
                .is_err()
        );
    }

    #[test]
    fn cancel_notification_keeps_mouse_metadata() {
        let frame = notification_frame(
            AcpMethod::SESSION_CANCEL,
            Some(serde_json::to_value(SessionCancelParams::mouse("s-1")).unwrap()),
            1_024,
        )
        .unwrap();
        assert_eq!(frame.last(), Some(&b'\n'));
        assert_eq!(
            serde_json::from_slice::<Value>(&frame[..frame.len() - 1]).unwrap(),
            json!({
                "jsonrpc": "2.0",
                "method": "session/cancel",
                "params": {"sessionId": "s-1", "_meta": {"cancelledBy": "mouse"}}
            })
        );
    }
}
