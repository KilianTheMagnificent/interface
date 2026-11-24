//! Durable explicit pane names, separate from the frozen workspace roster.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use interface_contracts::terminal::TerminalId;
use interface_storage::{JsonLimits, WriterLock, atomic_write_json, read_json, snapshot_target};
use serde::{Deserialize, Serialize};

use crate::private_path::{SettingsError, StoreLocation, map_document_write, prepare_store};

const LIMIT: usize = 1024 * 1024;

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u8,
    titles: BTreeMap<TerminalId, String>,
}

/// Only explicit Interface renames enter this owner-private versioned store.
pub struct TerminalTitleStore {
    location: StoreLocation,
    titles: BTreeMap<TerminalId, String>,
}

impl TerminalTitleStore {
    /// # Errors
    /// Refuses unsafe storage, unknown versions, or invalid names.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let location = prepare_store(file)?;
        let titles = read_titles(&location)?;
        Ok(Self { location, titles })
    }

    #[must_use]
    pub fn get(&self, id: TerminalId) -> Option<String> {
        self.titles.get(&id).cloned()
    }

    /// Commit the pin before acknowledging a rename, including an unchanged name.
    /// # Errors
    /// Reports invalid names, unsafe storage, concurrent writes, or uncertain durability.
    pub fn pin(&mut self, id: TerminalId, title: &str) -> Result<(), SettingsError> {
        if !valid_title(title) {
            return Err(SettingsError::InvalidTerminalWorkspace(
                "invalid pane name record".into(),
            ));
        }
        let lock = WriterLock::acquire(&self.location.directory, ".terminal-titles.lock")?;
        let expected = snapshot_target(&self.location.directory, &self.location.name, LIMIT)?;
        // A previous uncertain write may have put disk ahead of memory. Read
        // under the writer lease before merging another pin.
        let mut titles = read_titles(&self.location)?;
        if !titles.contains_key(&id) && titles.len() >= 8192 {
            return Err(SettingsError::InvalidTerminalWorkspace(
                "pane name limit reached".into(),
            ));
        }
        if titles.get(&id).is_some_and(|saved| saved == title) {
            lock.release(&self.location.directory)?;
            self.titles = titles;
            return Ok(());
        }
        titles.insert(id, title.to_owned());
        let document = Document { version: 1, titles };
        atomic_write_json(
            &self.location.directory,
            &lock,
            &self.location.name,
            &document,
            &expected,
            JsonLimits::new(LIMIT, 8, 32_768)?,
        )
        .map_err(|error| map_document_write(error, SettingsError::InvalidTerminalWorkspace))?;
        lock.release(&self.location.directory)?;
        self.titles = document.titles;
        Ok(())
    }

    /// Forget the names of panes outside `saved`, returning how many were
    /// dropped. Writes only when something is dropped. The caller passes a
    /// roster that proves which panes can still be reopened.
    /// # Errors
    /// Reports unsafe storage, concurrent writes, or uncertain durability.
    pub fn retain(&mut self, saved: &BTreeSet<TerminalId>) -> Result<usize, SettingsError> {
        let lock = WriterLock::acquire(&self.location.directory, ".terminal-titles.lock")?;
        let expected = snapshot_target(&self.location.directory, &self.location.name, LIMIT)?;
        let mut titles = read_titles(&self.location)?;
        let before = titles.len();
        titles.retain(|id, _| saved.contains(id));
        let dropped = before - titles.len();
        if dropped == 0 {
            lock.release(&self.location.directory)?;
            self.titles = titles;
            return Ok(0);
        }
        let document = Document { version: 1, titles };
        atomic_write_json(
            &self.location.directory,
            &lock,
            &self.location.name,
            &document,
            &expected,
            JsonLimits::new(LIMIT, 8, 32_768)?,
        )
        .map_err(|error| map_document_write(error, SettingsError::InvalidTerminalWorkspace))?;
        lock.release(&self.location.directory)?;
        self.titles = document.titles;
        Ok(dropped)
    }
}

fn read_titles(location: &StoreLocation) -> Result<BTreeMap<TerminalId, String>, SettingsError> {
    let loaded = read_json::<Document>(
        &location.directory,
        &location.name,
        JsonLimits::new(LIMIT, 8, 32_768)?,
    )?;
    let Some(loaded) = loaded else {
        return Ok(BTreeMap::new());
    };
    let document = loaded.value();
    if document.version != 1
        || document.titles.len() > 8192
        || document.titles.values().any(|title| !valid_title(title))
    {
        return Err(SettingsError::InvalidTerminalWorkspace(
            "invalid pane name record".into(),
        ));
    }
    Ok(document.titles.clone())
}

fn valid_title(title: &str) -> bool {
    !title.trim().is_empty() && title.chars().count() <= 120 && !title.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_names_survive_reopening_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("terminal-titles.v1.json");
        let id = TerminalId::random();
        let mut store = TerminalTitleStore::open(&file).unwrap();
        store.pin(id, "Please keep this exact name").unwrap();
        drop(store);
        let mut reopened = TerminalTitleStore::open(&file).unwrap();
        assert_eq!(
            reopened.get(id).as_deref(),
            Some("Please keep this exact name")
        );
        assert!(reopened.pin(id, "bad\nname").is_err());
        assert_eq!(
            reopened.get(id).as_deref(),
            Some("Please keep this exact name")
        );
        std::fs::write(&file, r#"{"version":2,"titles":{}}"#).unwrap();
        assert!(TerminalTitleStore::open(&file).is_err());
    }

    #[test]
    fn retain_forgets_only_panes_outside_the_roster_and_frees_the_cap() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("terminal-titles.v1.json");
        let kept = TerminalId::random();
        let mut store = TerminalTitleStore::open(&file).unwrap();
        store.pin(kept, "Kept").unwrap();
        let gone: Vec<_> = (1..8192).map(|_| TerminalId::random()).collect();
        // Fill the store to its cap the way years of renames would.
        let mut titles = BTreeMap::from([(kept, "Kept".to_owned())]);
        titles.extend(gone.iter().map(|id| (*id, "Old pane".to_owned())));
        std::fs::write(
            &file,
            serde_json::to_vec(&Document { version: 1, titles }).unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        let mut store = TerminalTitleStore::open(&file).unwrap();
        assert!(store.pin(TerminalId::random(), "New").is_err());

        let saved = BTreeSet::from([kept]);
        let unchanged = std::fs::read(&file).unwrap();
        assert_eq!(store.retain(&saved).unwrap(), gone.len());
        assert_ne!(std::fs::read(&file).unwrap(), unchanged);
        assert_eq!(store.get(kept).as_deref(), Some("Kept"));
        assert_eq!(store.get(gone[0]), None);

        // Nothing to drop: the file is left exactly as it was.
        let written = std::fs::read(&file).unwrap();
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(store.retain(&saved).unwrap(), 0);
        assert_eq!(std::fs::read(&file).unwrap(), written);
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            modified
        );

        let fresh = TerminalId::random();
        store.pin(fresh, "New").unwrap();
        drop(store);
        let reopened = TerminalTitleStore::open(&file).unwrap();
        assert_eq!(reopened.get(kept).as_deref(), Some("Kept"));
        assert_eq!(reopened.get(fresh).as_deref(), Some("New"));
        assert_eq!(reopened.get(gone[1]), None);
    }
}
