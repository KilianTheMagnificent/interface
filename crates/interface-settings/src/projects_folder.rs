//! Where New project creates a folder: the owner's choice from Change… when
//! it still exists, else `~/Developer/Projects` when it exists, else the
//! folder holding the last folder the owner picked in Interface, else
//! `~/Developer/Projects` again (New project creates it). Kept in
//! `projects-folder.v1.json`. Older builds never read the file and keep
//! creating in `~/Developer/Projects`. A file that does not decode is
//! quarantined: the cost is the default folder, never a host that cannot start.
use std::path::{Path, PathBuf};

use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};

use crate::{SettingsError, leased::LeasedDocument};

/// The default Projects folder, relative to home: outside iCloud Drive, which
/// syncs Desktop and stalls git, npm and cargo on evicted files.
pub const DEFAULT_PROJECTS_FOLDER: &str = "Developer/Projects";

const BYTES: usize = 16 * 1024;
const PATH_BYTES: usize = 4096;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    /// The folder the owner picked with Change….
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chosen: Option<String>,
    /// The folder holding the last folder the owner picked anywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_picked_parent: Option<String>,
}

/// Why the resolved folder is the one New project uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectsFolderOrigin {
    Chosen,
    Default,
    LastPicked,
}

/// The folder New project creates in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedProjectsFolder {
    pub path: PathBuf,
    pub origin: ProjectsFolderOrigin,
}

/// The owner's Projects folder choice and the last parent folder picked.
pub struct ProjectsFolderStore {
    document: LeasedDocument,
    state: State,
    /// Where an unreadable file was set aside at open.
    quarantined: Option<PathBuf>,
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= PATH_BYTES
        && !path.chars().any(char::is_control)
        && path != "/"
}

fn valid_state(state: &State) -> bool {
    state.version == 1
        && state.chosen.as_deref().is_none_or(valid_path)
        && state.last_picked_parent.as_deref().is_none_or(valid_path)
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 4, 16)?)
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

/// Resolves the folder New project creates in from the saved choice, with
/// `is_directory` deciding which candidates exist (tests pin it).
#[must_use]
pub fn resolve_projects_folder_with(
    home: &Path,
    chosen: Option<&Path>,
    last_picked_parent: Option<&Path>,
    mut is_directory: impl FnMut(&Path) -> bool,
) -> ResolvedProjectsFolder {
    if let Some(chosen) = chosen.filter(|path| is_directory(path)) {
        return ResolvedProjectsFolder {
            path: chosen.to_path_buf(),
            origin: ProjectsFolderOrigin::Chosen,
        };
    }
    let default = home.join(DEFAULT_PROJECTS_FOLDER);
    if is_directory(&default) {
        return ResolvedProjectsFolder {
            path: default,
            origin: ProjectsFolderOrigin::Default,
        };
    }
    if let Some(parent) = last_picked_parent.filter(|path| is_directory(path)) {
        return ResolvedProjectsFolder {
            path: parent.to_path_buf(),
            origin: ProjectsFolderOrigin::LastPicked,
        };
    }
    ResolvedProjectsFolder {
        path: default,
        origin: ProjectsFolderOrigin::Default,
    }
}

impl ProjectsFolderStore {
    /// # Errors
    /// Refuses unsafe or already leased storage. A file that does not decode
    /// is quarantined and the store starts empty.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            path,
            ".projects-folder.lock",
            limits()?,
            "The Projects folder setting is uncertain; restart Interface before changing it.",
            valid_state,
        )?;
        Ok(Self {
            document,
            state: state.unwrap_or(State {
                version: 1,
                ..State::default()
            }),
            quarantined,
        })
    }

    /// Whether opening found an unreadable file and set it aside.
    #[must_use]
    pub const fn quarantined(&self) -> bool {
        self.quarantined.is_some()
    }

    /// The folder New project creates in, checked against the disk now.
    #[must_use]
    pub fn resolve(&self, home: &Path) -> ResolvedProjectsFolder {
        resolve_projects_folder_with(
            home,
            self.state.chosen.as_deref().map(Path::new),
            self.state.last_picked_parent.as_deref().map(Path::new),
            Path::is_dir,
        )
    }

    /// Stores the folder the owner picked with Change….
    ///
    /// # Errors
    /// Refuses a relative, overlong or root path and failed storage.
    pub fn choose(&mut self, folder: &Path) -> Result<(), SettingsError> {
        let Some(text) = folder.to_str().filter(|text| valid_path(text)) else {
            return Err(invalid("Choose a folder other than the disk's root."));
        };
        let mut state = self.state.clone();
        state.chosen = Some(text.to_owned());
        self.commit(state)
    }

    /// Records the folder holding a folder the owner just picked, the default
    /// when `~/Developer/Projects` does not exist and nothing was chosen. The
    /// root and paths that do not fit are ignored.
    ///
    /// # Errors
    /// Failed storage.
    pub fn remember_picked(&mut self, picked: &Path) -> Result<(), SettingsError> {
        let Some(parent) = picked
            .parent()
            .and_then(Path::to_str)
            .filter(|text| valid_path(text))
        else {
            return Ok(());
        };
        let mut state = self.state.clone();
        state.last_picked_parent = Some(parent.to_owned());
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

    use super::*;

    const FILE: &str = "projects-folder.v1.json";

    #[test]
    fn resolution_prefers_the_choice_then_developer_projects_then_the_last_pick() {
        let home = Path::new("/home/owner");
        let developer = home.join(DEFAULT_PROJECTS_FOLDER);
        let chosen = Path::new("/Volumes/Work/code");
        let picked = Path::new("/home/owner/src");
        let all = |_: &Path| true;
        assert_eq!(
            resolve_projects_folder_with(home, Some(chosen), Some(picked), all),
            ResolvedProjectsFolder {
                path: chosen.to_path_buf(),
                origin: ProjectsFolderOrigin::Chosen
            }
        );
        // A chosen folder that went away falls back to Developer/Projects.
        assert_eq!(
            resolve_projects_folder_with(home, Some(chosen), Some(picked), |path| path != chosen),
            ResolvedProjectsFolder {
                path: developer.clone(),
                origin: ProjectsFolderOrigin::Default
            }
        );
        // Without Developer/Projects, the last picked parent folder.
        assert_eq!(
            resolve_projects_folder_with(home, None, Some(picked), |path| path == picked),
            ResolvedProjectsFolder {
                path: picked.to_path_buf(),
                origin: ProjectsFolderOrigin::LastPicked
            }
        );
        // Nothing exists: Developer/Projects, which New project creates.
        assert_eq!(
            resolve_projects_folder_with(home, Some(chosen), Some(picked), |_| false),
            ResolvedProjectsFolder {
                path: developer,
                origin: ProjectsFolderOrigin::Default
            }
        );
    }

    #[test]
    fn a_choice_and_the_last_pick_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let code = dir.path().join("code");
        let picked = dir.path().join("src/app");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&code).unwrap();
        fs::create_dir_all(&picked).unwrap();
        let path = dir.path().join(FILE);
        let mut store = ProjectsFolderStore::open(&path).unwrap();
        assert_eq!(store.resolve(&home).origin, ProjectsFolderOrigin::Default);
        store.remember_picked(&picked).unwrap();
        assert_eq!(
            store.resolve(&home),
            ResolvedProjectsFolder {
                path: dir.path().join("src"),
                origin: ProjectsFolderOrigin::LastPicked
            }
        );
        store.choose(&code).unwrap();
        drop(store);
        let reopened = ProjectsFolderStore::open(&path).unwrap();
        assert!(!reopened.quarantined());
        assert_eq!(
            reopened.resolve(&home),
            ResolvedProjectsFolder {
                path: code,
                origin: ProjectsFolderOrigin::Chosen
            }
        );
        let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["version"], 1);
    }

    #[test]
    fn the_root_is_never_a_projects_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProjectsFolderStore::open(&dir.path().join(FILE)).unwrap();
        assert!(store.choose(Path::new("/")).is_err());
        assert!(store.choose(Path::new("relative")).is_err());
        store.remember_picked(Path::new("/Applications")).unwrap();
        assert!(
            !dir.path().join(FILE).exists(),
            "picking a top-level folder records nothing"
        );
    }

    #[test]
    fn an_unreadable_file_is_set_aside_and_the_default_returns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        fs::write(&path, b"{\"version\":2}").unwrap();
        let store = ProjectsFolderStore::open(&path).unwrap();
        assert!(store.quarantined());
        assert_eq!(
            store.resolve(Path::new("/home/owner")).origin,
            ProjectsFolderOrigin::Default
        );
    }
}
