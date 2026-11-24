use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const AGENT_MAX_LINE_BYTES: usize = 1024 * 1024;
pub const AGENT_MAX_PROMPT_BYTES: usize = 64 * 1024;
/// The encoded bytes an inbox page or a transcript tail may fill. The rest of
/// the 1 MiB line holds the envelope and the snapshot beside them, so a page
/// or tail the caller asked to be long is cut short instead of the response
/// becoming unsendable.
pub const AGENT_RESULT_BUDGET_BYTES: usize = 896 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTaskMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_hash: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AgentCapabilities {
    pub reference: bool,
    pub transcript: bool,
    pub message: bool,
    pub steer: bool,
    pub stop: bool,
    pub inbox: bool,
    pub monitor: bool,
}

impl AgentCapabilities {
    /// The facets both sides of the socket assume when a snapshot carries no
    /// explicit capability object. The two sides must agree exactly, so they
    /// share this one derivation.
    #[must_use]
    pub const fn defaults(live: bool, status: AgentStatus) -> Self {
        let usable = live && !matches!(status, AgentStatus::Failed);
        Self {
            reference: true,
            transcript: usable,
            message: usable,
            steer: false,
            stop: live,
            inbox: true,
            monitor: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Failed,
    NotLive,
}

impl AgentStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::NotLive => "not-live",
        }
    }

    /// A status `agent wait` can wait for: idle, working or blocked.
    #[must_use]
    pub fn parse_waitable(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            _ => None,
        }
    }
}

/// Accepts the exact durable agent names both the client and the server admit.
#[must_use]
pub fn is_agent_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSnapshot {
    pub name: Option<String>,
    pub key: String,
    pub engine: String,
    pub session_id: String,
    pub status: AgentStatus,
    pub cwd: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<AgentTaskMetadata>,
    pub capabilities: AgentCapabilities,
    pub live: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorCode {
    InvalidRequest,
    Ambiguous,
    NotRunning,
    NotFound,
    NotLive,
    Blocked,
    Failed,
    NameTaken,
    Unsupported,
    Timeout,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProtocolError {
    pub code: AgentErrorCode,
    pub message: String,
    pub data: Option<Value>,
}

impl AgentProtocolError {
    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(AgentErrorCode::InvalidRequest, message)
    }

    #[must_use]
    pub fn new(code: AgentErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

impl fmt::Display for AgentProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AgentProtocolError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AgentMethod {
    #[serde(rename = "agent.commands")]
    Commands,
    #[serde(rename = "agent.list")]
    List,
    #[serde(rename = "agent.get")]
    Get,
    #[serde(rename = "agent.read")]
    Read,
    #[serde(rename = "agent.send")]
    Send,
    #[serde(rename = "agent.steer")]
    Steer,
    #[serde(rename = "agent.stop")]
    Stop,
    #[serde(rename = "agent.inbox")]
    Inbox,
    #[serde(rename = "agent.monitor")]
    Monitor,
    #[serde(rename = "agent.wait")]
    Wait,
    #[serde(rename = "agent.resume")]
    Resume,
    #[serde(rename = "agent.start")]
    Start,
    #[serde(rename = "agent.rename")]
    Rename,
    #[serde(rename = "agent.focus")]
    Focus,
}

impl AgentMethod {
    const ALL: [Self; 14] = [
        Self::Commands,
        Self::List,
        Self::Get,
        Self::Read,
        Self::Send,
        Self::Steer,
        Self::Stop,
        Self::Inbox,
        Self::Monitor,
        Self::Wait,
        Self::Resume,
        Self::Start,
        Self::Rename,
        Self::Focus,
    ];

    /// The frozen wire name; a test pins it to the serde rename.
    pub(crate) const fn wire_name(self) -> &'static str {
        match self {
            Self::Commands => "agent.commands",
            Self::List => "agent.list",
            Self::Get => "agent.get",
            Self::Read => "agent.read",
            Self::Send => "agent.send",
            Self::Steer => "agent.steer",
            Self::Stop => "agent.stop",
            Self::Inbox => "agent.inbox",
            Self::Monitor => "agent.monitor",
            Self::Wait => "agent.wait",
            Self::Resume => "agent.resume",
            Self::Start => "agent.start",
            Self::Rename => "agent.rename",
            Self::Focus => "agent.focus",
        }
    }

    /// The `iface agent <verb>` spelling: the wire name without `agent.`.
    pub(crate) fn verb(self) -> &'static str {
        self.wire_name().trim_start_matches("agent.")
    }

    pub(crate) fn from_verb(verb: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.verb() == verb)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentRequest {
    pub id: String,
    pub method: AgentMethod,
    pub params: Map<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFailure {
    pub code: AgentErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentResponsePayload {
    Result(Value),
    Error(AgentFailure),
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentResponse {
    pub id: String,
    pub payload: AgentResponsePayload,
}

/// Encodes one newline-delimited agent request.
///
/// # Errors
///
/// Returns an invalid-request error when the request cannot be serialized or
/// exceeds the protocol's one-mebibyte line limit.
pub fn encode_agent_request(request: &AgentRequest) -> Result<Vec<u8>, AgentProtocolError> {
    encode_json_line(request)
}

/// Encodes one newline-delimited agent response.
///
/// # Errors
///
/// Returns an invalid-request error when the response cannot be serialized or
/// exceeds the protocol's one-mebibyte line limit.
pub fn encode_agent_response(response: AgentResponse) -> Result<Vec<u8>, AgentProtocolError> {
    let mut object = Map::new();
    object.insert("id".into(), Value::String(response.id));
    match response.payload {
        AgentResponsePayload::Result(result) => {
            object.insert("result".into(), result);
        }
        AgentResponsePayload::Error(error) => {
            object.insert(
                "error".into(),
                serde_json::to_value(error).map_err(|_| {
                    AgentProtocolError::invalid("Agent response could not be encoded.")
                })?,
            );
        }
    }
    encode_json_line(&Value::Object(object))
}

/// Decodes and validates one newline-delimited agent request.
///
/// # Errors
///
/// Returns an invalid-request error for malformed UTF-8/JSON, invalid fields,
/// extra frames, missing framing, or an oversized line.
pub fn decode_agent_request(input: &[u8]) -> Result<AgentRequest, AgentProtocolError> {
    let value = decode_json_line(input)?;
    let object = value
        .as_object()
        .ok_or_else(|| AgentProtocolError::invalid("Agent request must be an object."))?;
    let id = non_empty_string(object.get("id"))
        .ok_or_else(|| AgentProtocolError::invalid("Agent request requires a non-empty id."))?;
    let method =
        serde_json::from_value::<AgentMethod>(object.get("method").cloned().unwrap_or(Value::Null))
            .map_err(|_| AgentProtocolError::invalid("Agent request method is not supported."))?;
    let params = object
        .get("params")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| AgentProtocolError::invalid("Agent request params must be an object."))?;
    Ok(AgentRequest { id, method, params })
}

/// Decodes and validates one newline-delimited agent response.
///
/// # Errors
///
/// Returns an invalid-request error for malformed UTF-8/JSON, invalid fields,
/// extra frames, missing framing, or an oversized line.
pub fn decode_agent_response(input: &[u8]) -> Result<AgentResponse, AgentProtocolError> {
    let value = decode_json_line(input)?;
    let object = value
        .as_object()
        .ok_or_else(|| AgentProtocolError::invalid("Agent response must be an object."))?;
    let id = non_empty_string(object.get("id"))
        .ok_or_else(|| AgentProtocolError::invalid("Agent response requires a non-empty id."))?;
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");
    if has_result == has_error {
        return Err(AgentProtocolError::invalid(
            "Agent response must contain exactly one result or error.",
        ));
    }
    let payload = if has_result {
        AgentResponsePayload::Result(object.get("result").cloned().unwrap_or(Value::Null))
    } else {
        let error = serde_json::from_value::<AgentFailure>(
            object.get("error").cloned().unwrap_or(Value::Null),
        )
        .map_err(|_| {
            AgentProtocolError::invalid("Agent response error has an invalid code or message.")
        })?;
        AgentResponsePayload::Error(error)
    };
    Ok(AgentResponse { id, payload })
}

fn decode_json_line(input: &[u8]) -> Result<Value, AgentProtocolError> {
    if input.last() != Some(&b'\n') {
        return Err(AgentProtocolError::invalid(
            "Agent protocol message must end with a newline.",
        ));
    }
    let line = &input[..input.len().saturating_sub(1)];
    if line.len() > AGENT_MAX_LINE_BYTES {
        return Err(AgentProtocolError::invalid(
            "Agent protocol line exceeds the 1 MiB limit.",
        ));
    }
    if line.contains(&b'\n') {
        return Err(AgentProtocolError::invalid(
            "Agent protocol accepts one request per connection.",
        ));
    }
    let source = std::str::from_utf8(line)
        .map_err(|_| AgentProtocolError::invalid("Agent protocol line must be valid UTF-8."))?;
    serde_json::from_str(source)
        .map_err(|_| AgentProtocolError::invalid("Agent protocol line must contain valid JSON."))
}

/// The encoded JSON length of `value`, counted without buffering it.
#[must_use]
pub fn encoded_len(value: &impl Serialize) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_or(usize::MAX, |()| counter.0)
}

fn encode_json_line<T: Serialize>(value: &T) -> Result<Vec<u8>, AgentProtocolError> {
    let mut line = serde_json::to_vec(value)
        .map_err(|_| AgentProtocolError::invalid("Agent protocol value could not be encoded."))?;
    if line.len() > AGENT_MAX_LINE_BYTES {
        return Err(AgentProtocolError::invalid(
            "Agent protocol line exceeds the 1 MiB limit.",
        ));
    }
    line.push(b'\n');
    Ok(line)
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn method_wire_names_match_the_serde_renames_and_verbs_round_trip() {
        for method in AgentMethod::ALL {
            assert_eq!(
                serde_json::to_value(method).expect("method"),
                json!(method.wire_name())
            );
            assert_eq!(AgentMethod::from_verb(method.verb()), Some(method));
        }
        assert_eq!(AgentMethod::from_verb("agent.send"), None);
        assert_eq!(AgentMethod::from_verb("SEND"), None);
    }

    #[test]
    fn protocol_golden_lines_match_the_typescript_wire_shape() {
        let request = AgentRequest {
            id: "request-1".into(),
            method: AgentMethod::Send,
            params: Map::from_iter([
                ("target".into(), json!("reviewer")),
                ("text".into(), json!("check this")),
            ]),
        };
        let encoded = encode_agent_request(&request).expect("encode");
        assert_eq!(decode_agent_request(&encoded).expect("decode"), request);
        assert_eq!(
            String::from_utf8(encoded).expect("utf8"),
            "{\"id\":\"request-1\",\"method\":\"agent.send\",\"params\":{\"target\":\"reviewer\",\"text\":\"check this\"}}\n"
        );
    }

    #[test]
    fn rejects_missing_newline_multiple_requests_and_oversized_lines() {
        assert_eq!(
            decode_agent_request(br#"{"id":"x","method":"agent.list","params":{}}"#)
                .expect_err("newline")
                .message,
            "Agent protocol message must end with a newline."
        );
        assert!(
            decode_agent_request(b"{\"id\":\"x\",\"method\":\"agent.list\",\"params\":{}}\n{}\n")
                .is_err()
        );
        let mut large = vec![b'x'; AGENT_MAX_LINE_BYTES + 1];
        large.push(b'\n');
        assert!(decode_agent_request(&large).is_err());
    }

    #[test]
    fn encoded_len_counts_the_escaped_json() {
        for value in [
            json!("plain"),
            json!("quote \" and \u{1}"),
            json!({"a":[1,2]}),
        ] {
            assert_eq!(
                encoded_len(&value),
                serde_json::to_vec(&value).expect("encode").len()
            );
        }
    }

    #[test]
    fn response_requires_exactly_one_result_or_error() {
        assert!(decode_agent_response(b"{\"id\":\"x\"}\n").is_err());
        assert!(decode_agent_response(
            b"{\"id\":\"x\",\"result\":null,\"error\":{\"code\":\"failed\",\"message\":\"x\"}}\n"
        )
        .is_err());
        let response = AgentResponse {
            id: "x".into(),
            payload: AgentResponsePayload::Error(AgentFailure {
                code: AgentErrorCode::NotRunning,
                message: "The iface TUI is not running.".into(),
                data: None,
            }),
        };
        assert_eq!(
            decode_agent_response(&encode_agent_response(response.clone()).expect("encode"))
                .expect("decode"),
            response
        );
    }
}
