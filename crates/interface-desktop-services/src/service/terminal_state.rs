//! The saved terminal roster and the host-private restart authority checked
//! against it: their recoverable stores, the one lock over both, the host
//! restart API and the three roster operations.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use interface_contracts::terminal::{
    TerminalId, TerminalLaunchRequestV1, TerminalLaunchSessionV1, TerminalSummary,
    TerminalWorkspaceRecord, TerminalWorkspaceTerminal,
};
use interface_settings::{
    SettingsError, TerminalReopenAuthority, TerminalReopenAuthorityCompareExchange,
    TerminalReopenAuthoritySlot, TerminalReopenAuthorityStore, TerminalWorkspaceStore,
    terminal_identity_matches, validate_terminal_workspaces,
};
use serde_json::Value;

use super::{DesktopService, display_path, encode, map_settings_error};
use crate::DesktopServiceError;
use crate::payload::{PathPayload, SetTerminalWorkspacesPayload, decode, decode_empty};

pub(super) enum RecoverableTerminalWorkspaceStore {
    Ready(TerminalWorkspaceStore),
    Unavailable(PathBuf),
}

impl RecoverableTerminalWorkspaceStore {
    pub(super) fn get(&self) -> Vec<TerminalWorkspaceRecord> {
        match self {
            Self::Ready(store) => store.get(),
            Self::Unavailable(_) => Vec::new(),
        }
    }

    /// Every saved terminal, borrowed in roster order; empty when unavailable.
    pub(super) fn saved_terminals(&self) -> impl Iterator<Item = &TerminalWorkspaceTerminal> {
        let workspaces = match self {
            Self::Ready(store) => store.workspaces(),
            Self::Unavailable(_) => &[],
        };
        workspaces.iter().flat_map(|workspace| &workspace.terminals)
    }

    /// The saved roster entry for one terminal, cloned alone.
    pub(super) fn saved_terminal(
        &self,
        terminal_id: TerminalId,
    ) -> Option<TerminalWorkspaceTerminal> {
        self.saved_terminals()
            .find(|terminal| terminal.id == terminal_id)
            .cloned()
    }

    pub(super) const fn is_available(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    fn prepare_save(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
        moved_folders: &[(PathBuf, PathBuf)],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        match self {
            Self::Ready(store) => {
                store.preserve_provider_reopen_identities_following(workspaces, moved_folders)
            }
            Self::Unavailable(path) => {
                let mut store = TerminalWorkspaceStore::open(path)?;
                if let Err(error) = store.load() {
                    eprintln!(
                        "[desktop] replacing malformed terminal workspace roster during explicit repair: {error}"
                    );
                }
                let prepared = store
                    .preserve_provider_reopen_identities_following(workspaces, moved_folders)?;
                *self = Self::Ready(store);
                Ok(prepared)
            }
        }
    }

    /// Commits a roster already merged against this store: the output of
    /// [`Self::prepare_save`], or the current roster with only names changed.
    /// The caller holds the store's guard since preparing it, so the roster it
    /// was merged against is still current and no second merge is needed.
    /// Only a ready store is reachable here: `prepare_save` reopens an
    /// unavailable one first, and the name repair skips an unavailable one.
    pub(super) fn commit_prepared(
        &mut self,
        file: &Path,
        prepared: &[TerminalWorkspaceRecord],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        let Self::Ready(store) = self else {
            return Err(SettingsError::UnsafeStorage(
                "the saved terminal workspace roster is unavailable",
            ));
        };
        let result = store.save(prepared);
        if result.is_err() {
            // A post-rename failure can leave disk ahead of this in-memory
            // store. Discard it on every write failure so a later explicit
            // Settings save must reload the target (`prepare_save`) before it
            // can make another authoritative decision.
            *self = Self::Unavailable(file.to_path_buf());
        }
        result
    }

    fn reconcile_terminal_provider_reopen(
        &mut self,
        file: &Path,
        terminal_id: TerminalId,
        cwd: &str,
        launch: Option<TerminalLaunchRequestV1>,
        launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<bool, SettingsError> {
        match self {
            Self::Ready(store) => {
                let result = store.reconcile_terminal_provider_reopen(
                    terminal_id,
                    cwd,
                    launch,
                    launch_session,
                );
                if result.is_err() {
                    // See `save`: never continue from a possibly stale
                    // in-memory roster after an uncertain write outcome.
                    *self = Self::Unavailable(file.to_path_buf());
                }
                result
            }
            // No saved roster was admitted from an unavailable target, so
            // there is no host-proven membership to update. Never replace the
            // unreadable file from an unsolicited lifecycle side effect.
            Self::Unavailable(_) => Ok(false),
        }
    }

    fn terminal_provider_reopen_is_durable(
        &self,
        terminal_id: TerminalId,
        cwd: &str,
        launch: Option<TerminalLaunchRequestV1>,
        launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> bool {
        match self {
            Self::Ready(store) => {
                store.terminal_provider_reopen_is_durable(terminal_id, cwd, launch, launch_session)
            }
            Self::Unavailable(_) => false,
        }
    }
}

pub(super) enum RecoverableTerminalReopenAuthorityStore {
    Ready(TerminalReopenAuthorityStore),
    Unavailable,
}

impl RecoverableTerminalReopenAuthorityStore {
    fn get(&self, terminal_id: TerminalId) -> Option<TerminalReopenAuthority> {
        match self {
            Self::Ready(store) => store.get(terminal_id),
            Self::Unavailable => None,
        }
    }

    /// The store every write goes through; an unavailable one refuses them all.
    fn ready_mut(&mut self) -> Result<&mut TerminalReopenAuthorityStore, SettingsError> {
        match self {
            Self::Ready(store) => Ok(store),
            Self::Unavailable => Err(SettingsError::UnsafeStorage(
                "terminal reopen authority is unavailable",
            )),
        }
    }

    fn compare_exchange(
        &mut self,
        terminal_id: TerminalId,
        expected: &TerminalReopenAuthoritySlot,
        replacement: &TerminalReopenAuthoritySlot,
    ) -> Result<TerminalReopenAuthorityCompareExchange, SettingsError> {
        self.ready_mut()?
            .compare_exchange(terminal_id, expected, replacement)
    }

    fn upsert(&mut self, authority: &TerminalReopenAuthority) -> Result<bool, SettingsError> {
        self.ready_mut()?.upsert(authority)
    }

    fn remove(&mut self, terminal_id: TerminalId) -> Result<bool, SettingsError> {
        self.ready_mut()?.remove(terminal_id)
    }

    fn narrow_to_saved_preserving_unsaved(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
        previously_saved_ids: Option<&BTreeSet<TerminalId>>,
    ) -> Result<bool, SettingsError> {
        let store = self.ready_mut()?;
        match previously_saved_ids {
            Some(previously_saved_ids) => {
                store.narrow_to_saved_preserving_unsaved(workspaces, previously_saved_ids)
            }
            None => store.narrow_to_saved(workspaces),
        }
    }
}

/// The saved terminal roster and the host-private reopen authority checked
/// against it. One lock covers both, so every restart decision reads and
/// commits a consistent pair; the lease table, when needed, is taken first.
pub(super) struct TerminalState {
    pub(super) workspaces: RecoverableTerminalWorkspaceStore,
    pub(super) authorities: RecoverableTerminalReopenAuthorityStore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SavedTerminalProviderSessionPersistence {
    Durable,
    NotSaved,
}

/// Atomic classification of one terminal ID across the renderer roster and
/// host-private restart sidecar. Only `Unsaved` may be treated as a genuinely
/// new create; every saved-but-unproven state fails closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalReopenAuthorityState {
    Unsaved,
    Authority(TerminalReopenAuthority),
    PendingMigration(TerminalWorkspaceTerminal),
    Unproven,
    Unavailable,
}

impl DesktopService {
    /// Repair names before exposing the startup roster, retaining every session,
    /// layout and membership field. The caller supplies the terminal crate policy.
    /// # Errors
    /// Refuses unreadable stores or a failed/uncertain name repair.
    pub fn repair_terminal_titles<F>(&self, mut title: F) -> Result<(), DesktopServiceError>
    where
        F: FnMut(&TerminalWorkspaceTerminal) -> Result<String, String>,
    {
        let mut state = self.terminal_state()?;
        let store = &mut state.workspaces;
        if !store.is_available() {
            return Ok(());
        }
        let mut workspaces = store.get();
        if workspaces.is_empty() {
            return Ok(());
        }
        let mut changed = false;
        for terminal in workspaces.iter_mut().flat_map(|w| &mut w.terminals) {
            let repaired = title(terminal).map_err(|message| {
                DesktopServiceError::new("terminal-name-unavailable", message, false)
            })?;
            if repaired != terminal.title {
                terminal.title = repaired;
                changed = true;
            }
        }
        // Names almost always survive a restart unchanged; only a real repair
        // pays for the lock, atomic write and fsyncs on the startup path.
        if !changed {
            return Ok(());
        }
        store
            .commit_prepared(&self.terminal_workspaces_file, &workspaces)
            .map_err(map_settings_error)?;
        Ok(())
    }
    /// Reads the one-writer workspace roster without acquiring new path authority.
    /// # Errors
    /// Returns a state error when the authoritative roster is unavailable.
    pub fn remote_terminal_workspaces(
        &self,
    ) -> Result<Vec<interface_contracts::terminal::TerminalWorkspaceRecord>, DesktopServiceError>
    {
        Ok(self.terminal_state()?.workspaces.get())
    }

    /// Returns the current durable saved-terminal cwd snapshot. `None` means
    /// the one-writer state is unavailable, which deletion callers must treat
    /// as a fail-closed retention signal.
    #[must_use]
    pub fn saved_terminal_cwds(&self) -> Option<Vec<(TerminalId, PathBuf)>> {
        let state = self.terminal_state.lock().ok()?;
        let store = &state.workspaces;
        store.is_available().then(|| {
            store
                .saved_terminals()
                .map(|terminal| (terminal.id, PathBuf::from(&terminal.cwd)))
                .collect()
        })
    }

    /// Records one successful host-owned terminal spawn as future cold-reopen
    /// authority. This sidecar is never writable through the desktop wire.
    ///
    /// # Errors
    ///
    /// Returns an error when the private authority sidecar cannot be committed
    /// safely; the store is then disabled for the rest of this process.
    pub fn record_terminal_reopen_authority(
        &self,
        terminal: &TerminalSummary,
    ) -> Result<bool, DesktopServiceError> {
        self.record_terminal_reopen_authority_value(&TerminalReopenAuthority::from_summary(
            terminal,
        ))
    }

    /// Strengthens an existing successful-spawn proof with the exact verified
    /// managed-worktree ledger key. Later lifecycle upserts preserve this key.
    ///
    /// # Errors
    ///
    /// Returns an error when the private authority sidecar cannot be committed.
    pub fn record_terminal_managed_reopen_authority(
        &self,
        terminal: &TerminalSummary,
        managed_worktree_key: &str,
    ) -> Result<bool, DesktopServiceError> {
        let mut authority = TerminalReopenAuthority::from_summary(terminal);
        authority.managed_worktree_key = Some(managed_worktree_key.to_owned());
        self.record_terminal_reopen_authority_value(&authority)
    }

    pub(super) fn record_terminal_reopen_authority_value(
        &self,
        authority: &TerminalReopenAuthority,
    ) -> Result<bool, DesktopServiceError> {
        let mut state = self.terminal_state()?;
        let store = &mut state.authorities;
        match store.upsert(authority) {
            Ok(changed) => Ok(changed),
            Err(error) => {
                *store = RecoverableTerminalReopenAuthorityStore::Unavailable;
                Err(map_settings_error(error))
            }
        }
    }

    /// Atomically replaces one exact per-terminal authority slot. A normal
    /// mismatch is reported with the current slot and does not write; an
    /// unsafe or uncertain store failure poisons the in-process authority
    /// store until an explicit rollback can independently reconcile disk.
    ///
    /// Calling this with identical expected and replacement slots is a
    /// write-free validation/capture barrier for a host lifecycle transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the private authority sidecar is unavailable or
    /// the compare-and-exchange cannot be completed safely.
    pub fn compare_exchange_terminal_reopen_authority(
        &self,
        terminal_id: TerminalId,
        expected: &TerminalReopenAuthoritySlot,
        replacement: &TerminalReopenAuthoritySlot,
    ) -> Result<TerminalReopenAuthorityCompareExchange, DesktopServiceError> {
        let mut state = self.terminal_state()?;
        let store = &mut state.authorities;
        match store.compare_exchange(terminal_id, expected, replacement) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                *store = RecoverableTerminalReopenAuthorityStore::Unavailable;
                Err(map_settings_error(error))
            }
        }
    }

    /// Returns the exact raw sidecar slot under the terminal-state lock.
    /// This host-only view deliberately distinguishes unsaved absence,
    /// authority, and unresolved migration state.
    ///
    /// # Errors
    ///
    /// Returns an error when the private authority store is unavailable.
    pub fn terminal_reopen_authority_slot(
        &self,
        terminal_id: TerminalId,
    ) -> Result<TerminalReopenAuthoritySlot, DesktopServiceError> {
        let state = self.terminal_state()?;
        match &state.authorities {
            RecoverableTerminalReopenAuthorityStore::Ready(store) => Ok(store.slot(terminal_id)),
            RecoverableTerminalReopenAuthorityStore::Unavailable => {
                Err(DesktopServiceError::state_unavailable())
            }
        }
    }

    /// Removes one host proof when its terminal identity is explicitly
    /// dismissed. Renderer roster saves may also narrow the store, never add.
    ///
    /// # Errors
    ///
    /// Returns an error when the private sidecar cannot be committed safely.
    pub fn remove_terminal_reopen_authority(
        &self,
        terminal_id: TerminalId,
    ) -> Result<bool, DesktopServiceError> {
        let mut state = self.terminal_state()?;
        let store = &mut state.authorities;
        match store.remove(terminal_id) {
            Ok(changed) => Ok(changed),
            Err(error) => {
                *store = RecoverableTerminalReopenAuthorityStore::Unavailable;
                Err(map_settings_error(error))
            }
        }
    }

    /// Reconciles an uncertain authority compare-and-exchange from disk and
    /// restores the exact pre-transaction value before a hidden terminal
    /// rollback becomes externally complete. The prior value is restored only
    /// when disk still contains the exact attempted value. An already-restored
    /// prior is idempotent; every third value is preserved and reloaded as the
    /// current in-process authority before a typed conflict is returned.
    ///
    /// # Errors
    ///
    /// Returns an error when the sidecar cannot be reopened, loaded, restored,
    /// or read back exactly, or when a third value won the slot.
    pub fn rollback_terminal_reopen_authority(
        &self,
        terminal_id: TerminalId,
        prior: &TerminalReopenAuthoritySlot,
        attempted: &TerminalReopenAuthoritySlot,
    ) -> Result<bool, DesktopServiceError> {
        enum RollbackOutcome {
            Restored(bool, TerminalReopenAuthorityStore),
            PreservedWinner(TerminalReopenAuthorityStore),
        }
        let mut state = self.terminal_state()?;
        let slot = &mut state.authorities;
        let restore = (|| -> Result<RollbackOutcome, DesktopServiceError> {
            let mut reconciled =
                TerminalReopenAuthorityStore::open(&self.terminal_reopen_authority_file)
                    .map_err(map_settings_error)?;
            reconciled.load().map_err(map_settings_error)?;
            let (changed, preserved_winner) = match reconciled
                .compare_exchange(terminal_id, attempted, prior)
                .map_err(map_settings_error)?
            {
                TerminalReopenAuthorityCompareExchange::Exchanged => (true, None),
                TerminalReopenAuthorityCompareExchange::AlreadyReplacement => (false, None),
                TerminalReopenAuthorityCompareExchange::Conflict(current) => (false, Some(current)),
            };

            // Read the named sidecar again rather than trusting the writer's
            // in-memory value. A second uncertain failure leaves `slot`
            // poisoned and this operation safely retryable.
            let mut verified =
                TerminalReopenAuthorityStore::open(&self.terminal_reopen_authority_file)
                    .map_err(map_settings_error)?;
            verified.load().map_err(map_settings_error)?;
            let expected = preserved_winner.as_ref().unwrap_or(prior);
            let exact = verified.slot(terminal_id) == *expected && verified.is_initialized();
            if !exact {
                return Err(map_settings_error(SettingsError::UnsafeStorage(
                    "terminal reopen authority rollback did not read back exactly",
                )));
            }
            if preserved_winner.is_some() {
                Ok(RollbackOutcome::PreservedWinner(verified))
            } else {
                Ok(RollbackOutcome::Restored(changed, verified))
            }
        })();
        match restore {
            Ok(RollbackOutcome::Restored(changed, verified)) => {
                *slot = RecoverableTerminalReopenAuthorityStore::Ready(verified);
                Ok(changed)
            }
            Ok(RollbackOutcome::PreservedWinner(verified)) => {
                *slot = RecoverableTerminalReopenAuthorityStore::Ready(verified);
                Err(DesktopServiceError::new(
                    "terminal-reopen-authority-rollback-conflict",
                    "Interface preserved newer terminal restart authority instead of overwriting it during rollback.",
                    false,
                ))
            }
            Err(error) => {
                *slot = RecoverableTerminalReopenAuthorityStore::Unavailable;
                Err(error)
            }
        }
    }

    /// Finds host-owned proof for a fresh terminal in an exact saved workspace
    /// root. A renderer roster alone grants nothing: its retained terminal must
    /// still match a successful spawn in the private sidecar. Descendant cwds
    /// are not roots. The caller must re-verify managed ownership and recheck
    /// this proof before publishing a new process.
    #[must_use]
    pub fn saved_terminal_root_authority(&self, root: &Path) -> Option<TerminalReopenAuthority> {
        let state = self.terminal_state.lock().ok()?;
        let (workspaces, authorities) = (&state.workspaces, &state.authorities);
        if !workspaces.is_available() {
            return None;
        }
        workspaces.saved_terminals().find_map(|saved| {
            let authority = authorities.get(saved.id)?;
            (Path::new(&authority.root) == root
                && terminal_identity_matches(saved, &authority.terminal))
            .then_some(authority)
        })
    }

    /// Returns the host-private proof for a terminal that the caller has
    /// already established is present in the current in-memory terminal
    /// service. Unlike cold-reopen lookup, this deliberately does not require a
    /// renderer roster entry: a newly opened, unsaved tab may exit and restart
    /// before the renderer's next durable roster save.
    ///
    /// This method is not exposed through the desktop wire. Cold startup must
    /// continue to use [`Self::terminal_reopen_authority_state`], which requires
    /// exact saved-roster membership.
    ///
    /// # Errors
    ///
    /// Returns an error when the host-private authority store is unavailable.
    pub fn live_terminal_reopen_authority(
        &self,
        terminal_id: TerminalId,
    ) -> Result<Option<TerminalReopenAuthority>, DesktopServiceError> {
        let state = self.terminal_state()?;
        let RecoverableTerminalReopenAuthorityStore::Ready(authorities) = &state.authorities else {
            return Err(DesktopServiceError::state_unavailable());
        };
        Ok(authorities.get(terminal_id))
    }

    /// Classifies a terminal ID under the one terminal-state lock so an
    /// unavailable sidecar can never masquerade as an unsaved/new terminal.
    #[must_use]
    pub fn terminal_reopen_authority_state(
        &self,
        terminal_id: TerminalId,
    ) -> TerminalReopenAuthorityState {
        let Ok(state) = self.terminal_state.lock() else {
            return TerminalReopenAuthorityState::Unavailable;
        };
        let workspaces = &state.workspaces;
        if !workspaces.is_available() {
            return TerminalReopenAuthorityState::Unavailable;
        }
        let RecoverableTerminalReopenAuthorityStore::Ready(authorities) = &state.authorities else {
            return TerminalReopenAuthorityState::Unavailable;
        };
        let saved = workspaces.saved_terminal(terminal_id);
        let Some(saved) = saved else {
            return TerminalReopenAuthorityState::Unsaved;
        };
        if let Some(authority) = authorities.get(terminal_id)
            && terminal_identity_matches(&saved, &authority.terminal)
        {
            return TerminalReopenAuthorityState::Authority(authority);
        }
        if let Some(pending) = authorities.pending_migration(terminal_id)
            && terminal_identity_matches(&saved, &pending)
        {
            return TerminalReopenAuthorityState::PendingMigration(pending);
        }
        TerminalReopenAuthorityState::Unproven
    }

    /// Resolves one startup migration candidate after asynchronous ledger
    /// verification. The latest roster is rechecked under the terminal-store
    /// lock, fencing renderer changes that raced verification.
    ///
    /// # Errors
    ///
    /// Returns an error when the authority sidecar is unavailable or cannot be
    /// committed safely.
    pub fn resolve_terminal_reopen_authority_migration(
        &self,
        terminal_id: TerminalId,
        verified: Option<&TerminalReopenAuthority>,
    ) -> Result<bool, DesktopServiceError> {
        let mut state = self.terminal_state()?;
        let saved = state.workspaces.saved_terminal(terminal_id);
        let authorities = &mut state.authorities;
        match authorities {
            RecoverableTerminalReopenAuthorityStore::Ready(store) => {
                let pending = store.pending_migration(terminal_id);
                let still_current = saved.as_ref().is_some_and(|saved| {
                    pending
                        .as_ref()
                        .is_some_and(|pending| terminal_identity_matches(saved, pending))
                        && verified.is_none_or(|authority| {
                            terminal_identity_matches(saved, &authority.terminal)
                        })
                });
                if !still_current {
                    return Ok(false);
                }
                match store.resolve_pending_migration(terminal_id, verified) {
                    Ok(changed) => Ok(changed),
                    Err(error) => {
                        // The sidecar may already contain the resolution when
                        // a post-rename durability check fails. Never keep
                        // serving the pre-write in-memory migration state.
                        *authorities = RecoverableTerminalReopenAuthorityStore::Unavailable;
                        Err(map_settings_error(error))
                    }
                }
            }
            RecoverableTerminalReopenAuthorityStore::Unavailable => Err(DesktopServiceError::new(
                "terminal-reopen-authority-unavailable",
                "Interface could not safely migrate that saved terminal worktree.",
                true,
            )),
        }
    }

    fn follow_moved_folders(
        state: &mut TerminalState,
        moves: &[(PathBuf, PathBuf)],
    ) -> Result<(), DesktopServiceError> {
        if moves.is_empty() {
            return Ok(());
        }
        let store = &mut state.authorities;
        let RecoverableTerminalReopenAuthorityStore::Ready(authorities) = store else {
            return Err(DesktopServiceError::state_unavailable());
        };
        for (from, to) in moves {
            if let Err(error) = authorities.follow_moved_folder(from, to) {
                eprintln!(
                    "[desktop] terminal reopen authority could not follow a moved folder: {error}"
                );
                *store = RecoverableTerminalReopenAuthorityStore::Unavailable;
                return Err(map_settings_error(error));
            }
        }
        Ok(())
    }

    fn narrow_terminal_reopen_authorities(
        &self,
        state: &mut TerminalState,
        workspaces: &[TerminalWorkspaceRecord],
        previously_saved_ids: Option<&BTreeSet<TerminalId>>,
    ) -> Result<bool, DesktopServiceError> {
        let store = &mut state.authorities;
        if matches!(*store, RecoverableTerminalReopenAuthorityStore::Unavailable) {
            // An explicit, validated Settings save is the only renderer action
            // allowed to repair this host-private store. Reload a valid
            // sidecar so existing proofs can only be narrowed. If the target
            // is malformed, replace it with an initialized empty sidecar:
            // renderer data can repair availability, but can never mint proof
            // or a new startup-migration candidate.
            let mut repaired =
                TerminalReopenAuthorityStore::open(&self.terminal_reopen_authority_file)
                    .map_err(map_settings_error)?;
            let repair = match repaired.load() {
                Ok(_) if repaired.is_initialized() => repaired.narrow_to_saved(workspaces),
                Ok(_) => repaired.initialize_pending_migration(&[]),
                Err(error) => {
                    eprintln!(
                        "[desktop] replacing malformed terminal reopen authority during explicit roster repair: {error}"
                    );
                    repaired.initialize_pending_migration(&[])
                }
            };
            return match repair {
                Ok(changed) => {
                    *store = RecoverableTerminalReopenAuthorityStore::Ready(repaired);
                    Ok(changed)
                }
                Err(error) => Err(map_settings_error(error)),
            };
        }
        match store.narrow_to_saved_preserving_unsaved(workspaces, previously_saved_ids) {
            Ok(changed) => Ok(changed),
            Err(error) => {
                eprintln!(
                    "[desktop] terminal reopen authority could not be narrowed safely: {error}"
                );
                *store = RecoverableTerminalReopenAuthorityStore::Unavailable;
                Err(map_settings_error(error))
            }
        }
    }

    /// Atomically advance the complete host-proven provider reopen identity.
    /// The private authority commits before the renderer roster, so a crash can
    /// leave only an authority-ahead state that startup may safely repair.
    ///
    /// # Errors
    ///
    /// Returns an error when current saved identity no longer matches the
    /// private authority or either durable write cannot be verified.
    pub fn persist_terminal_provider_reopen(
        &self,
        terminal: &TerminalSummary,
    ) -> Result<SavedTerminalProviderSessionPersistence, DesktopServiceError> {
        self.persist_terminal_provider_reopen_identity(
            &terminal.root,
            &TerminalWorkspaceTerminal {
                id: terminal.id,
                title: terminal.title.clone(),
                cwd: terminal.cwd.clone(),
                session_key: terminal.session_key.clone(),
                launch: terminal.launch,
                launch_session: terminal.launch_session.clone(),
            },
        )
    }

    /// Atomically advance a host-proven provider reopen identity recovered
    /// before any live terminal summary exists. This is the startup companion
    /// to [`Self::persist_terminal_provider_reopen`].
    ///
    /// # Errors
    ///
    /// Returns an error when current saved identity no longer matches either
    /// the private authority or the exact authority-ahead recovery target.
    pub fn persist_terminal_provider_reopen_identity(
        &self,
        root: &str,
        terminal: &TerminalWorkspaceTerminal,
    ) -> Result<SavedTerminalProviderSessionPersistence, DesktopServiceError> {
        let mut guard = self.terminal_state()?;
        let TerminalState {
            workspaces,
            authorities,
        } = &mut *guard;
        if !workspaces.is_available() {
            return Err(DesktopServiceError::new(
                "terminal-workspace-store-unavailable",
                "Interface could not safely update the saved terminal workspace.",
                true,
            ));
        }
        let saved = workspaces.saved_terminal(terminal.id);
        let RecoverableTerminalReopenAuthorityStore::Ready(_) = authorities else {
            return Err(DesktopServiceError::new(
                "terminal-reopen-authority-unavailable",
                "Interface could not safely update that terminal's restart authority.",
                true,
            ));
        };
        let authority = authorities.get(terminal.id);
        if let (Some(saved), Some(authority)) = (&saved, &authority)
            && !terminal_identity_matches(saved, &authority.terminal)
            && !terminal_identity_matches(&authority.terminal, terminal)
        {
            return Err(DesktopServiceError::new(
                "terminal-reopen-authority-mismatch",
                "Interface refused stale terminal restart authority.",
                false,
            ));
        }
        if let Some(mut authority) = authority {
            if authority.root != root
                || authority.terminal.id != terminal.id
                || authority.terminal.session_key != terminal.session_key
            {
                return Err(DesktopServiceError::new(
                    "terminal-reopen-authority-mismatch",
                    "Interface refused a provider transition for another terminal identity.",
                    false,
                ));
            }
            authority.terminal.cwd.clone_from(&terminal.cwd);
            authority.terminal.launch = terminal.launch;
            authority
                .terminal
                .launch_session
                .clone_from(&terminal.launch_session);
            if let Err(error) = authorities.upsert(&authority) {
                *authorities = RecoverableTerminalReopenAuthorityStore::Unavailable;
                return Err(map_settings_error(error));
            }
        }
        if saved.is_none() {
            return Ok(SavedTerminalProviderSessionPersistence::NotSaved);
        }
        if authorities.get(terminal.id).is_none() {
            return Err(DesktopServiceError::new(
                "terminal-reopen-authority-unavailable",
                "Interface could not safely update that terminal's restart authority.",
                true,
            ));
        }
        workspaces
            .reconcile_terminal_provider_reopen(
                &self.terminal_workspaces_file,
                terminal.id,
                &terminal.cwd,
                terminal.launch,
                terminal.launch_session.as_ref(),
            )
            .map_err(map_settings_error)?;
        if workspaces.terminal_provider_reopen_is_durable(
            terminal.id,
            &terminal.cwd,
            terminal.launch,
            terminal.launch_session.as_ref(),
        ) {
            Ok(SavedTerminalProviderSessionPersistence::Durable)
        } else {
            Err(DesktopServiceError::new(
                "terminal-provider-reopen-not-durable",
                "Interface could not confirm the saved provider reopen identity.",
                true,
            ))
        }
    }
    /// The saved roster and reopen authority, locked together.
    pub(super) fn terminal_state(
        &self,
    ) -> Result<MutexGuard<'_, TerminalState>, DesktopServiceError> {
        self.terminal_state
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())
    }
    pub(super) fn get_terminal_workspaces(
        &self,
        operation: &str,
        payload: &Value,
    ) -> Result<Value, DesktopServiceError> {
        decode_empty(operation, payload)?;
        let state = self.terminal_state()?;
        encode(operation, state.workspaces.get())
    }
    pub(super) fn set_terminal_workspaces(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<SetTerminalWorkspacesPayload>(operation, payload)?;
        let workspace_state = self
            .workspace_leases
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?;
        if !workspace_state.removals.is_empty() {
            return Err(DesktopServiceError::new(
                "maintenance-busy",
                "A managed workspace is finishing safe cleanup. Try saving terminal workspaces again.",
                true,
            ));
        }
        let mut terminal_state = self.terminal_state()?;
        let previously_saved_ids = terminal_state.workspaces.is_available().then(|| {
            terminal_state
                .workspaces
                .saved_terminals()
                .map(|terminal| terminal.id)
                .collect::<BTreeSet<_>>()
        });
        let validated =
            validate_terminal_workspaces(&request.workspaces).map_err(map_settings_error)?;
        let moves = crate::moved_folder::verified_moves(&request.moved_folders)?;
        let prepared = terminal_state
            .workspaces
            .prepare_save(&validated, &moves)
            .map_err(map_settings_error)?;
        // A confirmed move rewrites the proofs before narrowing, so the
        // roster that now names the new folder keeps them. Like every
        // two-file commit here, a crash between the writes can only leave
        // a moved pane refused, never resumed somewhere its proof does not
        // name.
        Self::follow_moved_folders(&mut terminal_state, &moves)?;
        // Revoke stale host proof before making the renderer roster
        // authoritative. A crash between these writes can deny a reopen,
        // but can never resurrect authority for an ID the roster removed.
        self.narrow_terminal_reopen_authorities(
            &mut terminal_state,
            &prepared,
            previously_saved_ids.as_ref(),
        )?;
        let workspaces = terminal_state
            .workspaces
            .commit_prepared(&self.terminal_workspaces_file, &prepared)
            .map_err(map_settings_error)?;
        drop(workspace_state);
        encode(operation, workspaces)
    }
    pub(super) fn resolve_moved_terminal_folder(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<PathPayload>(operation, payload)?;
        let path = Path::new(&request.path);
        if !path.is_absolute() {
            return Err(DesktopServiceError::invalid_payload(operation));
        }
        let terminal_state = self.terminal_state()?;
        let saved = terminal_state
            .workspaces
            .saved_terminals()
            .any(|terminal| Path::new(&terminal.cwd) == path);
        let target = if saved {
            crate::moved_folder::moved_folder_target(path)
                .map(|target| display_path(&target))
                .transpose()?
        } else {
            None
        };
        encode(operation, target)
    }
}
