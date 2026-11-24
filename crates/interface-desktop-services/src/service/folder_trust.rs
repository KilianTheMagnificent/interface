//! Folder trust: whether a folder's own settings and instruction files may
//! load into a Claude session started in it.
//!
//! One decision, [`DesktopService::folder_trust_of`], answers for the Claude
//! adapters (every launch and resume), the Threads view and the new-thread
//! question. A folder is trusted when any of these holds for its trust root
//! ([`interface_settings::folder_trust::trust_root`]):
//!
//! - the owner trusted it in Interface (`trusted-folders.v1.json`);
//! - Claude Code itself records the owner's trust for exactly that folder in
//!   the session's account's `.claude.json`;
//! - Interface made the folder and owns what is in it: an agent's home (a
//!   folder directly inside the agents root, never a repository cloned into
//!   one), and the folders the host registers with
//!   [`DesktopService::add_owned_folder`] (the Chat folder).
//!
//! Everything else is untrusted, including a folder that does not exist, a
//! store that could not be opened and a record that could not be read.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

use interface_settings::{
    FolderAnswer, SettingsError, TrustedFoldersStore,
    folder_trust::{ClaudeTrustCache, trust_root},
};

use super::DesktopService;
use crate::DesktopServiceError;

/// How long a folder's trust root is reused before it is worked out again
/// (a `git init` or a deleted folder shows within this long).
const ROOT_TTL: Duration = Duration::from_secs(5);
/// The most trust roots kept between reads.
const CACHED_ROOTS: usize = 1024;

/// Where a folder stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FolderTrustState {
    Trusted,
    /// Nobody has answered for this folder yet.
    Unasked,
    /// The owner said Not now.
    Declined,
}

/// The folder a trust answer covers and where it stands.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderTrustDecision {
    /// The trust root; `None` when the folder does not exist.
    pub root: Option<PathBuf>,
    pub state: FolderTrustState,
}

impl FolderTrustDecision {
    #[must_use]
    pub const fn trusted(&self) -> bool {
        matches!(self.state, FolderTrustState::Trusted)
    }
}

/// The folder-trust state the desktop service keeps.
#[derive(Default)]
pub(super) struct FolderTrustParts {
    /// `trusted-folders.v1.json`; `None` when it could not be opened, and
    /// then the owner's answers are unavailable and nothing is trusted
    /// through them.
    pub(super) store: Mutex<Option<TrustedFoldersStore>>,
    claude: ClaudeTrustCache,
    /// Folders Interface made and owns, canonical.
    owned: Mutex<Vec<PathBuf>>,
    roots: Mutex<HashMap<PathBuf, (Instant, Option<PathBuf>)>>,
}

impl FolderTrustParts {
    pub(super) fn open(data_root: Option<&Path>) -> Self {
        let store = data_root.and_then(|root| {
            TrustedFoldersStore::open(&root.join(interface_settings::TRUSTED_FOLDERS_FILE_NAME))
                .inspect(|store| {
                    if store.quarantined() {
                        eprintln!(
                            "[desktop] trusted-folders.v1.json was unreadable; set aside, nothing is trusted"
                        );
                    }
                })
                .inspect_err(|error| {
                    eprintln!("[desktop] trusted folders unavailable; nothing is trusted: {error}");
                })
                .ok()
        });
        Self {
            store: Mutex::new(store),
            ..Self::default()
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn store_error(error: &SettingsError) -> DesktopServiceError {
    let refused = DesktopServiceError::new(
        "folder-trust-unsaved",
        if error.write_may_have_landed() {
            "Interface may not have saved your answer for this folder. Check the folder's thread before trusting it again."
        } else {
            "Interface could not save your answer for this folder; nothing was recorded."
        },
        false,
    );
    if error.write_may_have_landed() {
        refused.outcome_unknown()
    } else {
        DesktopServiceError {
            certainty: crate::DesktopEffectCertainty::DefinitelyUnsent,
            ..refused
        }
    }
}

impl DesktopService {
    /// The trust root of `cwd` (see [`trust_root`]), reused for a few
    /// seconds so a thread list does not walk every thread's folders.
    fn cached_trust_root(&self, cwd: &Path) -> Option<PathBuf> {
        let now = Instant::now();
        if let Some((at, root)) = lock(&self.folder_trust.roots).get(cwd)
            && now.duration_since(*at) < ROOT_TTL
        {
            return root.clone();
        }
        let root = trust_root(cwd, &self.home_directory);
        let mut roots = lock(&self.folder_trust.roots);
        if roots.len() >= CACHED_ROOTS {
            roots.retain(|_, (at, _)| now.duration_since(*at) < ROOT_TTL);
            if roots.len() >= CACHED_ROOTS {
                roots.clear();
            }
        }
        roots.insert(cwd.to_path_buf(), (now, root.clone()));
        root
    }

    /// Whether a session started in `cwd`, on the account whose
    /// `.claude.json` is `claude_config` (`None`: an account Interface no
    /// longer knows, whose record is not read), may load the folder's own
    /// settings and instructions, and the folder the answer is for.
    #[must_use]
    pub fn folder_trust_of(&self, cwd: &Path, claude_config: Option<&Path>) -> FolderTrustDecision {
        let Some(root) = self.cached_trust_root(cwd) else {
            return FolderTrustDecision {
                root: None,
                state: FolderTrustState::Unasked,
            };
        };
        let answer = lock(&self.folder_trust.store)
            .as_ref()
            .map_or(FolderAnswer::Unasked, |store| store.answer(&root));
        let state = if answer == FolderAnswer::Trusted
            || claude_config.is_some_and(|config| self.folder_trust.claude.trusts(config, &root))
            || self.is_owned_folder(&root)
        {
            FolderTrustState::Trusted
        } else if answer == FolderAnswer::Declined {
            FolderTrustState::Declined
        } else {
            FolderTrustState::Unasked
        };
        FolderTrustDecision {
            root: Some(root),
            state,
        }
    }

    /// A folder Interface made and owns: an agent's home, or a folder inside
    /// one registered with [`Self::add_owned_folder`].
    fn is_owned_folder(&self, root: &Path) -> bool {
        self.is_agent_home(root)
            || lock(&self.folder_trust.owned)
                .iter()
                .any(|owned| root.starts_with(owned))
    }

    /// Exactly an agent's home: a folder directly inside the agents root.
    /// Interface writes what is there. A repository an agent cloned into its
    /// home is its own trust root, deeper than this, and is not the home: it
    /// is someone else's files and is answered for like any other folder.
    fn is_agent_home(&self, root: &Path) -> bool {
        std::fs::canonicalize(self.agents_root())
            .is_ok_and(|agents| root.parent() == Some(agents.as_path()))
    }

    /// Registers a folder Interface made and owns, whose sessions load what
    /// Interface wrote there (the Chat folder). Ignored when it does not
    /// exist.
    pub fn add_owned_folder(&self, folder: &Path) {
        if let Ok(canonical) = std::fs::canonicalize(folder) {
            let mut owned = lock(&self.folder_trust.owned);
            if !owned.contains(&canonical) {
                owned.push(canonical);
            }
        }
    }

    /// Records the owner's answer for `folder`'s trust root: `trust` trusts
    /// it; otherwise it is declined (a trusted folder stays trusted).
    /// Answers the trust root.
    ///
    /// # Errors
    /// A folder that does not exist, an unavailable store, or a failed write
    /// (outcome unknown when the write may have reached the disk).
    pub fn record_folder_trust(
        &self,
        folder: &Path,
        trust: bool,
    ) -> Result<PathBuf, DesktopServiceError> {
        let root = trust_root(folder, &self.home_directory).ok_or_else(|| {
            DesktopServiceError::refused(
                "That folder is not available.",
                "Nothing was recorded for it.",
            )
        })?;
        lock(&self.folder_trust.roots).remove(folder);
        let mut store = lock(&self.folder_trust.store);
        let store = store.as_mut().ok_or_else(|| {
            DesktopServiceError {
                certainty: crate::DesktopEffectCertainty::DefinitelyUnsent,
                ..DesktopServiceError::new(
                    "folder-trust-unavailable",
                    "Interface's list of trusted folders could not be opened, so nothing was recorded. Restart Interface and try again.",
                    false,
                )
            }
        })?;
        if trust {
            store.trust(&root)
        } else {
            store.decline(&root)
        }
        .map_err(|error| store_error(&error))?;
        Ok(root)
    }

    /// New project made `project`, an empty folder: nothing in it to
    /// distrust, so it is trusted from the start. Only when it is its own
    /// trust root: a new folder inside someone's repository is that
    /// repository's to answer for. Best effort: New project never fails
    /// because of this.
    pub(super) fn trust_new_project(&self, project: &Path) {
        let Ok(canonical) = std::fs::canonicalize(project) else {
            return;
        };
        if trust_root(&canonical, &self.home_directory).as_deref() != Some(canonical.as_path()) {
            return;
        }
        if let Some(store) = lock(&self.folder_trust.store).as_mut()
            && let Err(error) = store.trust(&canonical)
        {
            eprintln!("[desktop] a new project's folder was not recorded as trusted: {error}");
        }
    }
}

#[cfg(test)]
#[path = "folder_trust_tests.rs"]
mod tests;
