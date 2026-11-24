//! Explicit provider-scoped discovery and user-requested MCP probes.
use crate::ipc::CustomizationInventory;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomizationProvider {
    Claude,
    Codex,
    Grok,
    Qwen,
    Opencode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum CustomizationCommand {
    Read {
        root: String,
        provider: CustomizationProvider,
    },
    Source {
        root: String,
        provider: CustomizationProvider,
        path: String,
    },
    CheckMcp {
        root: String,
        provider: CustomizationProvider,
        path: String,
        name: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCustomization {
    pub provider: CustomizationProvider,
    pub user_root: String,
    pub inventory: CustomizationInventory,
    /// Discovery is not evidence that a running provider loaded these entries.
    pub loaded: CustomizationLoaded,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<McpHealth>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomizationLoaded {
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpHealth {
    pub path: String,
    pub name: String,
    pub state: McpHealthState,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpHealthState {
    Ready,
    Failed,
    Unsupported,
    Disabled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::{BackgroundCommand, BackgroundState};
    #[test]
    fn provider_inventory_and_background_share_the_v49_wire_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/customization-background-v49.json"
        ))
        .unwrap();
        for value in fixture["backgroundRequests"].as_array().unwrap() {
            let request: BackgroundCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
        }
        let background: BackgroundState =
            serde_json::from_value(fixture["backgroundResponse"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(background).unwrap(),
            fixture["backgroundResponse"]
        );
        for value in fixture["customizationRequests"].as_array().unwrap() {
            let request: CustomizationCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
        }
        let response: ProviderCustomization =
            serde_json::from_value(fixture["customizationResponse"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            fixture["customizationResponse"]
        );
        assert!(serde_json::from_value::<CustomizationCommand>(serde_json::json!({"kind":"read","root":"/project","provider":"codex","execute":true})).is_err());
    }
}
