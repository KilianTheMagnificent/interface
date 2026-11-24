//! Phone Threads v1 (review D6): the Mac's Threads on the iPhone, through the
//! same trusted connection as the terminal roster. Read-only apart from one
//! effect, answering a pending approval, which takes the phone's control lease
//! and a command receipt exactly as terminal input does. Independent of
//! desktop IPC and persisted schemas.
use serde::{Deserialize, Serialize};

use crate::threads::ThreadSection;

/// `RemoteThreadsV1::version` and `RemoteThreadDetailV1::version`.
pub const REMOTE_THREADS_VERSION: u8 = 1;

/// Additive phone inbox v2. The retained v1 list still lists only Threads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteInboxV2 {
    pub version: u8,
    pub items: Vec<RemoteInboxItemV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteInboxItemV2 {
    pub id: String,
    pub source: RemoteInboxSource,
    pub title: String,
    pub status: RemoteThreadStatus,
    pub detail: String,
    pub can_send: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<RemoteThreadV1>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteInboxSource {
    Thread,
    AgentThread,
    AgentTask,
}

/// What a thread is doing, from the session registry's status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteThreadStatus {
    /// Parked on an approval or a question.
    NeedsYou,
    Working,
    Idle,
    Failed,
}

/// One thread as the phone lists it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadV1 {
    /// `engine#sessionId`; the phone addresses it as `/{engine}/{sessionId}`.
    pub key: String,
    pub engine: String,
    pub session_id: String,
    pub title: String,
    /// The project folder it works in; absent for a plain chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub status: RemoteThreadStatus,
    pub section: ThreadSection,
    pub unread: bool,
    /// The registry's last activity, in its frozen ISO form.
    pub last_activity: String,
}

/// `GET /api/v1/threads`: needs you, then working, then the rest in the
/// Mac's Threads order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadsV1 {
    pub version: u8,
    pub threads: Vec<RemoteThreadV1>,
}

/// One recent turn, read-only: what was asked and what the agent wrote back.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadTurnV1 {
    pub prompt: String,
    pub reply: String,
    /// Tool calls the agent made in the turn.
    pub tools: u32,
    /// The prompt or reply was cut to fit the phone.
    pub clipped: bool,
}

/// The approval a thread is parked on.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadApprovalV1 {
    pub id: String,
    pub tool: String,
    pub summary: String,
    /// Allow once or Deny can answer it. A question stays on the Mac.
    pub answerable: bool,
}

/// `GET /api/v1/threads/{engine}/{sessionId}`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadDetailV1 {
    pub version: u8,
    pub thread: RemoteThreadV1,
    /// The most recent turns, oldest first.
    pub turns: Vec<RemoteThreadTurnV1>,
    /// Earlier turns not sent.
    pub older_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<RemoteThreadApprovalV1>,
    /// Why no turns could be read, when they could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteThreadDecisionV1 {
    AllowOnce,
    Deny,
}

/// `POST /api/v1/threads/{engine}/{sessionId}/approval`, with the control
/// lease and a new command id in `X-Interface-Control` and
/// `X-Interface-Command`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadApprovalResponseV1 {
    pub version: u8,
    pub approval_id: String,
    pub decision: RemoteThreadDecisionV1,
}

/// `POST /api/v1/threads/{engine}/{sessionId}/send`: a follow-up prompt for
/// a thread at rest, under the same lease and command id as an answer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteThreadSendV1 {
    pub version: u8,
    pub text: String,
}

/// The longest follow-up the phone sends, in bytes.
pub const REMOTE_THREAD_SEND_MAX_BYTES: usize = 12 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_v2_matches_the_native_and_typescript_fixture() {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-inbox-v2.json"
        ))
        .unwrap();
        let inbox: RemoteInboxV2 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(inbox.version, 2);
        assert!(inbox.items[1].thread.is_none());
        assert!(!inbox.items[2].can_send);
        assert_eq!(serde_json::to_value(inbox).unwrap(), value);
    }

    #[test]
    fn phone_threads_match_the_typescript_and_swift_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-phone-v4.json"
        ))
        .unwrap();
        let threads: RemoteThreadsV1 = serde_json::from_value(fixture["threads"].clone()).unwrap();
        assert_eq!(threads.version, REMOTE_THREADS_VERSION);
        assert_eq!(serde_json::to_value(&threads).unwrap(), fixture["threads"]);
        let detail: RemoteThreadDetailV1 =
            serde_json::from_value(fixture["detail"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&detail).unwrap(), fixture["detail"]);
        let response: RemoteThreadApprovalResponseV1 =
            serde_json::from_value(fixture["approvalResponse"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&response).unwrap(),
            fixture["approvalResponse"]
        );
        let mut always = fixture["approvalResponse"].clone();
        always["decision"] = "allowAlways".into();
        assert!(serde_json::from_value::<RemoteThreadApprovalResponseV1>(always).is_err());
        let mut text = fixture["approvalResponse"].clone();
        text["answers"] = serde_json::json!({"q": "a"});
        assert!(serde_json::from_value::<RemoteThreadApprovalResponseV1>(text).is_err());
        let send: RemoteThreadSendV1 = serde_json::from_value(fixture["send"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&send).unwrap(), fixture["send"]);
        let mut model = fixture["send"].clone();
        model["model"] = "other".into();
        assert!(serde_json::from_value::<RemoteThreadSendV1>(model).is_err());
    }
}
