use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const DEFAULT_MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const RETAINED_DECODE_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UserMessage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub session_id: String,
    pub message: UserMessageBody,
    pub parent_tool_use_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UserMessageBody {
    pub role: &'static str,
    pub content: Vec<TextBlock>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TextBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

impl UserMessage {
    #[must_use]
    pub fn text(session_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            kind: "user",
            session_id: session_id.into(),
            message: UserMessageBody {
                role: "user",
                content: vec![TextBlock {
                    kind: "text",
                    text: text.into(),
                }],
            },
            parent_tool_use_id: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HostRequest {
    pub request_id: String,
    pub request: Value,
}

impl HostRequest {
    #[must_use]
    pub fn subtype(&self) -> Option<&str> {
        self.request.get("subtype").and_then(Value::as_str)
    }

    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        self.request.get("tool_name").and_then(Value::as_str)
    }

    #[must_use]
    pub fn tool_use_id(&self) -> Option<&str> {
        self.request.get("tool_use_id").and_then(Value::as_str)
    }

    #[must_use]
    pub fn input(&self) -> Option<&Value> {
        self.request.get("input")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ControlResponse {
    pub subtype: String,
    pub request_id: String,
    #[serde(default)]
    pub response: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ControlResponse {
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.subtype == "success"
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderFrame {
    ControlRequest(HostRequest),
    ControlResponse(ControlResponse),
    ControlCancelled { request_id: String },
    KeepAlive,
    Event(Value),
}

impl ProviderFrame {
    /// Classifies a decoded provider JSON value without rejecting future event
    /// kinds.
    ///
    /// # Errors
    ///
    /// Returns an error when a control frame omits required protocol fields or
    /// contains a malformed control response.
    pub fn decode(value: Value) -> Result<Self, ProtocolError> {
        // The frame is owned here, so control payloads are moved out of it
        // rather than deep-cloned.
        let mut object = match value {
            Value::Object(object) => object,
            other => return Ok(Self::Event(other)),
        };
        match object.get("type").and_then(Value::as_str) {
            Some("control_request") => {
                let request_id = required_string(&object, "request_id")?;
                let request = object
                    .remove("request")
                    .filter(Value::is_object)
                    .ok_or(ProtocolError::MissingField("request"))?;
                Ok(Self::ControlRequest(HostRequest {
                    request_id,
                    request,
                }))
            }
            Some("control_response") => {
                let response = object
                    .remove("response")
                    .ok_or(ProtocolError::MissingField("response"))?;
                Ok(Self::ControlResponse(serde_json::from_value(response)?))
            }
            Some("control_cancel_request") => Ok(Self::ControlCancelled {
                request_id: required_string(&object, "request_id")?,
            }),
            Some("keep_alive") => Ok(Self::KeepAlive),
            _ => Ok(Self::Event(Value::Object(object))),
        }
    }

    #[must_use]
    pub fn control_request(request_id: &str, request: &Value) -> Value {
        json!({
            "type": "control_request",
            "request_id": request_id,
            "request": request,
        })
    }

    #[must_use]
    pub fn control_success(request_id: &str, response: &Value) -> Value {
        json!({
            "type": "control_response",
            "response": {
                "subtype": "success",
                "request_id": request_id,
                "response": response,
            },
        })
    }
}

fn required_string(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<String, ProtocolError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or(ProtocolError::MissingField(field))
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("provider frame exceeded the {limit}-byte limit")]
    FrameTooLarge { limit: usize },
    #[error("provider frame was not valid UTF-8")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("provider frame was not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("provider frame did not contain required field `{0}`")]
    MissingField(&'static str),
}

#[derive(Debug)]
pub struct NdjsonDecoder {
    buffered: Vec<u8>,
    max_frame_bytes: usize,
}

impl Default for NdjsonDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_FRAME_BYTES)
    }
}

impl NdjsonDecoder {
    #[must_use]
    pub const fn new(max_frame_bytes: usize) -> Self {
        Self {
            buffered: Vec::new(),
            max_frame_bytes,
        }
    }

    /// Adds bytes and returns every complete JSON line found in the chunk.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, non-UTF-8, or invalid JSON frames.
    #[cfg(test)]
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, ProtocolError> {
        let (values, error) = self.push_frames(chunk);
        error.map_or(Ok(values), Err)
    }

    /// Like [`Self::push`], but keeps the frames decoded before a bad line, so
    /// a reply Claude already sent is still routed before the stream ends.
    pub fn push_frames(&mut self, chunk: &[u8]) -> (Vec<Value>, Option<ProtocolError>) {
        let mut values = Vec::new();
        let error = self.push_into(chunk, &mut values).err();
        (values, error)
    }

    fn push_into(
        &mut self,
        mut chunk: &[u8],
        values: &mut Vec<Value>,
    ) -> Result<(), ProtocolError> {
        while let Some(newline) = chunk.iter().position(|byte| *byte == b'\n') {
            let segment = &chunk[..newline];
            if self.buffered.is_empty() {
                // The whole line already arrived in this chunk, so parse it in
                // place instead of copying it through the buffer. The per-frame
                // byte bound still applies, and is checked first.
                if segment.len() > self.max_frame_bytes {
                    return Err(ProtocolError::FrameTooLarge {
                        limit: self.max_frame_bytes,
                    });
                }
                let line = segment.strip_suffix(b"\r").unwrap_or(segment);
                if !line.iter().all(u8::is_ascii_whitespace) {
                    values.push(parse_line(line)?);
                }
            } else {
                self.extend_checked(segment)?;
                if self.buffered.last() == Some(&b'\r') {
                    self.buffered.pop();
                }
                if !self.buffered.iter().all(u8::is_ascii_whitespace) {
                    values.push(parse_line(&self.buffered)?);
                }
                self.buffered.clear();
                if self.buffered.capacity() > RETAINED_DECODE_BUFFER_BYTES {
                    self.buffered.shrink_to(RETAINED_DECODE_BUFFER_BYTES);
                }
            }
            chunk = &chunk[newline + 1..];
        }
        self.extend_checked(chunk)
    }

    /// Decodes a final unterminated frame, if the input ended with one.
    ///
    /// # Errors
    ///
    /// Returns an error when the trailing frame is not valid UTF-8 or JSON.
    pub fn finish(mut self) -> Result<Option<Value>, ProtocolError> {
        if self.buffered.last() == Some(&b'\r') {
            self.buffered.pop();
        }
        if self.buffered.iter().all(u8::is_ascii_whitespace) {
            Ok(None)
        } else {
            parse_line(&self.buffered).map(Some)
        }
    }

    fn extend_checked(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if self.buffered.len().saturating_add(bytes.len()) > self.max_frame_bytes {
            return Err(ProtocolError::FrameTooLarge {
                limit: self.max_frame_bytes,
            });
        }
        self.buffered.extend_from_slice(bytes);
        Ok(())
    }
}

fn parse_line(bytes: &[u8]) -> Result<Value, ProtocolError> {
    let text = std::str::from_utf8(bytes)?;
    serde_json::from_str(text).map_err(ProtocolError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_accepts_fragmented_and_batched_frames() {
        let mut decoder = NdjsonDecoder::new(128);
        assert!(
            decoder
                .push(br#"{"type":"keep_"#)
                .expect("fragment")
                .is_empty()
        );
        let values = decoder
            .push(b"alive\"}\n{\"type\":\"result\"}\r\n\n")
            .expect("frames");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["type"], "keep_alive");
        assert_eq!(values[1]["type"], "result");
        assert_eq!(decoder.finish().expect("finish"), None);
    }

    #[test]
    fn frames_before_a_bad_line_are_kept() {
        let mut decoder = NdjsonDecoder::new(128);
        let (values, error) = decoder
            .push_frames(b"{\"type\":\"control_response\"}\nnot json\n{\"type\":\"result\"}\n");
        assert_eq!(values, vec![json!({"type": "control_response"})]);
        assert!(error.is_some());
    }

    #[test]
    fn decoder_rejects_an_unbounded_line_before_json_parsing() {
        let mut decoder = NdjsonDecoder::new(4);
        let error = decoder.push(b"12345").expect_err("oversized");
        assert!(matches!(error, ProtocolError::FrameTooLarge { limit: 4 }));
    }

    #[test]
    fn a_large_fragmented_frame_does_not_pin_its_allocation_for_the_session() {
        let value = json!({"text": "x".repeat(512 * 1024)});
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        let mut decoder = NdjsonDecoder::default();
        assert!(decoder.push(&bytes[..bytes.len() - 1]).unwrap().is_empty());
        assert!(decoder.buffered.capacity() >= 512 * 1024);
        assert_eq!(decoder.push(b"\n").unwrap(), vec![value]);
        assert!(decoder.buffered.capacity() <= RETAINED_DECODE_BUFFER_BYTES);
        assert!(decoder.push(b"{\"ok\":").unwrap().is_empty());
        assert_eq!(
            decoder.push(b"true}\r\n").unwrap(),
            vec![json!({"ok": true})]
        );
    }

    #[test]
    fn control_frames_are_separated_from_provider_events() {
        let frame = ProviderFrame::decode(json!({
            "type": "control_request",
            "request_id": "request-1",
            "request": { "subtype": "can_use_tool", "tool_name": "Bash", "input": {} },
        }))
        .expect("control request");
        let ProviderFrame::ControlRequest(request) = frame else {
            panic!("wrong frame kind");
        };
        assert_eq!(request.subtype(), Some("can_use_tool"));
        assert_eq!(request.tool_name(), Some("Bash"));
    }

    #[test]
    fn whole_line_fast_path_matches_the_buffered_path_and_keeps_the_frame_bound() {
        let mut decoder = NdjsonDecoder::new(128);
        let values = decoder
            .push(b"{\"type\":\"keep_alive\"}\r\n \n{\"type\":\"result\"}\n")
            .expect("frames");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["type"], "keep_alive");
        assert_eq!(values[1]["type"], "result");

        let mut bounded = NdjsonDecoder::new(4);
        let error = bounded.push(b"12345\n").expect_err("oversized whole line");
        assert!(matches!(error, ProtocolError::FrameTooLarge { limit: 4 }));
    }

    #[test]
    fn control_frames_keep_their_missing_field_prose_and_non_objects_stay_events() {
        assert!(matches!(
            ProviderFrame::decode(json!({"type": "control_request", "request_id": "r-1"})),
            Err(ProtocolError::MissingField("request"))
        ));
        assert!(matches!(
            ProviderFrame::decode(json!({"type": "control_request", "request": {}})),
            Err(ProtocolError::MissingField("request_id"))
        ));
        assert!(matches!(
            ProviderFrame::decode(json!({"type": "control_response"})),
            Err(ProtocolError::MissingField("response"))
        ));
        assert!(matches!(
            ProviderFrame::decode(json!("not an object")).expect("event"),
            ProviderFrame::Event(Value::String(_))
        ));
        let ProviderFrame::Event(value) =
            ProviderFrame::decode(json!({"type": "system", "keep": 1})).expect("event")
        else {
            panic!("wrong frame kind");
        };
        assert_eq!(value["keep"], 1);
    }

    #[test]
    fn user_message_matches_the_streaming_cli_shape() {
        let value =
            serde_json::to_value(UserMessage::text("session-1", "hello")).expect("serialize");
        assert_eq!(value["type"], "user");
        assert_eq!(value["parent_tool_use_id"], Value::Null);
        assert_eq!(value["message"]["content"][0]["text"], "hello");
    }
}
