//! The permission mode the owner last chose for each session outside the
//! agent roster, kept in `session-access.v1.json` so that a resume after a
//! restart starts in it rather than in the provider's default. Only modes
//! other than the default are stored. Older builds never read this file, so
//! a rollback resumes every session in its default mode, as they always did.
//! A file that does not decode is quarantined: the cost is a resume in the
//! default mode, never a host that cannot start.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};

use crate::{SettingsError, leased::LeasedDocument};

/// The newest sessions kept; older entries are dropped on the next change.
const LIMIT: usize = 4_096;
const BYTES: usize = 1024 * 1024;
const KEY_CHARS: usize = 512;
const MODE_CHARS: usize = 64;
/// The token every provider's picker uses for "no mode of its own".
const DEFAULT_MODE: &str = "default";

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    sessions: BTreeMap<String, Entry>,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    mode: String,
    changed_at: u64,
}

/// Owner-chosen permission modes by session key.
pub struct SessionAccessStore {
    document: LeasedDocument,
    state: State,
    /// Where an unreadable file found at open was set aside.
    quarantined: Option<PathBuf>,
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 6, 8 * LIMIT + 16)?)
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= KEY_CHARS && !key.contains('\0')
}

fn valid_mode(mode: &str) -> bool {
    !mode.is_empty()
        && mode.len() <= MODE_CHARS
        && mode
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn valid_state(state: &State) -> bool {
    state.version == 1
        && state.sessions.len() <= LIMIT
        && state
            .sessions
            .iter()
            .all(|(key, entry)| valid_key(key) && valid_mode(&entry.mode))
}

impl SessionAccessStore {
    /// # Errors
    /// Refuses unsafe or already leased storage. A file that does not decode
    /// is quarantined and the store starts empty.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            path,
            ".session-access.lock",
            limits()?,
            "Session permission modes are uncertain; restart Interface before changing one.",
            valid_state,
        )?;
        let state = state.unwrap_or_else(|| State {
            version: 1,
            sessions: BTreeMap::new(),
        });
        Ok(Self {
            document,
            state,
            quarantined,
        })
    }

    /// Whether opening found an unreadable file and set it aside.
    #[must_use]
    pub const fn quarantined(&self) -> bool {
        self.quarantined.is_some()
    }

    /// Where that unreadable file now lives.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// The mode last chosen for `session_key`; `None` means its default.
    #[must_use]
    pub fn mode(&self, session_key: &str) -> Option<&str> {
        self.state
            .sessions
            .get(session_key)
            .map(|entry| entry.mode.as_str())
    }

    /// Records the mode just chosen for `session_key`; `None` or `default`
    /// forgets it. Past the limit the least recently changed entries go.
    ///
    /// # Errors
    /// Refuses an invalid key or mode and failed or uncertain storage.
    pub fn remember(
        &mut self,
        session_key: &str,
        mode: Option<&str>,
        now: u64,
    ) -> Result<(), SettingsError> {
        if !valid_key(session_key) {
            return Err(invalid("Invalid session key for a permission mode."));
        }
        let mode = mode.filter(|mode| *mode != DEFAULT_MODE);
        if mode.is_some_and(|mode| !valid_mode(mode)) {
            return Err(invalid("Invalid permission mode."));
        }
        let mut state = self.state.clone();
        match mode {
            None => {
                state.sessions.remove(session_key);
            }
            Some(mode) => {
                state.sessions.insert(
                    session_key.to_owned(),
                    Entry {
                        mode: mode.to_owned(),
                        changed_at: now,
                    },
                );
            }
        }
        while state.sessions.len() > LIMIT {
            let Some(oldest) = state
                .sessions
                .iter()
                .min_by_key(|(_, entry)| entry.changed_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            state.sessions.remove(&oldest);
        }
        self.commit(state)
    }

    fn commit(&mut self, state: State) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if state == self.state {
            return Ok(());
        }
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &tempfile::TempDir) -> SessionAccessStore {
        SessionAccessStore::open(&dir.path().join("session-access.v1.json")).unwrap()
    }

    #[test]
    fn a_chosen_mode_survives_reopening_and_default_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut access = store(&dir);
        access.remember("claude#one", Some("auto"), 1).unwrap();
        access.remember("codex#two", Some("never"), 2).unwrap();
        access.remember("claude#three", Some("plan"), 3).unwrap();
        access.remember("claude#three", Some("default"), 4).unwrap();
        access.remember("codex#two", None, 5).unwrap();
        drop(access);

        let reopened = store(&dir);
        assert_eq!(reopened.mode("claude#one"), Some("auto"));
        assert_eq!(reopened.mode("codex#two"), None);
        assert_eq!(reopened.mode("claude#three"), None);
        assert!(!reopened.quarantined());
    }

    #[test]
    fn invalid_modes_and_keys_are_refused_without_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut access = store(&dir);
        assert!(access.remember("claude#one", Some("auto mode"), 1).is_err());
        assert!(access.remember("", Some("auto"), 1).is_err());
        assert!(access.remember("claude#one", Some(""), 1).is_err());
        assert!(!dir.path().join("session-access.v1.json").exists());
    }

    #[test]
    fn the_least_recently_changed_entries_go_past_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut access = store(&dir);
        let mut state = access.state.clone();
        for index in 0..LIMIT {
            state.sessions.insert(
                format!("claude#{index}"),
                Entry {
                    mode: "auto".into(),
                    changed_at: 10 + index as u64,
                },
            );
        }
        access.commit(state).unwrap();
        access.remember("claude#0", Some("plan"), 100_000).unwrap();
        access
            .remember("claude#new", Some("auto"), 100_001)
            .unwrap();
        assert_eq!(access.state.sessions.len(), LIMIT);
        assert_eq!(access.mode("claude#0"), Some("plan"));
        assert_eq!(access.mode("claude#1"), None);
        assert_eq!(access.mode("claude#new"), Some("auto"));
    }

    #[test]
    fn an_unreadable_file_is_set_aside_and_the_store_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-access.v1.json");
        std::fs::write(&path, b"{\"version\":2,\"sessions\":{}}").unwrap();
        let mut access = SessionAccessStore::open(&path).unwrap();
        assert!(access.quarantined());
        // The set-aside bytes are kept where a notice can reveal them.
        let artifact = access.quarantined_artifact().unwrap().to_path_buf();
        assert_eq!(
            std::fs::read(&artifact).unwrap(),
            b"{\"version\":2,\"sessions\":{}}"
        );
        assert!(!path.exists());
        assert_eq!(access.mode("claude#one"), None);
        access.remember("claude#one", Some("auto"), 1).unwrap();
        drop(access);
        assert_eq!(store(&dir).mode("claude#one"), Some("auto"));
    }
}
