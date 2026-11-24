//! The owner's answers to "Trust this folder?", kept in
//! `trusted-folders.v1.json` beside `projects-folder.v1.json`.
//!
//! Each answer is for a trust root ([`crate::folder_trust::trust_root`]): a
//! repository's top folder, or a folder outside any repository. A file that
//! does not decode is quarantined and the store starts empty, so nothing is
//! trusted until the owner answers again: the cost of a damaged file is a
//! question asked twice, never a folder trusted by accident.
use std::path::Path;

use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};

use crate::{SettingsError, leased::LeasedDocument};

/// At most this many trusted folders, and as many declined ones.
pub const TRUSTED_FOLDERS_MAX_ENTRIES: usize = 4096;
/// A folder's path is at most this many bytes.
pub const TRUSTED_FOLDERS_MAX_PATH_BYTES: usize = 4096;
/// The store's file name, in the private data root.
pub const TRUSTED_FOLDERS_FILE_NAME: &str = "trusted-folders.v1.json";

/// Both lists full of the longest paths, quoted and separated, plus the rest.
const BYTES: usize = 2 * TRUSTED_FOLDERS_MAX_ENTRIES * (TRUSTED_FOLDERS_MAX_PATH_BYTES + 4) + 1024;
const NODES: usize = 2 * TRUSTED_FOLDERS_MAX_ENTRIES + 16;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    /// Sorted, no duplicates.
    trusted: Vec<String>,
    /// Sorted, no duplicates, none of them trusted.
    declined: Vec<String>,
}

/// What the owner said about one folder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FolderAnswer {
    Trusted,
    Declined,
    /// Nobody has answered for this folder yet.
    Unasked,
}

/// The owner's trust answers, by trust root.
pub struct TrustedFoldersStore {
    document: LeasedDocument,
    state: State,
    quarantined: bool,
}

/// An absolute, bounded folder path without control characters, other than
/// the disk's root.
#[must_use]
pub fn valid_trust_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= TRUSTED_FOLDERS_MAX_PATH_BYTES
        && !path.chars().any(char::is_control)
        && path != "/"
}

fn sorted_unique(list: &[String]) -> bool {
    list.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_state(state: &State) -> bool {
    state.version == 1
        && state.trusted.len() <= TRUSTED_FOLDERS_MAX_ENTRIES
        && state.declined.len() <= TRUSTED_FOLDERS_MAX_ENTRIES
        && state.trusted.iter().all(|path| valid_trust_path(path))
        && state.declined.iter().all(|path| valid_trust_path(path))
        && sorted_unique(&state.trusted)
        && sorted_unique(&state.declined)
        && state
            .declined
            .iter()
            .all(|path| state.trusted.binary_search(path).is_err())
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 4, NODES)?)
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

fn text_of(root: &Path) -> Result<&str, SettingsError> {
    root.to_str()
        .filter(|text| valid_trust_path(text))
        .ok_or_else(|| {
            invalid("That folder cannot be trusted: its path is not one Interface can keep.")
        })
}

fn insert(list: &mut Vec<String>, text: &str) -> Result<(), SettingsError> {
    match list.binary_search_by(|entry| entry.as_str().cmp(text)) {
        Ok(_) => Ok(()),
        Err(_) if list.len() >= TRUSTED_FOLDERS_MAX_ENTRIES => Err(invalid(
            "Interface already keeps the most folder answers it can; nothing was changed.",
        )),
        Err(at) => {
            list.insert(at, text.to_owned());
            Ok(())
        }
    }
}

impl TrustedFoldersStore {
    /// # Errors
    /// Refuses unsafe or already leased storage. A file that does not decode
    /// is quarantined and the store starts empty (nothing trusted).
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            path,
            ".trusted-folders.lock",
            limits()?,
            "The trusted folders list is uncertain; restart Interface before changing it.",
            valid_state,
        )?;
        Ok(Self {
            document,
            state: state.unwrap_or(State {
                version: 1,
                ..State::default()
            }),
            quarantined: quarantined.is_some(),
        })
    }

    /// Whether opening found an unreadable file and set it aside.
    #[must_use]
    pub const fn quarantined(&self) -> bool {
        self.quarantined
    }

    /// The owner's answer for exactly `root`.
    #[must_use]
    pub fn answer(&self, root: &Path) -> FolderAnswer {
        let Some(text) = root.to_str() else {
            return FolderAnswer::Unasked;
        };
        let has = |list: &[String]| {
            list.binary_search_by(|entry| entry.as_str().cmp(text))
                .is_ok()
        };
        if has(&self.state.trusted) {
            FolderAnswer::Trusted
        } else if has(&self.state.declined) {
            FolderAnswer::Declined
        } else {
            FolderAnswer::Unasked
        }
    }

    /// Records that the owner trusts `root`; a decline for it is dropped.
    ///
    /// # Errors
    /// A path that cannot be kept, a full list, or failed storage.
    pub fn trust(&mut self, root: &Path) -> Result<(), SettingsError> {
        let text = text_of(root)?;
        let mut state = self.state.clone();
        insert(&mut state.trusted, text)?;
        state.declined.retain(|entry| entry != text);
        self.commit(state)
    }

    /// Records that the owner declined `root`. A folder already trusted
    /// stays trusted: only the owner's Trust changes that.
    ///
    /// # Errors
    /// A path that cannot be kept, a full list, or failed storage.
    pub fn decline(&mut self, root: &Path) -> Result<(), SettingsError> {
        let text = text_of(root)?;
        if self.answer(root) == FolderAnswer::Trusted {
            return Ok(());
        }
        let mut state = self.state.clone();
        insert(&mut state.declined, text)?;
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
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn store() -> (tempfile::TempDir, PathBuf, TrustedFoldersStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TRUSTED_FOLDERS_FILE_NAME);
        let store = TrustedFoldersStore::open(&path).unwrap();
        (dir, path, store)
    }

    #[test]
    fn a_folder_nobody_answered_for_is_unasked_and_answers_survive_reopening() {
        let (_dir, path, mut store) = store();
        let project = Path::new("/Users/owner/code/project");
        let other = Path::new("/Users/owner/code/other");
        assert_eq!(store.answer(project), FolderAnswer::Unasked);
        store.trust(project).unwrap();
        store.decline(other).unwrap();
        drop(store);
        let reopened = TrustedFoldersStore::open(&path).unwrap();
        assert!(!reopened.quarantined());
        assert_eq!(reopened.answer(project), FolderAnswer::Trusted);
        assert_eq!(reopened.answer(other), FolderAnswer::Declined);
        assert_eq!(
            reopened.answer(Path::new("/Users/owner/code")),
            FolderAnswer::Unasked,
            "an answer covers exactly its folder"
        );
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved,
            serde_json::json!({
                "version": 1,
                "trusted": ["/Users/owner/code/project"],
                "declined": ["/Users/owner/code/other"],
            })
        );
    }

    #[test]
    fn trusting_a_declined_folder_drops_the_decline_and_a_decline_never_removes_a_trust() {
        let (_dir, _path, mut store) = store();
        let project = Path::new("/work/project");
        store.decline(project).unwrap();
        assert_eq!(store.answer(project), FolderAnswer::Declined);
        store.trust(project).unwrap();
        assert_eq!(store.answer(project), FolderAnswer::Trusted);
        store.decline(project).unwrap();
        assert_eq!(store.answer(project), FolderAnswer::Trusted);
    }

    #[test]
    fn only_absolute_bounded_paths_without_control_characters_are_kept() {
        let (_dir, path, mut store) = store();
        for refused in [
            PathBuf::from("relative/project"),
            PathBuf::from("/"),
            PathBuf::from("/tmp/a\nb"),
            PathBuf::from(format!("/{}", "a".repeat(TRUSTED_FOLDERS_MAX_PATH_BYTES))),
        ] {
            assert!(store.trust(&refused).is_err(), "{refused:?}");
            assert!(store.decline(&refused).is_err(), "{refused:?}");
        }
        assert!(!path.exists(), "nothing was written");
    }

    #[test]
    fn an_unreadable_or_inconsistent_file_is_set_aside_and_nothing_is_trusted() {
        for bytes in [
            &b"{\"version\":2,\"trusted\":[],\"declined\":[]}"[..],
            b"{\"version\":1,\"trusted\":[\"relative\"],\"declined\":[]}",
            b"{\"version\":1,\"trusted\":[\"/b\",\"/a\"],\"declined\":[]}",
            b"{\"version\":1,\"trusted\":[\"/a\"],\"declined\":[\"/a\"]}",
            b"{\"version\":1,\"trusted\":[\"/a\"],\"declined\":[],\"extra\":1}",
            b"not json",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(TRUSTED_FOLDERS_FILE_NAME);
            fs::write(&path, bytes).unwrap();
            let store = TrustedFoldersStore::open(&path).unwrap();
            assert!(store.quarantined(), "{}", String::from_utf8_lossy(bytes));
            assert_eq!(store.answer(Path::new("/a")), FolderAnswer::Unasked);
        }
    }

    #[test]
    fn each_list_holds_at_most_its_bound() {
        let (_dir, _path, mut store) = store();
        let mut state = store.state.clone();
        state.trusted = (0..TRUSTED_FOLDERS_MAX_ENTRIES)
            .map(|index| format!("/f/{index:05}"))
            .collect();
        store.commit(state).unwrap();
        assert!(store.trust(Path::new("/g/one-more")).is_err());
        assert!(store.trust(Path::new("/f/00001")).is_ok(), "already there");
        assert!(store.decline(Path::new("/g/declined")).is_ok());
    }
}
