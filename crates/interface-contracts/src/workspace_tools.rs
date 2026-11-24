//! Native workspace tools. Versioned separately from terminal launch profiles.
use crate::ValidationError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceTool {
    Files,
    Browser,
    IosSimulator,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenWorkspaceToolV1 {
    pub version: u8,
    pub root: String,
    pub tool: WorkspaceTool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl OpenWorkspaceToolV1 {
    /// # Errors
    /// Rejects unsupported versions, malformed roots and non-web browser URLs.
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
                "expected an absolute workspace directory",
            ));
        }
        match (self.tool, self.url.as_deref()) {
            (WorkspaceTool::Browser, Some(url)) if valid_browser_url(url) => Ok(()),
            (WorkspaceTool::Browser, _) => Err(ValidationError::new(
                "url",
                "enter an absolute http:// or https:// URL without embedded credentials",
            )),
            (_, None) => Ok(()),
            _ => Err(ValidationError::new("url", "only Browser accepts a URL")),
        }
    }
}

fn valid_browser_url(url: &str) -> bool {
    if url.len() > 8192
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !authority.is_empty()
        && !authority.contains('@')
        && !authority.starts_with(':')
        && authority
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_electron_fixture_round_trips() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/workspace-tools-v15.json"
        ))
        .unwrap();
        for value in fixtures.as_array().unwrap() {
            let request: OpenWorkspaceToolV1 = serde_json::from_value(value.clone()).unwrap();
            request.validate().unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
        }
    }

    #[test]
    fn native_tool_wire_and_url_boundary() {
        let request: OpenWorkspaceToolV1 = serde_json::from_str(r#"{"version":1,"root":"/project","tool":"browser","url":"http://localhost:3000/path?q=1"}"#).unwrap();
        request.validate().unwrap();
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"version":1,"root":"/project","tool":"browser","url":"http://localhost:3000/path?q=1"}"#
        );
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pass@example.com",
            "https://",
            "http://:80",
            "https://example.com\n",
            "https://example.com\\@evil",
        ] {
            assert!(
                OpenWorkspaceToolV1 {
                    url: Some(url.into()),
                    ..request.clone()
                }
                .validate()
                .is_err(),
                "{url}"
            );
        }
        for json in [
            r#"{"version":2,"root":"/project","tool":"files"}"#,
            r#"{"version":1,"root":"relative","tool":"files"}"#,
            r#"{"version":1,"root":"/project","tool":"ios-simulator","url":"https://example.com"}"#,
        ] {
            assert!(
                serde_json::from_str::<OpenWorkspaceToolV1>(json)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(
            serde_json::from_str::<OpenWorkspaceToolV1>(
                r#"{"version":1,"root":"/project","tool":"files","argv":[]}"#
            )
            .is_err()
        );
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalServerLink {
    pub process_id: u32,
    pub url: String,
    pub readiness: String,
}
