//! A prompt recovery receipt records provider delivery, never infers non-delivery.
use crate::{ValidationError, ipc::PromptContextDescriptor};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptDelivery {
    Unsent,
    Accepted,
    Unknown,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryMode {
    Retry,
    Continue,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverySource {
    pub id: String,
    pub mode: RecoveryMode,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptRecoveryInput {
    pub text: String,
    pub context: Vec<PromptContextDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RecoverySource>,
}
impl PromptRecoveryInput {
    /// # Errors
    /// Refuses malformed or unbounded prompt descriptors. This does not grant file access.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.text.trim().is_empty()
            || self.text.len() > 1024 * 1024
            || self.context.len() > 8
            || self.context.iter().any(|item| {
                item.id.is_empty()
                    || item.id.len() > 512
                    || item.label.len() > 512
                    || item.value.is_empty()
                    || item.value.len() > 4096
                    || item.detail.as_ref().is_some_and(|s| s.len() > 4096)
                    || item.safe_content.as_ref().is_some_and(|s| s.len() > 32_000)
            })
        {
            return Err(ValidationError::new(
                "recovery",
                "invalid or oversized prompt context",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptRecoveryReceipt {
    pub id: String,
    pub key: String,
    pub input: PromptRecoveryInput,
    pub serialized: String,
    pub delivery: PromptDelivery,
    pub at: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRequest {
    pub version: u8,
    pub key: String,
    #[serde(flatten)]
    pub command: RecoveryCommand,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum RecoveryCommand {
    Read,
    ListPending,
    RetainDrafts { paths: Vec<String> },
    AcknowledgeLaunch { id: String },
    Prepare { id: String, mode: RecoveryMode },
    Forget { id: String },
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryResponse {
    pub version: u8,
    pub receipt: Option<PromptRecoveryReceipt>,
    pub prepared: Option<PromptRecoveryInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PromptRecoverySummary>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptRecoverySummary {
    pub key: String,
    pub id: String,
    pub preview: String,
    pub delivery: PromptDelivery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
}

impl<'de> Deserialize<'de> for RecoveryRequest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize, Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            version: u8,
            key: String,
            #[serde(flatten)]
            command: RecoveryCommand,
        }
        let value = serde_json::Value::deserialize(deserializer)?;
        let wire: Wire = serde_json::from_value(value.clone()).map_err(serde::de::Error::custom)?;
        let encoded = serde_json::to_value(&wire).map_err(serde::de::Error::custom)?;
        if value
            .as_object()
            .is_none_or(|object| object.keys().any(|key| encoded.get(key).is_none()))
        {
            return Err(serde::de::Error::custom("unknown request field"));
        }
        Ok(Self {
            version: wire.version,
            key: wire.key,
            command: wire.command,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_v49_fixture_round_trips() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/recovery-v49.json"
        ))
        .unwrap();
        for value in fixture["requests"].as_array().unwrap() {
            let request: RecoveryRequest = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
            let mut bad = value.clone();
            bad["grant"] = serde_json::json!(true);
            assert!(serde_json::from_value::<RecoveryRequest>(bad).is_err());
        }
        for value in fixture["responses"].as_array().unwrap() {
            let response: RecoveryResponse = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), *value);
        }
        let input: PromptRecoveryInput = serde_json::from_value(fixture["input"].clone()).unwrap();
        input.validate().unwrap();
        let task: crate::agents::AgentCommand =
            serde_json::from_value(fixture["agentTask"].clone()).unwrap();
        assert_eq!(serde_json::to_value(task).unwrap(), fixture["agentTask"]);
    }
}
