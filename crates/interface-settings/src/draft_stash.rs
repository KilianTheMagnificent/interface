//! `draft-stash.v1.json` (T3 section 9, protocol 63): drafts a thread's
//! owner parked to answer something else, per thread key, newest last on
//! disk and newest first in a view. One leased writer (the process holding
//! `host.lock`), atomic writes, a file that does not decode or breaks a bound
//! is quarantined and the store starts empty.
//!
//! Bounds: a thread keeps [`DRAFT_STASH_PER_THREAD`] drafts, each at most
//! 64 KiB of text and [`DRAFT_STASH_ITEMS`] attachment
//! references; the whole file is at most [`DRAFT_STASH_FILE_BYTES`]. A stash
//! keeps references, never content: a diff chip's Review bytes are dropped
//! (they are rebuilt when the chip is sent). The file paths it holds keep
//! pasted images from being pruned ([`DraftStashStore::retained_paths`]).

use std::path::{Path, PathBuf};

use interface_contracts::draft_stash::{
    DRAFT_STASH_FILE_BYTES, DRAFT_STASH_ITEMS, DRAFT_STASH_PER_THREAD, DRAFT_STASH_PREVIEW_CHARS,
    DRAFT_STASH_VIEW_VERSION, DraftStashEntry, DraftStashSummary, DraftStashView, StashDraft,
};
use interface_contracts::ipc::PromptContextDescriptorKind;
use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};

use crate::leased::LeasedDocument;
use crate::private_path::SettingsError;

pub const DRAFT_STASH_FILE_NAME: &str = "draft-stash.v1.json";
const LOCK_NAME: &str = ".draft-stash.lock";
const UNCERTAIN: &str =
    "The stashed drafts may not have been saved. Restart Interface before stashing again.";
/// A thread key, as every other per-thread store bounds it.
const KEY_CHARS: usize = 512;

/// One stashed draft as the file keeps it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StashedDraft {
    /// The thread's key (its head's, for a switched thread).
    pub key: String,
    pub id: String,
    pub at: u64,
    pub draft: StashDraft,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    /// The next id's number: ids never repeat in one file.
    next: u64,
    stashes: Vec<StashedDraft>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            next: 1,
            stashes: Vec::new(),
        }
    }
}

fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= KEY_CHARS && !key.contains('\0')
}

fn valid_state(state: &State) -> bool {
    let mut ids = std::collections::BTreeSet::new();
    let mut per_thread = std::collections::BTreeMap::<&str, usize>::new();
    state.version == 1
        && state.stashes.iter().all(|stash| {
            let count = per_thread.entry(stash.key.as_str()).or_default();
            *count += 1;
            key_ok(&stash.key)
                && *count <= DRAFT_STASH_PER_THREAD
                && ids.insert(stash.id.as_str())
                && !stash.draft.is_empty()
                && stash.draft.validate().is_ok()
                && stash
                    .draft
                    .items
                    .iter()
                    .all(|item| item.safe_content.is_none())
        })
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(
        DRAFT_STASH_FILE_BYTES,
        8,
        // Each stash is a handful of nodes plus seven per item.
        64 + 4096 * (16 + 8 * DRAFT_STASH_ITEMS),
    )?)
}

fn invalid(message: impl Into<String>) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

/// What a stash keeps of a draft: references only (no Review bytes).
fn kept(mut draft: StashDraft) -> StashDraft {
    for item in &mut draft.items {
        item.safe_content = None;
    }
    draft
}

/// The text's first non-blank line, bounded, "…" when cut.
#[must_use]
pub fn stash_preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    match collapsed.char_indices().nth(DRAFT_STASH_PREVIEW_CHARS) {
        Some((end, _)) => {
            let cut: String = collapsed[..end]
                .chars()
                .take(DRAFT_STASH_PREVIEW_CHARS - 1)
                .collect();
            format!("{}…", cut.trim_end())
        }
        None => collapsed,
    }
}

/// The leased single writer of `draft-stash.v1.json`.
pub struct DraftStashStore {
    document: LeasedDocument,
    state: State,
    quarantined: Option<PathBuf>,
}

impl DraftStashStore {
    /// Opens (or creates) the store at `file`. A document that does not
    /// decode or breaks a bound is quarantined and the store starts empty.
    ///
    /// # Errors
    /// Refuses unsafe storage, a lease held by another writer, or a file over
    /// the byte bound.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            file,
            LOCK_NAME,
            limits()?,
            UNCERTAIN,
            valid_state,
        )?;
        Ok(Self {
            document,
            state: state.unwrap_or_default(),
            quarantined,
        })
    }

    /// Where an unreadable file found at open was set aside.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// `key`'s stashed drafts, newest first.
    #[must_use]
    pub fn view(&self, key: &str) -> DraftStashView {
        let mut stashes = self
            .state
            .stashes
            .iter()
            .filter(|stash| stash.key == key)
            .map(|stash| DraftStashSummary {
                id: stash.id.clone(),
                preview: stash_preview(&stash.draft.text),
                at: stash.at,
                items: u32::try_from(stash.draft.items.len()).unwrap_or(u32::MAX),
            })
            .collect::<Vec<_>>();
        stashes.reverse();
        DraftStashView {
            version: DRAFT_STASH_VIEW_VERSION,
            key: key.to_owned(),
            stashes,
            popped: None,
        }
    }

    /// How many drafts `key` keeps.
    #[must_use]
    pub fn count(&self, key: &str) -> usize {
        self.state
            .stashes
            .iter()
            .filter(|stash| stash.key == key)
            .count()
    }

    /// Every file path a stashed draft refers to: they keep pasted images.
    /// They grant nothing.
    #[must_use]
    pub fn retained_paths(&self) -> Vec<PathBuf> {
        self.state
            .stashes
            .iter()
            .flat_map(|stash| &stash.draft.items)
            .filter(|item| item.kind == PromptContextDescriptorKind::File)
            .map(|item| PathBuf::from(&item.value))
            .collect()
    }

    fn mint(state: &mut State, key: &str, at: u64, draft: StashDraft) -> String {
        let id = format!("stash-{}", state.next);
        state.next = state.next.saturating_add(1);
        state.stashes.push(StashedDraft {
            key: key.to_owned(),
            id: id.clone(),
            at,
            draft: kept(draft),
        });
        id
    }

    /// Stashes `draft` for `key`; answers its id.
    ///
    /// # Errors
    /// Refuses an empty or oversized draft, a thread that already keeps
    /// [`DRAFT_STASH_PER_THREAD`], a full file, and failed or uncertain
    /// storage. Nothing is stashed then.
    pub fn push(&mut self, key: &str, draft: StashDraft, at: u64) -> Result<String, SettingsError> {
        if !key_ok(key) {
            return Err(invalid("That is not a thread."));
        }
        if draft.is_empty() {
            return Err(invalid("The draft is empty; there is nothing to stash."));
        }
        draft.validate().map_err(invalid)?;
        if self.count(key) >= DRAFT_STASH_PER_THREAD {
            return Err(invalid(format!(
                "This thread already keeps {DRAFT_STASH_PER_THREAD} stashed drafts. Remove one, then stash again; nothing was stashed."
            )));
        }
        let mut state = self.state.clone();
        let id = Self::mint(&mut state, key, at, draft);
        self.commit(state)?;
        Ok(id)
    }

    /// Takes stash `id` of `key` back and removes it. A non-empty `current`
    /// is stashed in the same commit, so the composer's words are never lost:
    /// both happen or neither does.
    ///
    /// # Errors
    /// Refuses an unknown id, an oversized `current`, a full file, and failed
    /// or uncertain storage.
    pub fn pop(
        &mut self,
        key: &str,
        id: &str,
        current: Option<StashDraft>,
        at: u64,
    ) -> Result<DraftStashEntry, SettingsError> {
        let mut state = self.state.clone();
        let index = state
            .stashes
            .iter()
            .position(|stash| stash.key == key && stash.id == id)
            .ok_or_else(|| invalid("That stashed draft is gone."))?;
        let taken = state.stashes.remove(index);
        if let Some(current) = current.filter(|current| !current.is_empty()) {
            current.validate().map_err(invalid)?;
            Self::mint(&mut state, key, at, current);
        }
        self.commit(state)?;
        Ok(DraftStashEntry {
            id: taken.id,
            at: taken.at,
            draft: taken.draft,
        })
    }

    /// Removes stash `id` of `key`; an unknown id changes nothing.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn delete(&mut self, key: &str, id: &str) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        state
            .stashes
            .retain(|stash| !(stash.key == key && stash.id == id));
        self.commit(state)
    }

    /// A deleted or forgotten thread's drafts go with it.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn forget(&mut self, key: &str) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        state.stashes.retain(|stash| stash.key != key);
        self.commit(state)
    }

    /// A switched thread (T3 section 5) runs on `to` now: its drafts follow
    /// the head. Past [`DRAFT_STASH_PER_THREAD`] the oldest go.
    ///
    /// # Errors
    /// Refuses an invalid key, and failed or uncertain storage.
    pub fn follow(&mut self, from: &str, to: &str) -> Result<(), SettingsError> {
        if !key_ok(to) {
            return Err(invalid("Invalid thread to follow."));
        }
        if from == to {
            return Ok(());
        }
        let mut state = self.state.clone();
        for stash in &mut state.stashes {
            if stash.key == from {
                to.clone_into(&mut stash.key);
            }
        }
        // Stable by time: the newest kept.
        let mut seen = 0usize;
        let total = state.stashes.iter().filter(|stash| stash.key == to).count();
        let excess = total.saturating_sub(DRAFT_STASH_PER_THREAD);
        let mut order = state
            .stashes
            .iter()
            .enumerate()
            .filter(|(_, stash)| stash.key == to)
            .map(|(index, stash)| (stash.at, index))
            .collect::<Vec<_>>();
        order.sort_unstable();
        let dropped = order
            .into_iter()
            .take(excess)
            .map(|(_, index)| index)
            .collect::<std::collections::BTreeSet<_>>();
        state.stashes.retain(|_| {
            let keep = !dropped.contains(&seen);
            seen += 1;
            keep
        });
        self.commit(state)
    }

    fn commit(&mut self, state: State) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if state == self.state {
            return Ok(());
        }
        if !valid_state(&state) {
            return Err(invalid(
                "That change would break the stashed drafts' bounds.",
            ));
        }
        self.document.commit(&state).map_err(|error| match error {
            SettingsError::Storage(interface_storage::StorageError::BoundsExceeded { .. }) => {
                invalid("The stashed drafts are full (8 MB). Remove some, then stash again; nothing was stashed.")
            }
            other => other,
        })?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
#[path = "draft_stash_tests.rs"]
mod tests;
