//! Versioned rendered-screen observations for native terminal settings.
use crate::terminal::{TerminalGeneration, TerminalId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalControlScreenV1 {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_column: usize,
    pub at_bottom: bool,
    pub bracketed_paste: bool,
    pub ghost_text: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum TerminalControlSelectionV1 {
    Model { value: String },
    Effort { value: String },
    Cancel,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalControlsRequestV1 {
    pub version: u8,
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    pub screen: TerminalControlScreenV1,
    pub selection: Option<TerminalControlSelectionV1>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalControlsStateV1 {
    pub version: u8,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub ready: bool,
    pub pending: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trips_the_shared_v13_contract_and_rejects_unknown_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-controls-v13.json"
        ))
        .unwrap();
        let request: TerminalControlsRequestV1 =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
        let state: TerminalControlsStateV1 =
            serde_json::from_value(fixture["state"].clone()).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), fixture["state"]);
        let mut invalid = fixture["request"].clone();
        invalid["screen"]["command"] = serde_json::json!("/quit");
        assert!(serde_json::from_value::<TerminalControlsRequestV1>(invalid).is_err());
    }
}
