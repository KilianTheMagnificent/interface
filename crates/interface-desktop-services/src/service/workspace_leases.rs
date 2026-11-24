//! Worktree cleanup leases: reference-counted protection for roots in use and
//! the exclusive reservation cleanup takes before deleting one.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{DesktopService, lexical_absolute};
use crate::DesktopServiceError;

#[derive(Default)]
pub(super) struct WorkspaceLeaseState {
    leases: BTreeMap<PathBuf, BTreeSet<String>>,
    pub(super) removals: BTreeSet<PathBuf>,
}

/// Exclusive deletion reservation for one managed root. While it is alive,
/// terminal lease acquisition and full saved-roster writes that could create a
/// new reference fail closed.
pub struct WorkspaceRemovalReservation<'a> {
    service: &'a DesktopService,
    root: PathBuf,
}

impl Drop for WorkspaceRemovalReservation<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.service.workspace_leases.lock() {
            state.removals.remove(&self.root);
        }
    }
}

impl DesktopService {
    #[must_use]
    pub fn workspace_is_open(&self, root: &Path) -> bool {
        let Some(root) = lexical_absolute(root) else {
            return false;
        };
        self.workspace_leases.lock().is_ok_and(|state| {
            state.leases.iter().any(|(leased, owners)| {
                !owners.is_empty() && workspace_roots_overlap(&root, leased)
            })
        })
    }

    /// Acquires a reference-counted cleanup lease for one exact internal
    /// consumer. Terminal IDs use distinct owners, so closing one terminal can
    /// never unprotect a worktree still used by another.
    ///
    /// # Errors
    ///
    /// Refuses non-absolute roots, empty/oversized owner identities, control
    /// bytes, or poisoned service state.
    pub fn acquire_workspace_lease(
        &self,
        root: &Path,
        owner: &str,
    ) -> Result<PathBuf, DesktopServiceError> {
        let root = lexical_absolute(root).ok_or_else(|| {
            DesktopServiceError::refused(
                "Could not protect that workspace.",
                "The workspace lease did not name an absolute local root.",
            )
        })?;
        if owner.is_empty() || owner.len() > 256 || owner.chars().any(char::is_control) {
            return Err(DesktopServiceError::refused(
                "Could not protect that workspace.",
                "The workspace lease owner was invalid.",
            ));
        }
        let mut state = self
            .workspace_leases
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?;
        if state
            .removals
            .iter()
            .any(|removing| workspace_roots_overlap(&root, removing))
        {
            return Err(DesktopServiceError::new(
                "maintenance-busy",
                "That managed workspace is finishing safe cleanup. Try opening it again.",
                true,
            ));
        }
        state
            .leases
            .entry(root.clone())
            .or_default()
            .insert(owner.to_owned());
        Ok(root)
    }

    /// Releases only the exact owner and prunes an empty root entry.
    ///
    /// # Errors
    ///
    /// Returns a state error when the lease table is unavailable.
    pub fn release_workspace_lease(
        &self,
        root: &Path,
        owner: &str,
    ) -> Result<(), DesktopServiceError> {
        let Some(root) = lexical_absolute(root) else {
            return Ok(());
        };
        let mut state = self
            .workspace_leases
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?;
        if let Some(owners) = state.leases.get_mut(&root) {
            owners.remove(owner);
            if owners.is_empty() {
                state.leases.remove(&root);
            }
        }
        Ok(())
    }

    /// Atomically reserves one managed root for verification and deletion.
    /// The reservation is refused while any overlapping live lease or durable
    /// saved terminal reference exists, and blocks both from appearing until
    /// the returned guard is dropped.
    ///
    /// # Errors
    ///
    /// Returns a state error when lease or saved-roster state is unavailable.
    pub fn reserve_workspace_removal(
        &self,
        root: &Path,
    ) -> Result<Option<WorkspaceRemovalReservation<'_>>, DesktopServiceError> {
        let root = lexical_absolute(root).ok_or_else(|| {
            DesktopServiceError::refused(
                "Could not reserve that workspace for cleanup.",
                "The managed workspace root was not an absolute local path.",
            )
        })?;
        let mut state = self
            .workspace_leases
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?;
        if state
            .removals
            .iter()
            .any(|removing| workspace_roots_overlap(&root, removing))
            || state.leases.iter().any(|(leased, owners)| {
                !owners.is_empty() && workspace_roots_overlap(&root, leased)
            })
        {
            return Ok(None);
        }
        let terminal_state = self.terminal_state()?;
        let terminal_store = &terminal_state.workspaces;
        if !terminal_store.is_available() {
            return Err(DesktopServiceError::new(
                "terminal-workspace-store-unavailable",
                "Interface could not safely inspect saved terminal workspaces.",
                true,
            ));
        }
        if terminal_store
            .saved_terminals()
            .filter_map(|terminal| lexical_absolute(Path::new(&terminal.cwd)))
            .any(|cwd| cwd == root || cwd.starts_with(&root))
        {
            return Ok(None);
        }
        state.removals.insert(root.clone());
        Ok(Some(WorkspaceRemovalReservation {
            service: self,
            root,
        }))
    }
}

fn workspace_roots_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}
