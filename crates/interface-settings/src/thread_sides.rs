//! `thread-sides.v1.json` (protocol 67, T3 section 12): each thread's open
//! side question: the side thread (a fork of the thread at its latest turn)
//! and the question it was asked. A side thread is never listed in the
//! inbox; it goes when its question is closed or its thread is removed.
//!
//! One leased writer (the process holding `host.lock`), atomic writes, a file
//! that does not decode or breaks a bound is quarantined and the store starts
//! empty. Bounds: one side question per thread, [`MAX_SIDES`] in all, a
//! question of at most [`MAX_QUESTION_BYTES`].

use std::path::{Path, PathBuf};

use interface_contracts::threads::ThreadSide;
use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};

use crate::leased::LeasedDocument;
use crate::private_path::SettingsError;

pub const THREAD_SIDES_FILE_NAME: &str = "thread-sides.v1.json";
const LOCK_NAME: &str = ".thread-sides.lock";
const UNCERTAIN: &str =
    "The side questions may not have been saved. Restart Interface before asking another.";
/// Side questions open at once, over every thread.
pub const MAX_SIDES: usize = 64;
/// A side question's text, in UTF-8 bytes.
pub const MAX_QUESTION_BYTES: usize = 16 * 1024;
const KEY_CHARS: usize = 512;

/// One open side question as the file keeps it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SideQuestion {
    /// The thread it was asked from (its head's key).
    pub parent: String,
    /// The side thread's session key.
    pub side: String,
    pub question: String,
    pub asked_at: u64,
}

impl SideQuestion {
    /// As a thread's view carries it.
    #[must_use]
    pub fn view(&self) -> ThreadSide {
        ThreadSide {
            key: self.side.clone(),
            question: self.question.clone(),
            asked_at: self.asked_at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    sides: Vec<SideQuestion>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            sides: Vec::new(),
        }
    }
}

fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= KEY_CHARS && !key.contains('\0')
}

/// Whether `question` may be asked: not blank, at most
/// [`MAX_QUESTION_BYTES`], no NUL.
#[must_use]
pub fn question_ok(question: &str) -> bool {
    !question.trim().is_empty() && question.len() <= MAX_QUESTION_BYTES && !question.contains('\0')
}

fn valid_state(state: &State) -> bool {
    let mut keys = std::collections::BTreeSet::new();
    state.version == 1
        && state.sides.len() <= MAX_SIDES
        && state.sides.iter().all(|side| {
            key_ok(&side.parent)
                && key_ok(&side.side)
                && side.parent != side.side
                && question_ok(&side.question)
                && keys.insert(side.parent.as_str())
                && keys.insert(side.side.as_str())
        })
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(2 * 1024 * 1024, 8, 64 + 8 * MAX_SIDES)?)
}

fn refused(message: impl Into<String>) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

/// The leased single writer of `thread-sides.v1.json`.
pub struct ThreadSideStore {
    document: LeasedDocument,
    state: State,
    quarantined: Option<PathBuf>,
}

impl ThreadSideStore {
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

    /// Every open side question, oldest first.
    #[must_use]
    pub fn sides(&self) -> &[SideQuestion] {
        &self.state.sides
    }

    /// `parent`'s open side question.
    #[must_use]
    pub fn of_parent(&self, parent: &str) -> Option<&SideQuestion> {
        self.state.sides.iter().find(|side| side.parent == parent)
    }

    /// Whether `key` is a side thread.
    #[must_use]
    pub fn is_side(&self, key: &str) -> bool {
        self.state.sides.iter().any(|side| side.side == key)
    }

    /// Records `side` as `parent`'s side question.
    ///
    /// # Errors
    /// `parent` already has one, the question or a key breaks its bound,
    /// every slot is taken, or the write failed.
    pub fn open_side(&mut self, side: SideQuestion) -> Result<(), SettingsError> {
        if self.of_parent(&side.parent).is_some() {
            return Err(refused(
                "This thread already has a side question open. Close it first.",
            ));
        }
        if !question_ok(&side.question) {
            return Err(refused("A side question is 1 byte to 16 KiB of text."));
        }
        if self.state.sides.len() >= MAX_SIDES {
            return Err(refused(
                "Too many side questions are open. Close one, then ask again.",
            ));
        }
        let mut state = self.state.clone();
        state.sides.push(side);
        self.commit(state)
    }

    /// Removes the side question `parent` has, or the one whose side thread
    /// is `parent` itself. Answers what it removed.
    ///
    /// # Errors
    /// The write failed; nothing was removed.
    pub fn close(&mut self, key: &str) -> Result<Option<SideQuestion>, SettingsError> {
        let Some(index) = self
            .state
            .sides
            .iter()
            .position(|side| side.parent == key || side.side == key)
        else {
            return Ok(None);
        };
        let mut state = self.state.clone();
        let removed = state.sides.remove(index);
        self.commit(state)?;
        Ok(Some(removed))
    }

    fn commit(&mut self, state: State) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if state == self.state {
            return Ok(());
        }
        if !valid_state(&state) {
            return Err(refused(
                "That change would break the side questions' bounds.",
            ));
        }
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
#[path = "thread_sides_tests.rs"]
mod tests;
