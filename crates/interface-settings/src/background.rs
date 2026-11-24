//! A separate leased v1 file keeps the existing preference schemas unchanged.
use crate::{SettingsError, leased::LeasedDocument};
use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    enabled: bool,
}

pub struct BackgroundStore {
    document: LeasedDocument,
    state: State,
}

impl BackgroundStore {
    /// # Errors
    /// Refuses an unsafe path or a competing writer.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state, _) = LeasedDocument::open_or_quarantine::<State>(
            path,
            ".background.lock",
            JsonLimits::new(4096, 3, 8)?,
            "Background preferences are uncertain; restart before changing them.",
            |state| state.version == 1,
        )?;
        // Preserve the established macOS close-to-background behavior.
        Ok(Self {
            document,
            state: state.unwrap_or(State {
                version: 1,
                enabled: true,
            }),
        })
    }
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.state.enabled
    }
    /// # Errors
    /// Refuses uncertain storage; updates the mirror only after durable commit.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if self.state.enabled != enabled {
            let next = State {
                version: 1,
                enabled,
            };
            self.document.commit(&next)?;
            self.state = next;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn background_choice_survives_restart_and_has_one_writer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.v1.json");
        let mut first = BackgroundStore::open(&path).unwrap();
        assert!(first.enabled());
        assert!(BackgroundStore::open(&path).is_err());
        first.set_enabled(false).unwrap();
        drop(first);
        let reopened = BackgroundStore::open(&path).unwrap();
        assert!(!reopened.enabled());
    }
}
