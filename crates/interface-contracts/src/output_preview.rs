//! Read-only, version-bound output inspection. Paths are relative to a host-authorized project.
use serde::{Deserialize, Serialize};

use crate::ValidationError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputPreviewAction {
    List,
    Read,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputPreviewRequest {
    pub version: u8,
    pub action: OutputPreviewAction,
    pub root: String,
    pub path: String,
}

impl OutputPreviewRequest {
    /// # Errors
    /// Refuses unknown versions, malformed roots, traversal and control characters.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.version != 1 {
            return Err(ValidationError::new("version", "expected 1"));
        }
        if !self.root.starts_with('/')
            || self.root.len() > 4096
            || self.root.chars().any(char::is_control)
        {
            return Err(ValidationError::new(
                "root",
                "expected an absolute project folder",
            ));
        }
        if self.path.len() > 4096
            || self.path.starts_with('/')
            || self.path.contains('\\')
            || self.path.chars().any(char::is_control)
            || self.path.split('/').any(|part| part == ".." || part == ".")
            || (self.action == OutputPreviewAction::Read && self.path.is_empty())
        {
            return Err(ValidationError::new(
                "path",
                "expected a relative path inside the project",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputPreviewEntry {
    pub name: String,
    pub directory: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputTextFormat {
    Plain,
    Markdown,
    Diff,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OutputPreviewContent {
    Text {
        text: String,
        format: OutputTextFormat,
    },
    Image {
        #[serde(rename = "mediaType")]
        media_type: String,
        bytes: Vec<u8>,
    },
    Pdf {
        bytes: Vec<u8>,
    },
    Unsupported {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputPreviewSnapshot {
    pub root: String,
    pub path: String,
    pub byte_length: u64,
    /// SHA-256 of the exact bytes displayed; absent when no content was read.
    pub revision: Option<String>,
    pub content: OutputPreviewContent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum OutputPreviewResponse {
    Directory {
        version: u8,
        root: String,
        path: String,
        entries: Vec<OutputPreviewEntry>,
    },
    File {
        version: u8,
        snapshot: OutputPreviewSnapshot,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_wire_fixture_round_trips() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/output-preview-v1.json"
        ))
        .unwrap();
        for value in fixture["requests"].as_array().unwrap() {
            let request: OutputPreviewRequest = serde_json::from_value(value.clone()).unwrap();
            request.validate().unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
        }
        for value in fixture["responses"].as_array().unwrap() {
            let response: OutputPreviewResponse = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(response).unwrap(), *value);
        }
    }

    #[test]
    fn refuses_traversal_and_unknown_fields() {
        let mut request = OutputPreviewRequest {
            version: 1,
            action: OutputPreviewAction::Read,
            root: "/project".into(),
            path: "report.md".into(),
        };
        for path in [
            "../secret",
            "/outside",
            "folder/../secret",
            "a\\b",
            "a\nb",
            "",
        ] {
            request.path = path.into();
            assert!(request.validate().is_err(), "{path}");
        }
        assert!(
            serde_json::from_str::<OutputPreviewRequest>(
                r#"{"version":1,"action":"read","root":"/project","path":"x","grant":true}"#
            )
            .is_err()
        );
    }
}
