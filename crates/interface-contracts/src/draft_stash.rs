//! Composer references and stash (T3 section 9, protocol 63): the
//! `iface:draftStash` request channel. A thread's composer parks its draft
//! (text and attachment chips) in Rust's `draft-stash.v1.json` and takes it
//! back later. Fixture: `electron/src/fixtures/composer-references-v63.json`.

use serde::{Deserialize, Serialize};

use crate::ipc::{PROMPT_CONTEXT_VALIDATION_LIMITS, PromptContextDescriptor};

/// Every answer's `version`.
pub const DRAFT_STASH_VIEW_VERSION: u8 = 1;
/// A thread keeps at most this many stashed drafts.
pub const DRAFT_STASH_PER_THREAD: usize = 20;
/// A stashed draft's text, in UTF-8 bytes.
pub const DRAFT_STASH_TEXT_BYTES: usize = 64 * 1024;
/// A stashed draft's attachment references.
pub const DRAFT_STASH_ITEMS: usize = 16;
/// The whole store file, serialized.
pub const DRAFT_STASH_FILE_BYTES: usize = 8 * 1024 * 1024;
/// A list row's first line, in characters.
pub const DRAFT_STASH_PREVIEW_CHARS: usize = 120;

/// A draft as the composer holds it: its text and its chips.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashDraft {
    pub text: String,
    #[serde(default)]
    pub items: Vec<PromptContextDescriptor>,
}

impl StashDraft {
    /// Nothing typed and nothing attached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.items.is_empty()
    }

    /// The draft's bounds: text, item count and each item's fields (the
    /// prompt-context validation limits). Paths are not authorized here:
    /// a restored chip is validated again when it is sent.
    ///
    /// # Errors
    /// What is over a bound, as a sentence for the owner.
    pub fn validate(&self) -> Result<(), String> {
        if self.text.len() > DRAFT_STASH_TEXT_BYTES {
            return Err("That draft is longer than 64 KB; it was not stashed.".into());
        }
        if self.items.len() > DRAFT_STASH_ITEMS {
            return Err(format!(
                "A stashed draft keeps at most {DRAFT_STASH_ITEMS} attachments; nothing was stashed."
            ));
        }
        let limits = PROMPT_CONTEXT_VALIDATION_LIMITS;
        let field =
            |value: &str, max: usize| !value.trim().is_empty() && value.chars().count() <= max;
        let ok = self.items.iter().all(|item| {
            field(&item.id, limits.max_id_chars)
                && field(&item.label, limits.max_label_chars)
                && field(&item.value, limits.max_value_chars)
                && item
                    .detail
                    .as_deref()
                    .is_none_or(|detail| detail.chars().count() <= limits.max_detail_chars)
                && item
                    .estimated_tokens
                    .is_none_or(|tokens| tokens <= limits.max_estimated_tokens)
        });
        if !ok {
            return Err("An attachment of that draft is malformed; nothing was stashed.".into());
        }
        Ok(())
    }
}

/// One `iface:draftStash` command. `key` is the thread's session key; a
/// switched thread's earlier segment means its head.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DraftStashCommand {
    /// The thread's stashed drafts, newest first.
    List { key: String },
    /// Stashes `draft` (refused when empty, over a bound, or the thread
    /// already keeps [`DRAFT_STASH_PER_THREAD`]).
    Push { key: String, draft: StashDraft },
    /// Takes stash `id` back (`popped` in the answer) and removes it. A
    /// non-empty `current` (what the composer holds now) is stashed in the
    /// same commit, so nothing is lost; both happen or neither does.
    Pop {
        key: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<StashDraft>,
    },
    /// Removes stash `id` without restoring it.
    Delete { key: String, id: String },
}

impl DraftStashCommand {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::List { key }
            | Self::Push { key, .. }
            | Self::Pop { key, .. }
            | Self::Delete { key, .. } => key,
        }
    }

    /// The command's shape: a key, an id, bounded drafts.
    ///
    /// # Errors
    /// Why the command is refused, as a sentence.
    pub fn validate(&self) -> Result<(), String> {
        let key = self.key();
        if key.is_empty() || key.len() > 512 || key.contains('\0') {
            return Err("That is not a thread.".into());
        }
        match self {
            Self::List { .. } => Ok(()),
            Self::Push { draft, .. } => {
                if draft.is_empty() {
                    return Err("The draft is empty; there is nothing to stash.".into());
                }
                draft.validate()
            }
            Self::Pop { id, current, .. } => {
                valid_id(id)?;
                current.as_ref().map_or(Ok(()), StashDraft::validate)
            }
            Self::Delete { id, .. } => valid_id(id),
        }
    }
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("That stashed draft does not exist.".into());
    }
    Ok(())
}

/// One stashed draft in the list: its first line and when it was stashed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftStashSummary {
    pub id: String,
    /// The text's first non-blank line, at most
    /// [`DRAFT_STASH_PREVIEW_CHARS`] characters ("…" when cut); empty for a
    /// draft that is attachments only.
    pub preview: String,
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    /// How many attachments it keeps.
    pub items: u32,
}

/// A stashed draft taken back.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftStashEntry {
    pub id: String,
    pub at: u64,
    pub draft: StashDraft,
}

/// What every `iface:draftStash` command answers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftStashView {
    /// Always [`DRAFT_STASH_VIEW_VERSION`].
    pub version: u8,
    /// The thread the stashes belong to (its head).
    pub key: String,
    /// Newest first.
    pub stashes: Vec<DraftStashSummary>,
    /// `pop`'s draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popped: Option<DraftStashEntry>,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::ipc::PromptContextValidation;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/composer-references-v63.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn composer_references_v63_fixture_round_trips_every_shape() {
        let fixture = fixture();
        assert_eq!(fixture["channel"], crate::ipc::CHANNELS.draft_stash);
        assert_eq!(fixture["protocol"], 63);
        for request in fixture["requests"].as_array().expect("requests") {
            let command: DraftStashCommand =
                serde_json::from_value(request.clone()).expect("command");
            command.validate().expect("a valid command");
            assert_eq!(serde_json::to_value(&command).expect("encode"), *request);
        }
        for refused in fixture["refusedRequests"].as_array().expect("refused") {
            let decoded = serde_json::from_value::<DraftStashCommand>(refused.clone());
            assert!(
                decoded.is_err() || decoded.is_ok_and(|command| command.validate().is_err()),
                "{refused}"
            );
        }
        for name in ["view", "poppedView"] {
            let view: DraftStashView = serde_json::from_value(fixture[name].clone()).expect("view");
            assert_eq!(view.version, DRAFT_STASH_VIEW_VERSION);
            assert_eq!(serde_json::to_value(&view).expect("encode"), fixture[name]);
        }
        // A thread reference after validation: Rust's block in safeContent.
        let validated: PromptContextValidation =
            serde_json::from_value(fixture["threadReference"].clone()).expect("validation");
        assert!(validated.ok);
        assert!(validated.items[0].safe_content.is_some());
        assert_eq!(
            serde_json::to_value(&validated).expect("encode"),
            fixture["threadReference"]
        );
    }

    #[test]
    fn a_stash_draft_keeps_to_its_bounds() {
        let item = |id: &str| PromptContextDescriptor {
            id: id.into(),
            kind: crate::ipc::PromptContextDescriptorKind::File,
            label: "a.png".into(),
            value: "/tmp/a.png".into(),
            detail: None,
            estimated_tokens: None,
            safe_content: None,
        };
        let draft = |text: &str, items: usize| StashDraft {
            text: text.into(),
            items: (0..items).map(|index| item(&format!("i{index}"))).collect(),
        };
        assert!(draft("  ", 0).is_empty());
        assert!(!draft("", 1).is_empty());
        assert!(draft("x", DRAFT_STASH_ITEMS).validate().is_ok());
        assert!(draft("x", DRAFT_STASH_ITEMS + 1).validate().is_err());
        assert!(
            draft(&"x".repeat(DRAFT_STASH_TEXT_BYTES), 0)
                .validate()
                .is_ok()
        );
        assert!(
            draft(&"x".repeat(DRAFT_STASH_TEXT_BYTES + 1), 0)
                .validate()
                .is_err()
        );
        let push = |draft: StashDraft| DraftStashCommand::Push {
            key: "claude#a".into(),
            draft,
        };
        assert!(push(draft(" \n", 0)).validate().is_err());
        assert!(
            DraftStashCommand::Delete {
                key: "claude#a".into(),
                id: "../x".into()
            }
            .validate()
            .is_err()
        );
    }
}
