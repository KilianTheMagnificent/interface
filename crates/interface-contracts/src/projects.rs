//! The Projects folder New project creates in (protocol 40).
//!
//! `iface:createProject` keeps its v1 request, `{name}`, which now creates the
//! project in the folder this module describes. The same channel also takes a
//! `projectsFolder` request: `read` answers where New project will create, and
//! `choose` stores the folder the owner picked in the native picker. Electron's
//! main process injects `selectedPath` from that picker; whatever the renderer
//! put there is replaced.
use serde::{Deserialize, Serialize};

use crate::ValidationError;

/// The version of [`ProjectsFolderView`].
pub const PROJECTS_FOLDER_VIEW_VERSION: u8 = 1;

/// Longest folder path the view or a choice may carry, in bytes.
pub const PROJECTS_FOLDER_PATH_MAX: usize = 4096;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "projectsFolder",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProjectsFolderRequest {
    /// Where New project will create a folder. A struct variant, so that
    /// unknown fields are refused as they are for `Choose`.
    Read {},
    /// Store the folder the owner picked; `None` (cancelled) changes nothing.
    Choose {
        #[serde(default)]
        selected_path: Option<String>,
    },
}

impl ProjectsFolderRequest {
    /// # Errors
    /// Refuses a chosen path that is not absolute, too long or carries control
    /// characters.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Read {}
            | Self::Choose {
                selected_path: None,
            } => Ok(()),
            Self::Choose {
                selected_path: Some(path),
            } => {
                if !path.starts_with('/')
                    || path.len() > PROJECTS_FOLDER_PATH_MAX
                    || path.chars().any(char::is_control)
                {
                    return Err(ValidationError::new(
                        "selectedPath",
                        "expected an absolute folder",
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Why New project uses the folder it does.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectsFolderSource {
    /// The owner picked it with Change….
    Chosen,
    /// `~/Developer/Projects`: it exists, or nothing better is known (New
    /// project creates it).
    Default,
    /// The folder holding the last folder the owner picked in Interface,
    /// because `~/Developer/Projects` does not exist.
    LastPicked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectsFolderView {
    pub version: u8,
    /// The absolute folder New project creates in.
    pub path: String,
    /// `path` with the home folder written `~`, for the dialog.
    pub display: String,
    pub source: ProjectsFolderSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v40_projects_folder_fixture_round_trips_and_rejects_unknown_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/projects-folder-v40.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 40);
        for value in fixture["requests"].as_array().unwrap() {
            let request: ProjectsFolderRequest = serde_json::from_value(value.clone()).unwrap();
            request.validate().unwrap();
            assert_eq!(serde_json::to_value(&request).unwrap(), *value);
            let mut invalid = value.clone();
            invalid["name"] = serde_json::json!("not a folder request field");
            assert!(serde_json::from_value::<ProjectsFolderRequest>(invalid).is_err());
        }
        for value in fixture["views"].as_array().unwrap() {
            let view: ProjectsFolderView = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(view.version, PROJECTS_FOLDER_VIEW_VERSION);
            assert_eq!(serde_json::to_value(&view).unwrap(), *value);
            let mut invalid = value.clone();
            invalid["extra"] = serde_json::json!(true);
            assert!(serde_json::from_value::<ProjectsFolderView>(invalid).is_err());
        }
        // The v1 create request is not a folder request, and a folder request
        // is never mistaken for a create.
        assert!(
            serde_json::from_value::<ProjectsFolderRequest>(serde_json::json!({"name": "web"}))
                .is_err()
        );
    }

    #[test]
    fn a_chosen_folder_must_be_absolute_and_plain() {
        for bad in ["relative/path", "/with\nnewline", ""] {
            let request = ProjectsFolderRequest::Choose {
                selected_path: Some(bad.to_owned()),
            };
            assert!(request.validate().is_err(), "{bad:?}");
        }
        let long = format!("/{}", "a".repeat(PROJECTS_FOLDER_PATH_MAX));
        assert!(
            ProjectsFolderRequest::Choose {
                selected_path: Some(long)
            }
            .validate()
            .is_err()
        );
        assert!(
            ProjectsFolderRequest::Choose {
                selected_path: None
            }
            .validate()
            .is_ok()
        );
    }
}
