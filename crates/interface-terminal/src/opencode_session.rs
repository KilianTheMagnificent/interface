//! Validate a private observation copied directly from the selected `OpenCode` TUI.
use std::path::Path;

use serde_json::Value;

use crate::{TerminalProviderSession, TerminalProviderSessionTransition};

pub const OPENCODE_SESSION_RECEIPT_FILE: &str = ".interface-opencode-session.json";

/// The host owns the private file read and live process check. A shared log's
/// newest chat is never evidence of which conversation this pane selected.
#[must_use]
pub fn opencode_session_receipt(
    value: &Value,
    capture_token: &str,
    process_id: u32,
    cwd: &Path,
) -> Option<TerminalProviderSession> {
    let fields = value.as_object()?;
    if fields.len() != 5
        || value.get("version")?.as_u64()? != 1
        || value.get("captureToken")?.as_str()? != capture_token
        || value.get("processId")?.as_u64()? != u64::from(process_id)
        || Path::new(value.get("cwd")?.as_str()?) != cwd
    {
        return None;
    }
    let id = value.get("sessionId")?.as_str()?;
    if !opencode_session_id(id) {
        return None;
    }
    Some(TerminalProviderSession {
        id: id.to_owned(),
        // Route changes, including selecting an old conversation, are explicit
        // provider transitions and may replace this pane's previous binding.
        transition: TerminalProviderSessionTransition::Resume,
    })
}

/// The exact `ses_…` shape `OpenCode` 1.18 issues: the prefix and a bounded
/// alphanumeric body, well inside the argv charset a binding may carry.
/// Receipts, bindings and resumes all hold to it; the renderer mirrors it in
/// `OPENCODE_CHAT_ID`. (The activity projection deliberately reads a wider
/// shape; see `opencode_activity`.)
pub(crate) fn opencode_session_id(value: &str) -> bool {
    let body = value.strip_prefix("ses_").unwrap_or_default();
    (8..=64).contains(&body.len()) && body.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_old_and_changed_conversations_without_a_creation_time_heuristic() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-opencode-session-v1.json"
        ))
        .unwrap();
        assert_eq!(
            opencode_session_receipt(
                &fixture,
                "11111111-1111-4111-8111-111111111111",
                42,
                Path::new("/project")
            )
            .unwrap()
            .id,
            "ses_existing123"
        );
        let mut receipt = serde_json::json!({"version":1,"captureToken":"pane-a",
            "processId":42,"cwd":"/project","sessionId":"ses_existing123"});
        let read =
            |value: &Value| opencode_session_receipt(value, "pane-a", 42, Path::new("/project"));
        assert_eq!(read(&receipt).unwrap().id, "ses_existing123");
        receipt["sessionId"] = "ses_selected456".into();
        assert_eq!(read(&receipt).unwrap().id, "ses_selected456");
        assert_eq!(
            read(&receipt).unwrap().transition,
            TerminalProviderSessionTransition::Resume
        );
    }

    #[test]
    fn refuses_cross_pane_stale_process_and_malformed_receipts() {
        let receipt = serde_json::json!({"version":1,"captureToken":"pane-a",
            "processId":42,"cwd":"/project","sessionId":"ses_existing123"});
        assert!(opencode_session_receipt(&receipt, "pane-b", 42, Path::new("/project")).is_none());
        assert!(opencode_session_receipt(&receipt, "pane-a", 43, Path::new("/project")).is_none());
        assert!(opencode_session_receipt(&receipt, "pane-a", 42, Path::new("/other")).is_none());
        for (field, value) in [
            ("version", Value::from(2)),
            ("sessionId", Value::from("../other")),
            ("extra", Value::Bool(true)),
            ("sessionId", Value::Null),
        ] {
            let mut invalid = receipt.clone();
            invalid[field] = value;
            assert!(
                opencode_session_receipt(&invalid, "pane-a", 42, Path::new("/project")).is_none()
            );
        }
    }
}
