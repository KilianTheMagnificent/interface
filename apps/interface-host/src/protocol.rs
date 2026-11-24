#[cfg(test)]
use interface_contracts::ipc::{ALL_CHANNELS, EXTRA_CHANNELS};
use interface_contracts::ipc::{ALL_DESKTOP_CHANNEL_COUNT, is_desktop_channel};
use interface_desktop_services::INTERNAL_DESKTOP_OPERATIONS;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, value::RawValue};
use std::sync::OnceLock;

use crate::updater::INTERNAL_UPDATER_OPERATIONS;

pub const PROTOCOL_VERSION: u8 = 72;
pub const MAX_REQUEST_ID_BYTES: usize = 256;
pub const MAX_OPERATION_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeRequest {
    pub version: u8,
    pub id: String,
    pub operation: String,
    #[serde(default)]
    pub payload: Value,
}

impl BridgeRequest {
    /// Validates the protocol envelope before dispatch.
    ///
    /// # Errors
    ///
    /// Returns a stable bridge error for version, identifier, or operation
    /// violations. Payload-specific validation belongs to the operation.
    pub fn validate(&self) -> Result<(), BridgeError> {
        if self.version != PROTOCOL_VERSION {
            return Err(BridgeError::new(
                "protocol-version",
                format!(
                    "Unsupported host protocol version {}; expected {PROTOCOL_VERSION}.",
                    self.version
                ),
                false,
                true,
            ));
        }
        validate_token(&self.id, MAX_REQUEST_ID_BYTES, "request-id")?;
        validate_token(&self.operation, MAX_OPERATION_BYTES, "operation")?;
        if !is_desktop_channel(&self.operation)
            && !INTERNAL_DESKTOP_OPERATIONS.contains(&self.operation.as_str())
            && !INTERNAL_UPDATER_OPERATIONS.contains(&self.operation.as_str())
        {
            return Err(BridgeError::new(
                "unknown-operation",
                "The requested Interface operation is not part of this protocol version.",
                false,
                true,
            ));
        }
        if is_push_operation(&self.operation) {
            return Err(BridgeError::new(
                "push-only-operation",
                "That channel is emitted by the host and cannot be invoked.",
                false,
                true,
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub replay_safe: bool,
}

impl BridgeError {
    #[must_use]
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
        replay_safe: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            replay_safe,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeResponse {
    pub version: u8,
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<BridgeError>,
}

impl BridgeResponse {
    #[must_use]
    pub fn success(id: impl Into<String>, result: Value) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.into(),
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn failure(id: impl Into<String>, error: BridgeError) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id: id.into(),
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushFrame {
    pub version: u8,
    pub operation: String,
    pub payload: PushPayload,
}

/// A push frame's payload: a JSON value, or JSON the host already encoded
/// straight from typed rows (the whole-registry push), which the writer
/// copies out verbatim instead of walking a value tree. Reading an encoded
/// payload as a [`Value`] parses it once, on first use.
#[derive(Clone, Debug)]
pub enum PushPayload {
    Value(Value),
    Encoded {
        json: Box<RawValue>,
        parsed: OnceLock<Value>,
    },
}

impl PushPayload {
    /// Decodes the payload as `T`, straight from encoded JSON when it is.
    ///
    /// # Errors
    ///
    /// Returns the decoding error when the payload is not a `T`.
    pub fn decode<T: serde::de::DeserializeOwned>(self) -> Result<T, serde_json::Error> {
        match self {
            Self::Value(value) => serde_json::from_value(value),
            Self::Encoded { json, .. } => serde_json::from_str(json.get()),
        }
    }

    /// The payload as a JSON value.
    #[must_use]
    pub fn into_value(self) -> Value {
        match self {
            Self::Value(value) => value,
            Self::Encoded { json, parsed } => {
                parsed.into_inner().unwrap_or_else(|| parse_encoded(&json))
            }
        }
    }
}

fn parse_encoded(json: &RawValue) -> Value {
    // The host serialized this JSON itself, so it always parses.
    serde_json::from_str(json.get()).unwrap_or(Value::Null)
}

impl From<Value> for PushPayload {
    fn from(value: Value) -> Self {
        Self::Value(value)
    }
}

impl std::ops::Deref for PushPayload {
    type Target = Value;

    fn deref(&self) -> &Value {
        match self {
            Self::Value(value) => value,
            Self::Encoded { json, parsed } => parsed.get_or_init(|| parse_encoded(json)),
        }
    }
}

impl PartialEq for PushPayload {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl PartialEq<Value> for PushPayload {
    fn eq(&self, other: &Value) -> bool {
        **self == *other
    }
}

impl Serialize for PushPayload {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(value) => value.serialize(serializer),
            Self::Encoded { json, .. } => json.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for PushPayload {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Value::deserialize(deserializer).map(Self::Value)
    }
}

impl PushFrame {
    /// Creates a validated host-to-renderer push frame.
    ///
    /// # Errors
    ///
    /// Returns an error when `operation` is not one of the versioned push
    /// channels in the Electron compatibility contract.
    pub fn new(operation: impl Into<String>, payload: Value) -> Result<Self, BridgeError> {
        let operation = operation.into();
        if !is_push_operation(&operation) {
            return Err(BridgeError::new(
                "not-push-operation",
                "The requested channel is not a host push channel.",
                false,
                true,
            ));
        }
        Ok(Self {
            version: PROTOCOL_VERSION,
            operation,
            payload: PushPayload::Value(payload),
        })
    }

    /// A push whose payload is serialized once, straight from `payload`,
    /// rather than built as a [`Value`] tree and walked again by the writer.
    ///
    /// # Errors
    ///
    /// Returns an error when `operation` is not a push channel or `payload`
    /// cannot be encoded.
    pub fn encoded<T: Serialize + ?Sized>(
        operation: impl Into<String>,
        payload: &T,
    ) -> Result<Self, BridgeError> {
        let json = serde_json::value::to_raw_value(payload).map_err(|error| {
            tracing::error!(%error, "failed to encode a host push");
            BridgeError::new(
                "push-encode-failed",
                "The Rust host could not encode a desktop push.",
                false,
                false,
            )
        })?;
        let mut frame = Self::new(operation, Value::Null)?;
        frame.payload = PushPayload::Encoded {
            json,
            parsed: OnceLock::new(),
        };
        Ok(frame)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostReady {
    pub version: u8,
    pub host_version: String,
    pub process_id: u32,
    pub operations: usize,
    pub pushes: usize,
}

impl HostReady {
    /// The ready frame for a host reporting `host_version` (the binary's
    /// release-stamped version, carried in [`crate::HostServerConfig`]).
    #[must_use]
    pub fn current(host_version: &str) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            host_version: host_version.to_owned(),
            process_id: std::process::id(),
            operations: ALL_DESKTOP_CHANNEL_COUNT - PUSH_OPERATIONS.len(),
            pushes: PUSH_OPERATIONS.len(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum HostFrame {
    Ready(HostReady),
    Response(BridgeResponse),
    Push(PushFrame),
}

const PUSH_OPERATIONS: [&str; 12] = [
    "iface:event",
    "iface:registry",
    "iface:updateState",
    "iface:collaborationEvent",
    "iface:orchestrationEvent",
    "iface:importProgress",
    "iface:worktree",
    "iface:terminalLifecycle",
    "iface:voice",
    "iface:voiceUiCall",
    "iface:appearanceChanged",
    "iface:threads",
];

fn is_push_operation(operation: &str) -> bool {
    PUSH_OPERATIONS.contains(&operation)
}

fn validate_token(value: &str, max_bytes: usize, field: &str) -> Result<(), BridgeError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(BridgeError::new(
            format!("invalid-{field}"),
            format!("The {field} is empty, over limit, or contains whitespace/control bytes."),
            false,
            true,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_validation_uses_the_version_5_channel_contract() {
        let request = BridgeRequest {
            version: PROTOCOL_VERSION,
            id: "request-1".to_owned(),
            operation: "iface:getAppearance".to_owned(),
            payload: Value::Null,
        };
        assert_eq!(ALL_CHANNELS.len(), 135);
        assert_eq!(EXTRA_CHANNELS.len(), 4);
        assert_eq!(ALL_DESKTOP_CHANNEL_COUNT, 139);
        assert!(request.validate().is_ok());
        assert_eq!(HostReady::current("test").operations, 127);
        assert_eq!(HostReady::current("test").pushes, 12);
    }

    #[test]
    fn unknown_and_push_only_operations_fail_closed() {
        let mut request = BridgeRequest {
            version: PROTOCOL_VERSION,
            id: "request-1".to_owned(),
            operation: "iface:notReal".to_owned(),
            payload: Value::Null,
        };
        assert_eq!(request.validate().unwrap_err().code, "unknown-operation");
        request.operation = "iface:event".to_owned();
        assert_eq!(request.validate().unwrap_err().code, "push-only-operation");
        request.operation = "iface:voiceUiCall".to_owned();
        assert_eq!(request.validate().unwrap_err().code, "push-only-operation");
        request.operation = "iface:appearanceChanged".to_owned();
        assert_eq!(request.validate().unwrap_err().code, "push-only-operation");
        request.operation = "iface:threads".to_owned();
        assert_eq!(request.validate().unwrap_err().code, "push-only-operation");
        request.operation = "iface:threadControl".to_owned();
        assert!(request.validate().is_ok());
        request.operation = "iface:voiceSetEnabled".to_owned();
        assert!(request.validate().is_ok());
    }

    #[test]
    fn private_native_operations_are_parent_only_and_do_not_change_ready_counts() {
        for operation in INTERNAL_DESKTOP_OPERATIONS
            .into_iter()
            .chain(INTERNAL_UPDATER_OPERATIONS)
        {
            let request = BridgeRequest {
                version: PROTOCOL_VERSION,
                id: "native-updater".to_owned(),
                operation: operation.to_owned(),
                payload: Value::Null,
            };
            assert!(request.validate().is_ok(), "{operation}");
            assert!(!is_desktop_channel(operation), "{operation}");
        }
        assert_eq!(HostReady::current("test").operations, 127);
    }

    #[test]
    fn frame_wire_shape_is_stable_and_unambiguous() {
        let frame = HostFrame::Response(BridgeResponse::success(
            "request-1",
            serde_json::json!({"preference":"system"}),
        ));
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            serde_json::json!({
                "kind":"response",
                "version":72,
                "id":"request-1",
                "ok":true,
                "result":{"preference":"system"}
            })
        );
    }

    #[test]
    fn failed_response_exposes_replay_certainty() {
        let frame = HostFrame::Response(BridgeResponse::failure(
            "request-1",
            BridgeError::new(
                "effect-outcome-unknown",
                "Delivery could not be confirmed.",
                false,
                false,
            ),
        ));
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            serde_json::json!({
                "kind":"response",
                "version":72,
                "id":"request-1",
                "ok":false,
                "error":{
                    "code":"effect-outcome-unknown",
                    "message":"Delivery could not be confirmed.",
                    "retryable":false,
                    "replaySafe":false
                }
            })
        );
    }

    #[test]
    fn push_payloads_preserve_legacy_callback_arity() {
        let event = PushFrame::new(
            interface_contracts::ipc::CHANNELS.event,
            serde_json::json!({"key":"codex#one","event":{"type":"status"}}),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(HostFrame::Push(event)).unwrap(),
            serde_json::json!({
                "kind":"push",
                "version":72,
                "operation":"iface:event",
                "payload":{"key":"codex#one","event":{"type":"status"}}
            })
        );

        let registry = PushFrame::new(
            interface_contracts::ipc::CHANNELS.registry,
            serde_json::json!([{"sessionId":"one"}]),
        )
        .unwrap();
        assert_eq!(registry.payload, serde_json::json!([{"sessionId":"one"}]));
    }
}
