//! `iface:diagnostics` (protocol 39): what a user can reach when something
//! goes wrong. A credential-filtered report for Help → Copy Diagnostics, the
//! one-time notices for stores the host set aside because it could not read
//! them, and Finder reveals of the log, the data folder and a set-aside file.
//! Rust writes the report, keeps the notices and reveals; the renderer and
//! the main process only show and ask.

use serde::{Deserialize, Serialize};

use crate::ValidationError;

/// The longest notice id accepted back from the renderer.
pub const MAX_NOTICE_ID_BYTES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DiagnosticsCommand {
    /// The Copy Diagnostics text: version, macOS, architecture, the data
    /// folder, each provider's probe and the last warnings, filtered.
    Report {},
    /// Stores set aside this run that nobody has dismissed yet.
    Notices {},
    /// Forget one notice; answers the rest.
    DismissNotice { id: String },
    /// Show something in Finder.
    Reveal {
        target: RevealTarget,
        /// The notice whose set-aside file to show (`target: notice` only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RevealTarget {
    /// `host.log` in the data folder.
    Logs,
    /// The data folder itself.
    DataFolder,
    /// A notice's set-aside file.
    Notice,
}

impl DiagnosticsCommand {
    /// # Errors
    /// Refuses empty or oversized notice ids and an id on a reveal that does
    /// not take one (or none on one that does).
    pub fn validate(&self) -> Result<(), ValidationError> {
        let valid_id = |id: &str| {
            !id.is_empty()
                && id.len() <= MAX_NOTICE_ID_BYTES
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        };
        match self {
            Self::Report {} | Self::Notices {} => Ok(()),
            Self::DismissNotice { id } => {
                if valid_id(id) {
                    Ok(())
                } else {
                    Err(ValidationError::new("id", "expected a notice id"))
                }
            }
            Self::Reveal { target, id } => match (target, id) {
                (RevealTarget::Notice, Some(id)) if valid_id(id) => Ok(()),
                (RevealTarget::Notice, _) => {
                    Err(ValidationError::new("id", "expected a notice id"))
                }
                (RevealTarget::Logs | RevealTarget::DataFolder, None) => Ok(()),
                (RevealTarget::Logs | RevealTarget::DataFolder, Some(_)) => Err(
                    ValidationError::new("id", "only a notice reveal names a notice"),
                ),
            },
        }
    }
}

/// One store the host could not read this run and moved aside before
/// starting it empty. Shown once, until dismissed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoreNotice {
    pub id: String,
    /// What was lost, as a phrase: "your threads list".
    pub what: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticsResponse {
    /// `report`: the text to put on the clipboard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// `notices` and `dismissNotice`: the notices still to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notices: Option<Vec<StoreNotice>>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/diagnostics-v39.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn diagnostics_commands_and_answers_match_the_shared_fixture() {
        let fixture = fixture();
        for request in fixture["requests"].as_array().expect("requests") {
            let command: DiagnosticsCommand =
                serde_json::from_value(request.clone()).expect("command decodes");
            command.validate().expect("valid command");
            assert_eq!(&serde_json::to_value(&command).expect("encode"), request);
        }
        for refused in fixture["refused"].as_array().expect("refused") {
            let decoded = serde_json::from_value::<DiagnosticsCommand>(refused.clone());
            assert!(
                decoded.is_err() || decoded.is_ok_and(|command| command.validate().is_err()),
                "{refused} must be refused"
            );
        }
        for response in fixture["responses"].as_array().expect("responses") {
            let decoded: DiagnosticsResponse =
                serde_json::from_value(response.clone()).expect("response decodes");
            assert_eq!(&serde_json::to_value(&decoded).expect("encode"), response);
        }
        assert_eq!(
            serde_json::to_value(DiagnosticsResponse::default()).expect("encode"),
            json!({})
        );
    }
}
