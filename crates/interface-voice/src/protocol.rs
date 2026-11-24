//! NDJSON protocol (version 1) between the host and the `interface-voice`
//! helper. Field names are camelCase on the wire; unknown fields and unknown
//! helper messages are ignored rather than refused.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HostToHelper {
    #[serde(rename_all = "camelCase")]
    Start {
        end_silence_ms: u32,
        locale: String,
    },
    Stop,
    Speak {
        id: String,
        text: String,
    },
    Cancel,
    SetVoice {
        voice: String,
    },
    Shutdown,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum HelperToHost {
    #[serde(rename_all = "camelCase")]
    Ready {
        protocol: u32,
        voices: Vec<VoiceInfo>,
        /// Omitted on the wire when the helper found no voice to speak with.
        #[serde(default)]
        default_voice: String,
    },
    Permission {
        microphone: String,
    },
    Asset {
        status: String,
        #[serde(default)]
        progress: f64,
    },
    State {
        listening: bool,
        speaking: bool,
    },
    Partial {
        text: String,
    },
    Final {
        text: String,
    },
    SpeechStarted {
        id: String,
    },
    SpeechFinished {
        id: String,
    },
    SpeechCancelled {
        id: String,
    },
    Interrupted,
    Error {
        message: String,
        #[serde(default)]
        fatal: bool,
    },
    Level {
        rms: f64,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    /// Optional on the wire: a backend without quality tiers omits the key.
    #[serde(default)]
    pub quality: String,
    pub language: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn helper_messages_round_trip_and_unknown_types_decode_to_unknown() {
        let started: HelperToHost =
            serde_json::from_str(r#"{"type":"speechStarted","id":"u1"}"#).unwrap();
        assert_eq!(started, HelperToHost::SpeechStarted { id: "u1".into() });
        let ready: HelperToHost = serde_json::from_value(json!({
            "type": "ready",
            "protocol": 1,
            "voices": [{"id": "v", "name": "Ava", "language": "en-US"}],
            "defaultVoice": "v",
            "extra": true
        }))
        .unwrap();
        assert_eq!(
            ready,
            HelperToHost::Ready {
                protocol: 1,
                voices: vec![VoiceInfo {
                    id: "v".into(),
                    name: "Ava".into(),
                    quality: String::new(),
                    language: "en-US".into(),
                }],
                default_voice: "v".into(),
            }
        );
        let error: HelperToHost =
            serde_json::from_str(r#"{"type":"error","message":"boom"}"#).unwrap();
        assert_eq!(
            error,
            HelperToHost::Error {
                message: "boom".into(),
                fatal: false
            }
        );
        let unknown: HelperToHost = serde_json::from_str(r#"{"type":"whatever","x":1}"#).unwrap();
        assert_eq!(unknown, HelperToHost::Unknown);
    }

    #[test]
    fn ready_without_a_default_voice_decodes_to_an_empty_one() {
        let ready: HelperToHost =
            serde_json::from_value(json!({"type": "ready", "protocol": 1, "voices": []})).unwrap();
        assert_eq!(
            ready,
            HelperToHost::Ready {
                protocol: 1,
                voices: Vec::new(),
                default_voice: String::new(),
            }
        );
    }

    #[test]
    fn host_messages_serialize_camel_case_with_a_type_tag() {
        assert_eq!(
            serde_json::to_value(HostToHelper::Start {
                end_silence_ms: 800,
                locale: "en-US".into()
            })
            .unwrap(),
            json!({"type":"start","endSilenceMs":800,"locale":"en-US"})
        );
        assert_eq!(
            serde_json::to_string(&HostToHelper::Speak {
                id: "u1".into(),
                text: "Hi.".into()
            })
            .unwrap(),
            r#"{"type":"speak","id":"u1","text":"Hi."}"#
        );
        assert_eq!(
            serde_json::to_string(&HostToHelper::Shutdown).unwrap(),
            r#"{"type":"shutdown"}"#
        );
    }
}
