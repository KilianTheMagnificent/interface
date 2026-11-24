//! The thread journal (protocol 56): the app-owned, append-only record of what
//! happened in each thread, and the `iface:threadJournal` request that reads
//! it. Rust writes and reads it (`interface_settings::thread_journal`); the
//! renderer only asks. Contract:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 1. Both
//! languages read `electron/src/fixtures/thread-journal-v56.json`.
//!
//! Protocol 61 adds visual replies (section 8): the `visual` entry, the
//! `visual` command that reads a visual's page, and the answer's `visual`.
//! Both languages read `electron/src/fixtures/visual-replies-v61.json`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    ValidationError,
    engine::{ContextCompactTrigger, EngineErrorCause, EngineId, NoticeTone, PlanEntry},
    threads::RunOutcome,
};

/// The journal line format this build writes and reads.
pub const JOURNAL_FORMAT_VERSION: u32 = 1;
/// The most entries one `read` or `tail` answers.
pub const MAX_JOURNAL_PAGE_ENTRIES: u32 = 2_000;
/// What `read` and `tail` answer without a limit.
pub const DEFAULT_JOURNAL_PAGE_ENTRIES: u32 = 200;
/// The longest search query, in characters.
pub const MAX_JOURNAL_QUERY_CHARS: usize = 256;
/// The longest session key a command may name, in bytes.
pub const MAX_JOURNAL_KEY_BYTES: usize = 512;

/// Who sent the prompt that started a turn.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnOrigin {
    /// The thread's launch prompt.
    Launch,
    /// The owner sent it.
    Owner,
    /// Interface sent it itself (an Away round).
    Host,
    /// Sent while a turn ran and folded into it by the provider.
    Steer,
    /// Read from the provider's own history when the journal began.
    Import,
}

/// One thing that happened in a thread.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum JournalEntry {
    TurnStarted {
        origin: TurnOrigin,
        prompt: String,
        /// The Threads store's prompt digest.
        digest: String,
        #[serde(default, skip_serializing_if = "is_false")]
        cut: bool,
    },
    /// The send never reached the provider; readers drop that turn's entries.
    TurnRetracted {},
    /// One whole assistant message.
    Message {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        cut: bool,
    },
    ToolCall {
        call_id: String,
        tool: String,
        summary: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        cut: bool,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
    },
    ApprovalRequested {
        approval_id: String,
        tool: String,
        summary: String,
    },
    Diff {
        call_id: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_key: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        cut: bool,
    },
    Plan {
        entries: Vec<PlanEntry>,
    },
    SubagentStarted {
        agent_key: String,
        subagent_type: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    SubagentFinished {
        agent_key: String,
        ok: bool,
        summary: String,
    },
    PeerResult {
        operation_id: String,
        source_engine: EngineId,
        source_session_id: String,
        objective: String,
        text: String,
        untrusted: bool,
    },
    Notice {
        message: String,
        tone: NoticeTone,
    },
    Error {
        message: String,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cause: Option<EngineErrorCause>,
    },
    Compact {
        trigger: ContextCompactTrigger,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        post_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<bool>,
    },
    /// A page the agent showed through `show_visual` (protocol 61). Its
    /// bytes live in the thread's visual store, not here.
    Visual {
        id: String,
        title: String,
        bytes: u64,
    },
    TurnEnded {
        outcome: RunOutcome,
        /// `restart` when Interface found the turn open at start; `import`
        /// for a turn read from the provider's history.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    SessionClosed {},
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// One journal line after a segment's header.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalLine {
    /// From 1 per thread; never repeats.
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    /// How many `turnStarted` entries came before, from 0; absent before
    /// the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub entry: JournalEntry,
}

/// The first line of every segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalHeader {
    pub v: u32,
    pub key: String,
    pub first_seq: u64,
}

/// A page of one thread's journal.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalPage {
    pub key: String,
    pub entries: Vec<JournalLine>,
    /// The page's first entry (absent when the page is empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seq: Option<u64>,
    /// The thread's newest entry (0 when it has none).
    pub last_seq: u64,
    /// The oldest entry still kept: every `seq` below it is gone.
    pub floor_seq: u64,
    /// `read`: newer entries follow the page. `tail`: older kept entries
    /// come before it.
    pub has_more: bool,
}

/// Which side of the conversation a hit is from.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JournalRole {
    Prompt,
    Message,
}

/// One search match.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JournalHit {
    pub key: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub role: JournalRole,
    /// The match with a little text around it, on one line.
    pub excerpt: String,
    pub at: u64,
}

/// `iface:threadJournal`: reads only, so every command is replay-safe.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadJournalCommand {
    /// Entries after `after_seq` (from the oldest kept), oldest first.
    Read {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_seq: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// The newest entries, oldest first.
    Tail {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Prompts and messages across every thread.
    Search { query: String },
    /// One visual's page (protocol 61). `key` may be any session of the
    /// thread: a switched thread's visuals are found in every segment.
    Visual { key: String, id: String },
}

/// The longest visual title, in characters.
pub const MAX_VISUAL_TITLE_CHARS: usize = 120;

/// Whether `id` is a visual id as the host makes them: `v`, the
/// milliseconds it was shown at in 13 digits, `-`, 8 lowercase hex. Ids sort
/// oldest first; the check also keeps any path out of a file name.
#[must_use]
pub fn is_visual_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 23
        && bytes[0] == b'v'
        && bytes[1..14].iter().all(u8::is_ascii_digit)
        && bytes[14] == b'-'
        && bytes[15..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

impl ThreadJournalCommand {
    /// # Errors
    /// Refuses an empty or oversized key, a limit outside 1..=2000 and an
    /// empty or oversized query.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let key = |key: &str| {
            if key.is_empty() || key.len() > MAX_JOURNAL_KEY_BYTES || !key.contains('#') {
                Err(ValidationError::new("key", "expected a session key"))
            } else {
                Ok(())
            }
        };
        let limit = |limit: Option<u32>| match limit {
            Some(0) => Err(ValidationError::new("limit", "expected at least 1")),
            Some(limit) if limit > MAX_JOURNAL_PAGE_ENTRIES => {
                Err(ValidationError::new("limit", "expected at most 2000"))
            }
            _ => Ok(()),
        };
        match self {
            Self::Read {
                key: k, limit: l, ..
            }
            | Self::Tail { key: k, limit: l } => {
                key(k)?;
                limit(*l)
            }
            Self::Visual { key: k, id } => {
                key(k)?;
                if is_visual_id(id) {
                    Ok(())
                } else {
                    Err(ValidationError::new("id", "expected a visual id"))
                }
            }
            Self::Search { query } => {
                if query.trim().is_empty() {
                    Err(ValidationError::new("query", "expected some text"))
                } else if query.chars().count() > MAX_JOURNAL_QUERY_CHARS {
                    Err(ValidationError::new(
                        "query",
                        "expected at most 256 characters",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// A visual's page, as `visual` answers it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadVisual {
    pub id: String,
    /// The page exactly as the agent gave it. The renderer only ever shows
    /// it inside its sandboxed frame or as source text.
    pub html: String,
}

/// The answer: `page` for `read` and `tail`, `hits` for `search`, `visual`
/// for `visual` (absent when the thread no longer keeps that visual).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadJournalResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<JournalPage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hits: Option<Vec<JournalHit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visual: Option<ThreadVisual>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/thread-journal-v56.json"
        ))
        .expect("fixture")
    }

    fn round_trip<T>(value: &Value) -> T
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let decoded: T = serde_json::from_value(value.clone())
            .unwrap_or_else(|error| panic!("{value} decodes: {error}"));
        assert_eq!(&serde_json::to_value(&decoded).expect("encode"), value);
        decoded
    }

    #[test]
    fn thread_journal_fixture_round_trips_every_shape() {
        let fixture = fixture();
        assert_eq!(fixture["channel"], crate::ipc::CHANNELS.thread_journal);
        assert_eq!(fixture["protocol"], 56);
        let requests = fixture["requests"].as_array().expect("requests");
        assert_eq!(requests.len(), 4);
        for request in requests {
            round_trip::<ThreadJournalCommand>(request)
                .validate()
                .expect("valid command");
        }
        let refused = fixture["refused"].as_array().expect("refused");
        assert_eq!(refused.len(), 7);
        for refused in refused {
            let decoded = serde_json::from_value::<ThreadJournalCommand>(refused.clone());
            assert!(
                decoded.is_err() || decoded.is_ok_and(|command| command.validate().is_err()),
                "{refused} must be refused"
            );
        }
        let header = round_trip::<JournalHeader>(&fixture["header"]);
        assert_eq!(header.v, JOURNAL_FORMAT_VERSION);
        let lines = fixture["lines"].as_array().expect("lines");
        let kinds = lines
            .iter()
            .map(|line| round_trip::<JournalLine>(line).entry)
            .map(|entry| {
                serde_json::to_value(entry).expect("entry")["kind"]
                    .as_str()
                    .expect("kind")
                    .to_owned()
            })
            .collect::<std::collections::BTreeSet<_>>();
        // Every entry kind appears in the fixture.
        assert_eq!(kinds.len(), 16, "{kinds:?}");
        for response in fixture["responses"].as_array().expect("responses") {
            round_trip::<ThreadJournalResponse>(response);
        }
        assert_eq!(
            serde_json::to_value(ThreadJournalResponse::default()).expect("encode"),
            json!({})
        );
        for line in fixture["refusedLines"].as_array().expect("refusedLines") {
            assert!(
                serde_json::from_value::<JournalLine>(line.clone()).is_err(),
                "{line} must not decode"
            );
        }
    }

    #[test]
    fn visual_replies_fixture_round_trips_every_shape() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/visual-replies-v61.json"
        ))
        .expect("fixture");
        assert_eq!(fixture["channel"], crate::ipc::CHANNELS.thread_journal);
        assert_eq!(fixture["protocol"], 61);
        for event in fixture["events"].as_array().expect("events") {
            let decoded = round_trip::<crate::engine::EngineEvent>(event);
            assert!(matches!(
                decoded.event,
                crate::engine::EngineEventKind::VisualShown { .. }
            ));
        }
        for event in fixture["refusedEvents"].as_array().expect("refusedEvents") {
            assert!(
                serde_json::from_value::<crate::engine::EngineEvent>(event.clone()).is_err(),
                "{event} must not decode"
            );
        }
        for line in fixture["lines"].as_array().expect("lines") {
            let line = round_trip::<JournalLine>(line);
            assert!(matches!(line.entry, JournalEntry::Visual { .. }));
        }
        for line in fixture["refusedLines"].as_array().expect("refusedLines") {
            assert!(
                serde_json::from_value::<JournalLine>(line.clone()).is_err(),
                "{line} must not decode"
            );
        }
        for request in fixture["requests"].as_array().expect("requests") {
            round_trip::<ThreadJournalCommand>(request)
                .validate()
                .expect("valid command");
        }
        let refused = fixture["refused"].as_array().expect("refused");
        assert_eq!(refused.len(), 6);
        for refused in refused {
            let decoded = serde_json::from_value::<ThreadJournalCommand>(refused.clone());
            assert!(
                decoded.is_err() || decoded.is_ok_and(|command| command.validate().is_err()),
                "{refused} must be refused"
            );
        }
        for response in fixture["responses"].as_array().expect("responses") {
            round_trip::<ThreadJournalResponse>(response);
        }
    }

    #[test]
    fn visual_ids_are_time_ordered_and_path_free() {
        assert!(is_visual_id("v1731630000500-0a1b2c3d"));
        assert!(is_visual_id("v0000000000000-00000000"));
        for bad in [
            "",
            "v1731630000500-0a1b2c3",
            "v1731630000500-0a1b2c3dd",
            "w1731630000500-0a1b2c3d",
            "v1731630000500_0a1b2c3d",
            "v1731630000500-0A1B2C3D",
            "v17316300005a0-0a1b2c3d",
            "v1731630000500-../../..",
        ] {
            assert!(!is_visual_id(bad), "{bad}");
        }
        let mut ids = ["v1731630000600-00000000", "v1731630000500-ffffffff"];
        ids.sort_unstable();
        assert_eq!(ids[0], "v1731630000500-ffffffff");
    }
}
