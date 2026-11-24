//! Frozen voice-assistant DTOs shared with the Electron shell.
//!
//! Unlike the older desktop DTOs these are `snake_case` on the wire: the voice
//! plan fixed the JSON shapes verbatim for both sides, so the field names
//! serialize as written.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum VoiceState {
    Off,
    Starting,
    Listening,
    Thinking,
    Speaking,
    Error,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct VoiceAsset {
    /// `installed`, `installing` or `failed`.
    pub status: String,
    pub progress: f64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct VoiceEvent {
    pub state: VoiceState,
    pub session_key: Option<String>,
    pub cwd: Option<String>,
    pub partial: Option<String>,
    pub level: Option<f64>,
    pub message: Option<String>,
    /// `granted`, `denied` or `undetermined`.
    pub microphone: Option<String>,
    pub asset: Option<VoiceAsset>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct VoiceSetEnabledRequest {
    pub enabled: bool,
    pub cwd: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct VoiceSetEnabledResponse {
    pub state: VoiceState,
    pub session_key: Option<String>,
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct VoiceUiCall {
    pub call_id: String,
    pub tool: String,
    pub input: Value,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct VoiceUiResultRequest {
    pub call_id: String,
    pub ok: bool,
    pub output: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn voice_event_golden_json_round_trips() {
        let golden = json!({
            "state":"listening","session_key":"s1","cwd":"/p","partial":null,
            "level":0.2,"message":null,"microphone":"granted","asset":null
        });
        let event: VoiceEvent = serde_json::from_value(golden.clone()).unwrap();
        assert_eq!(event.state, VoiceState::Listening);
        assert_eq!(event.session_key.as_deref(), Some("s1"));
        assert_eq!(event.level, Some(0.2));
        assert_eq!(serde_json::to_value(&event).unwrap(), golden);
        assert_eq!(
            [
                "off",
                "starting",
                "listening",
                "thinking",
                "speaking",
                "error"
            ]
            .map(|state| serde_json::from_value::<VoiceState>(json!(state)).unwrap()),
            [
                VoiceState::Off,
                VoiceState::Starting,
                VoiceState::Listening,
                VoiceState::Thinking,
                VoiceState::Speaking,
                VoiceState::Error
            ]
        );
    }

    #[test]
    fn request_dtos_use_snake_case_and_refuse_unknown_fields() {
        let request: VoiceSetEnabledRequest =
            serde_json::from_value(json!({"enabled":true,"cwd":"/p"})).unwrap();
        assert_eq!(request.cwd, "/p");
        let result: VoiceUiResultRequest =
            serde_json::from_value(json!({"call_id":"c1","ok":true,"output":"[v1] button"}))
                .unwrap();
        assert_eq!(result.call_id, "c1");
        assert!(
            serde_json::from_value::<VoiceUiResultRequest>(
                json!({"callId":"c1","ok":true,"output":""})
            )
            .is_err()
        );
        assert_eq!(
            serde_json::to_value(VoiceUiCall {
                call_id: "c1".into(),
                tool: "ui_snapshot".into(),
                input: json!({}),
            })
            .unwrap(),
            json!({"call_id":"c1","tool":"ui_snapshot","input":{}})
        );
        assert_eq!(
            serde_json::to_value(VoiceSetEnabledResponse {
                state: VoiceState::Error,
                session_key: None,
                message: Some("boom".into()),
            })
            .unwrap(),
            json!({"state":"error","session_key":null,"message":"boom"})
        );
    }
}
