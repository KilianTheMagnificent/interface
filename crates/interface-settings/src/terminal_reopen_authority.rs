use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use interface_contracts::terminal::{
    TERMINAL_LAUNCH_REQUEST_VERSION, TERMINAL_LAUNCH_SESSION_VERSION, TerminalId, TerminalSummary,
    TerminalWorkspaceRecord, TerminalWorkspaceTerminal,
};
use interface_storage::{JsonLimits, read_json};
use serde::{Deserialize, Serialize};

use crate::{
    leased::write_document,
    private_path::{SettingsError, map_document_write, prepare_store},
    terminal_workspaces::validate_terminal_workspace_terminal,
};

const AUTHORITY_VERSION: u8 = 1;
const AUTHORITY_MAX_BYTES: usize = 512 * 1024;
const AUTHORITY_MAX_RECORDS: usize = 4_096;
const AUTHORITY_LOCK: &str = ".terminal-reopen-authority.lock";

/// Host-private proof that one terminal ID successfully spawned in an exact
/// authorized root. Renderer-authored saved workspaces can retain or narrow a
/// matching proof, but they cannot create one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalReopenAuthority {
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_worktree_key: Option<String>,
    pub terminal: TerminalWorkspaceTerminal,
}

/// Exact per-terminal contents of the private reopen-authority sidecar.
/// Pending migration is distinct from absence because a host transaction may
/// never consume or erase an unresolved startup candidate by accident.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalReopenAuthoritySlot {
    Absent,
    Authority(TerminalReopenAuthority),
    PendingMigration(TerminalWorkspaceTerminal),
}

impl From<Option<TerminalReopenAuthority>> for TerminalReopenAuthoritySlot {
    fn from(authority: Option<TerminalReopenAuthority>) -> Self {
        authority.map_or(Self::Absent, Self::Authority)
    }
}

/// Outcome of an exact per-terminal sidecar compare-and-exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalReopenAuthorityCompareExchange {
    Exchanged,
    AlreadyReplacement,
    Conflict(TerminalReopenAuthoritySlot),
}

impl TerminalReopenAuthority {
    #[must_use]
    pub fn from_summary(summary: &TerminalSummary) -> Self {
        Self {
            root: summary.root.clone(),
            managed_worktree_key: None,
            terminal: TerminalWorkspaceTerminal {
                id: summary.id,
                title: summary.title.clone(),
                cwd: summary.cwd.clone(),
                session_key: summary.session_key.clone(),
                launch: summary.launch,
                launch_session: summary.launch_session.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredTerminalReopenAuthorities {
    version: u8,
    authorities: Vec<TerminalReopenAuthority>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_migration: Vec<TerminalWorkspaceTerminal>,
}

/// Derives a separate private sidecar path without changing the accepted
/// renderer-owned terminal-workspaces document.
///
/// # Errors
///
/// Returns [`SettingsError::InvalidPath`] for a relative path or one without
/// a usable parent and filename.
pub fn terminal_reopen_authority_path(
    terminal_workspaces_file: &Path,
) -> Result<PathBuf, SettingsError> {
    if !terminal_workspaces_file.is_absolute() {
        return Err(SettingsError::InvalidPath);
    }
    let parent = terminal_workspaces_file
        .parent()
        .ok_or(SettingsError::InvalidPath)?;
    let name = terminal_workspaces_file
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or(SettingsError::InvalidPath)?;
    Ok(parent.join(format!(".{name}.host-authority")))
}

/// One-writer store for cold-reopen authority. It is deliberately not exposed
/// through any desktop wire operation.
pub struct TerminalReopenAuthorityStore {
    location: crate::private_path::StoreLocation,
    authorities: Vec<TerminalReopenAuthority>,
    pending_migration: Vec<TerminalWorkspaceTerminal>,
    initialized: bool,
}

impl TerminalReopenAuthorityStore {
    /// # Errors
    ///
    /// Refuses a relative, linked, hard-linked, or otherwise unsafe sidecar.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        Ok(Self {
            location: prepare_store(file)?,
            authorities: Vec::new(),
            pending_migration: Vec::new(),
            initialized: false,
        })
    }

    /// # Errors
    ///
    /// Refuses malformed, over-budget, unsupported, or unsafe storage.
    pub fn load(&mut self) -> Result<Vec<TerminalReopenAuthority>, SettingsError> {
        let limits = JsonLimits::new(AUTHORITY_MAX_BYTES, 16, 100_000)?;
        let Some(loaded) = read_json::<StoredTerminalReopenAuthorities>(
            &self.location.directory,
            &self.location.name,
            limits,
        )?
        else {
            self.authorities.clear();
            self.pending_migration.clear();
            self.initialized = false;
            return Ok(Vec::new());
        };
        if loaded.value().version != AUTHORITY_VERSION {
            return Err(invalid("authority version is unsupported"));
        }
        let authorities = validate_authorities(&loaded.value().authorities)?;
        let pending_migration =
            validate_pending_migration(&loaded.value().pending_migration, &authorities)?;
        self.authorities.clone_from(&authorities);
        self.pending_migration = pending_migration;
        self.initialized = true;
        Ok(authorities)
    }

    /// Whether a host has completed the one-time upgrade import. Absence is
    /// distinct from an initialized empty sidecar so legacy saved worktrees get
    /// one verified migration opportunity and renderer additions never do.
    #[must_use]
    pub const fn is_initialized(&self) -> bool {
        self.initialized
    }

    #[must_use]
    pub fn get(&self, terminal_id: TerminalId) -> Option<TerminalReopenAuthority> {
        self.authorities
            .iter()
            .find(|authority| authority.terminal.id == terminal_id)
            .cloned()
    }

    #[must_use]
    pub fn pending_migration(&self, terminal_id: TerminalId) -> Option<TerminalWorkspaceTerminal> {
        self.pending_migration
            .iter()
            .find(|terminal| terminal.id == terminal_id)
            .cloned()
    }

    #[must_use]
    pub fn slot(&self, terminal_id: TerminalId) -> TerminalReopenAuthoritySlot {
        if let Some(authority) = self.get(terminal_id) {
            TerminalReopenAuthoritySlot::Authority(authority)
        } else if let Some(pending) = self.pending_migration(terminal_id) {
            TerminalReopenAuthoritySlot::PendingMigration(pending)
        } else {
            TerminalReopenAuthoritySlot::Absent
        }
    }

    /// # Errors
    ///
    /// Atomically installs or advances one host-confirmed terminal proof.
    pub fn upsert(&mut self, authority: &TerminalReopenAuthority) -> Result<bool, SettingsError> {
        let mut authority = validate_authority(authority)?;
        let terminal_id = authority.terminal.id;
        let slot = self
            .authorities
            .iter()
            .position(|current| current.terminal.id == terminal_id);
        if let Some(index) = slot {
            let current = &self.authorities[index];
            if current.root == authority.root
                && terminal_identity_matches(&current.terminal, &authority.terminal)
                && authority.managed_worktree_key.is_none()
            {
                authority
                    .managed_worktree_key
                    .clone_from(&current.managed_worktree_key);
            }
            // The stored title is display metadata that no authority check
            // reads. Shells announce a new title about twice per command, and
            // this runs on the pane's PTY reader thread under the terminal
            // event gate, so a title-only change is not worth a durable write:
            // the stored title stays the one from the last real change.
            if equal_except_title(current, &authority) {
                return Ok(false);
            }
        } else if self.authorities.len() >= AUTHORITY_MAX_RECORDS {
            return Err(invalid("too many terminal reopen authorities"));
        }
        let mut authorities = self.authorities.clone();
        match slot {
            Some(index) => authorities[index] = authority,
            None => authorities.push(authority),
        }
        let mut pending_migration = self.pending_migration.clone();
        pending_migration.retain(|terminal| terminal.id != terminal_id);
        self.save(authorities, pending_migration)?;
        Ok(true)
    }

    /// # Errors
    ///
    /// Atomically removes only the named host proof.
    pub fn remove(&mut self, terminal_id: TerminalId) -> Result<bool, SettingsError> {
        let mut authorities = self.authorities.clone();
        let mut pending_migration = self.pending_migration.clone();
        let before = (authorities.len(), pending_migration.len());
        authorities.retain(|authority| authority.terminal.id != terminal_id);
        pending_migration.retain(|terminal| terminal.id != terminal_id);
        if (authorities.len(), pending_migration.len()) == before {
            return Ok(false);
        }
        self.save(authorities, pending_migration)?;
        Ok(true)
    }

    /// Replaces one exact per-terminal sidecar slot only when it still equals
    /// `expected`. Unlike [`Self::upsert`], this deliberately does not preserve
    /// fields from the current value. Equality with `replacement` is an
    /// idempotent success, while any third value is returned without writing.
    ///
    /// # Errors
    ///
    /// Refuses a slot for another terminal, invalid authority, excess records,
    /// or a sidecar write that cannot be committed safely.
    pub fn compare_exchange(
        &mut self,
        terminal_id: TerminalId,
        expected: &TerminalReopenAuthoritySlot,
        replacement: &TerminalReopenAuthoritySlot,
    ) -> Result<TerminalReopenAuthorityCompareExchange, SettingsError> {
        let expected = validate_slot(terminal_id, expected)?;
        let replacement = validate_slot(terminal_id, replacement)?;
        let current = self.slot(terminal_id);
        if current == replacement {
            return Ok(TerminalReopenAuthorityCompareExchange::AlreadyReplacement);
        }
        if current != expected {
            return Ok(TerminalReopenAuthorityCompareExchange::Conflict(current));
        }

        let mut authorities = self.authorities.clone();
        let mut pending_migration = self.pending_migration.clone();
        authorities.retain(|authority| authority.terminal.id != terminal_id);
        pending_migration.retain(|terminal| terminal.id != terminal_id);
        match replacement {
            TerminalReopenAuthoritySlot::Authority(authority) => {
                if authorities.len() >= AUTHORITY_MAX_RECORDS {
                    return Err(invalid("too many terminal reopen authorities"));
                }
                authorities.push(authority);
            }
            TerminalReopenAuthoritySlot::PendingMigration(terminal) => {
                if authorities.len().saturating_add(pending_migration.len())
                    >= AUTHORITY_MAX_RECORDS
                {
                    return Err(invalid("too many terminal reopen records"));
                }
                pending_migration.push(terminal);
            }
            TerminalReopenAuthoritySlot::Absent => {}
        }
        self.save(authorities, pending_migration)?;
        Ok(TerminalReopenAuthorityCompareExchange::Exchanged)
    }

    /// Captures the exact startup roster as untrusted one-time migration
    /// candidates. A candidate grants nothing until the host resolves it
    /// against a verified managed-worktree ledger record.
    ///
    /// # Errors
    ///
    /// Returns an error when any proof is invalid or the sidecar cannot be
    /// committed safely.
    pub fn initialize_pending_migration(
        &mut self,
        pending_migration: &[TerminalWorkspaceTerminal],
    ) -> Result<bool, SettingsError> {
        if self.initialized {
            return Ok(false);
        }
        self.save(Vec::new(), pending_migration.to_vec())?;
        Ok(true)
    }

    /// Resolves one startup migration candidate. `Some` installs a verified
    /// host proof; `None` records a proven non-managed/invalid candidate. An
    /// unavailable verifier must not call this method, leaving the candidate
    /// pending for a later retry.
    ///
    /// # Errors
    ///
    /// Refuses an authority that does not match the pending stable identity or
    /// a sidecar write that cannot be committed safely.
    pub fn resolve_pending_migration(
        &mut self,
        terminal_id: TerminalId,
        authority: Option<&TerminalReopenAuthority>,
    ) -> Result<bool, SettingsError> {
        let Some(pending) = self.pending_migration(terminal_id) else {
            return Ok(false);
        };
        let mut authorities = self.authorities.clone();
        if let Some(authority) = authority {
            let authority = validate_authority(authority)?;
            if !terminal_identity_matches(&pending, &authority.terminal) {
                return Err(invalid("migration authority did not match its candidate"));
            }
            authorities.retain(|current| current.terminal.id != terminal_id);
            authorities.push(authority);
        }
        let mut pending_migration = self.pending_migration.clone();
        pending_migration.retain(|terminal| terminal.id != terminal_id);
        self.save(authorities, pending_migration)?;
        Ok(true)
    }

    /// Narrows host proofs to stable terminal identities that the current durable
    /// renderer roster still retains. Provider session bindings are deliberately
    /// excluded from that identity: the host sidecar is authoritative for them and
    /// may be one atomic write ahead of the renderer-owned roster after recovery.
    /// This operation can remove authority but can never add or broaden it.
    ///
    /// # Errors
    ///
    /// Returns an error when the narrowed sidecar cannot be committed safely.
    pub fn narrow_to_saved(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
    ) -> Result<bool, SettingsError> {
        let saved = workspaces
            .iter()
            .flat_map(|workspace| workspace.terminals.iter())
            .collect::<Vec<_>>();
        let mut authorities = self.authorities.clone();
        authorities.retain(|authority| {
            saved
                .iter()
                .any(|terminal| terminal_identity_matches(&authority.terminal, terminal))
        });
        let mut pending_migration = self.pending_migration.clone();
        pending_migration.retain(|pending| {
            saved
                .iter()
                .any(|terminal| terminal_identity_matches(pending, terminal))
        });
        if authorities == self.authorities && pending_migration == self.pending_migration {
            return Ok(false);
        }
        self.save(authorities, pending_migration)?;
        Ok(true)
    }

    /// Narrows host proofs to stable terminal identities that the current durable
    /// renderer roster still retains, while preserving proofs for live host-spawned
    /// terminals (`!previously_saved_ids.contains(&id)`) that have not yet been
    /// filed in any renderer roster snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the narrowed sidecar cannot be committed safely.
    pub fn narrow_to_saved_preserving_unsaved(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
        previously_saved_ids: &BTreeSet<TerminalId>,
    ) -> Result<bool, SettingsError> {
        let saved = workspaces
            .iter()
            .flat_map(|workspace| workspace.terminals.iter())
            .collect::<Vec<_>>();
        let mut authorities = self.authorities.clone();
        authorities.retain(|authority| {
            let id = authority.terminal.id;
            if saved.iter().any(|terminal| terminal.id == id) {
                saved
                    .iter()
                    .any(|terminal| terminal_identity_matches(&authority.terminal, terminal))
            } else {
                !previously_saved_ids.contains(&id)
            }
        });
        let mut pending_migration = self.pending_migration.clone();
        pending_migration.retain(|pending| {
            saved
                .iter()
                .any(|terminal| terminal_identity_matches(pending, terminal))
        });
        if authorities == self.authorities && pending_migration == self.pending_migration {
            return Ok(false);
        }
        self.save(authorities, pending_migration)?;
        Ok(true)
    }

    /// Carries proofs recorded under `from` to the same place under `to`, for a
    /// folder that moved and left a link behind. The caller has already shown
    /// that `from` now resolves to the real directory `to` and that the owner
    /// confirmed that exact target; this store only rewrites paths. Managed
    /// worktree proofs stay put: their identity belongs to the worktree ledger.
    ///
    /// # Errors
    ///
    /// Returns an error when the rewritten sidecar cannot be committed safely.
    pub fn follow_moved_folder(&mut self, from: &Path, to: &Path) -> Result<bool, SettingsError> {
        let mut authorities = self.authorities.clone();
        for authority in &mut authorities {
            if authority.managed_worktree_key.is_some() {
                continue;
            }
            let (Some(root), Some(cwd)) = (
                moved_path(&authority.root, from, to),
                moved_path(&authority.terminal.cwd, from, to),
            ) else {
                continue;
            };
            authority.root = root;
            authority.terminal.cwd = cwd;
        }
        let mut pending_migration = self.pending_migration.clone();
        for pending in &mut pending_migration {
            if let Some(cwd) = moved_path(&pending.cwd, from, to) {
                pending.cwd = cwd;
            }
        }
        if authorities == self.authorities && pending_migration == self.pending_migration {
            return Ok(false);
        }
        self.save(authorities, pending_migration)?;
        Ok(true)
    }

    #[allow(clippy::needless_pass_by_value)] // Validation normalizes both owned documents before commit.
    fn save(
        &mut self,
        authorities: Vec<TerminalReopenAuthority>,
        pending_migration: Vec<TerminalWorkspaceTerminal>,
    ) -> Result<(), SettingsError> {
        let authorities = validate_authorities(&authorities)?;
        let pending_migration = validate_pending_migration(&pending_migration, &authorities)?;
        let document = StoredTerminalReopenAuthorities {
            version: AUTHORITY_VERSION,
            authorities: authorities.clone(),
            pending_migration: pending_migration.clone(),
        };
        write_document(
            &self.location,
            AUTHORITY_LOCK,
            &document,
            JsonLimits::new(AUTHORITY_MAX_BYTES, 16, 100_000)?,
            |error| map_document_write(error, SettingsError::InvalidTerminalWorkspace),
        )?;
        self.authorities = authorities;
        self.pending_migration = pending_migration;
        self.initialized = true;
        Ok(())
    }
}

fn validate_pending_migration(
    pending: &[TerminalWorkspaceTerminal],
    authorities: &[TerminalReopenAuthority],
) -> Result<Vec<TerminalWorkspaceTerminal>, SettingsError> {
    if pending.len().saturating_add(authorities.len()) > AUTHORITY_MAX_RECORDS {
        return Err(invalid("too many terminal reopen records"));
    }
    let mut ids = authorities
        .iter()
        .map(|authority| authority.terminal.id)
        .collect::<BTreeSet<_>>();
    pending
        .iter()
        .map(|terminal| {
            let terminal = validate_terminal_workspace_terminal(terminal)?;
            if !ids.insert(terminal.id) {
                return Err(invalid("duplicate terminal reopen record"));
            }
            Ok(terminal)
        })
        .collect()
}

fn validate_authorities(
    authorities: &[TerminalReopenAuthority],
) -> Result<Vec<TerminalReopenAuthority>, SettingsError> {
    if authorities.len() > AUTHORITY_MAX_RECORDS {
        return Err(invalid("too many terminal reopen authorities"));
    }
    let mut ids = BTreeSet::new();
    authorities
        .iter()
        .map(|authority| {
            let authority = validate_authority(authority)?;
            if !ids.insert(authority.terminal.id) {
                return Err(invalid("duplicate terminal reopen authority"));
            }
            Ok(authority)
        })
        .collect()
}

fn validate_authority(
    authority: &TerminalReopenAuthority,
) -> Result<TerminalReopenAuthority, SettingsError> {
    let terminal = validate_terminal_workspace_terminal(&authority.terminal)?;
    let root = authority.root.trim();
    let managed_worktree_key = authority
        .managed_worktree_key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let invalid_managed_worktree_key = authority.managed_worktree_key.is_some()
        && managed_worktree_key
            .is_none_or(|key| key.len() > 1_024 || key.chars().any(char::is_control));
    if root.is_empty()
        || root.len() > 4_096
        || root.chars().any(char::is_control)
        || !Path::new(root).is_absolute()
        || !Path::new(&terminal.cwd).starts_with(root)
        || invalid_managed_worktree_key
        || terminal
            .launch
            .is_some_and(|launch| launch.version != TERMINAL_LAUNCH_REQUEST_VERSION)
        || terminal.launch_session.as_ref().is_some_and(|session| {
            session.version != TERMINAL_LAUNCH_SESSION_VERSION
                || session.id.as_deref().is_some_and(str::is_empty)
        })
    {
        return Err(invalid("terminal reopen authority failed validation"));
    }
    Ok(TerminalReopenAuthority {
        root: root.to_owned(),
        managed_worktree_key: managed_worktree_key.map(ToOwned::to_owned),
        terminal,
    })
}

fn validate_slot(
    terminal_id: TerminalId,
    slot: &TerminalReopenAuthoritySlot,
) -> Result<TerminalReopenAuthoritySlot, SettingsError> {
    let slot = match slot {
        TerminalReopenAuthoritySlot::Absent => TerminalReopenAuthoritySlot::Absent,
        TerminalReopenAuthoritySlot::Authority(authority) => {
            TerminalReopenAuthoritySlot::Authority(validate_authority(authority)?)
        }
        TerminalReopenAuthoritySlot::PendingMigration(terminal) => {
            TerminalReopenAuthoritySlot::PendingMigration(validate_terminal_workspace_terminal(
                terminal,
            )?)
        }
    };
    let names_terminal = match &slot {
        TerminalReopenAuthoritySlot::Absent => true,
        TerminalReopenAuthoritySlot::Authority(authority) => authority.terminal.id == terminal_id,
        TerminalReopenAuthoritySlot::PendingMigration(terminal) => terminal.id == terminal_id,
    };
    if !names_terminal {
        return Err(invalid(
            "terminal reopen authority slot names another terminal",
        ));
    }
    Ok(slot)
}

fn invalid(message: impl Into<String>) -> SettingsError {
    SettingsError::InvalidTerminalWorkspace(message.into())
}

/// Every authority field except the display title. Exhaustive destructuring
/// makes a new field a compile error here rather than a silently skipped write.
fn equal_except_title(left: &TerminalReopenAuthority, right: &TerminalReopenAuthority) -> bool {
    let TerminalReopenAuthority {
        root,
        managed_worktree_key,
        terminal:
            TerminalWorkspaceTerminal {
                id,
                title: _,
                cwd,
                session_key,
                launch,
                launch_session,
            },
    } = right;
    left.root == *root
        && left.managed_worktree_key == *managed_worktree_key
        && left.terminal.id == *id
        && left.terminal.cwd == *cwd
        && left.terminal.session_key == *session_key
        && left.terminal.launch == *launch
        && left.terminal.launch_session == *launch_session
}

#[must_use]
pub fn terminal_identity_matches(
    left: &TerminalWorkspaceTerminal,
    right: &TerminalWorkspaceTerminal,
) -> bool {
    left.id == right.id
        && left.cwd == right.cwd
        && left.session_key == right.session_key
        && left.launch == right.launch
}

/// `path` rewritten from under `from` to the same place under `to`, or `None`
/// when it is not inside `from`. Components are compared whole, so a sibling
/// such as `from-archive` never matches.
pub(crate) fn moved_path(path: &str, from: &Path, to: &Path) -> Option<String> {
    let rest = Path::new(path).strip_prefix(from).ok()?;
    let moved = if rest.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rest)
    };
    moved.to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use interface_contracts::{
        engine::SessionKey,
        terminal::{
            TERMINAL_LAUNCH_REQUEST_VERSION, TERMINAL_LAUNCH_SESSION_VERSION,
            TerminalLaunchProfileId, TerminalLaunchRequestV1, TerminalLaunchSessionV1,
        },
    };
    use tempfile::TempDir;

    use super::*;

    fn terminal(id: TerminalId, cwd: &Path) -> TerminalWorkspaceTerminal {
        TerminalWorkspaceTerminal {
            id,
            title: "Claude".to_owned(),
            cwd: cwd.to_string_lossy().into_owned(),
            session_key: Some(SessionKey::from("claude#saved")),
            launch: Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            launch_session: Some(TerminalLaunchSessionV1 {
                version: TERMINAL_LAUNCH_SESSION_VERSION,
                id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
                resume: true,
            }),
        }
    }

    fn workspace(terminal: TerminalWorkspaceTerminal) -> TerminalWorkspaceRecord {
        TerminalWorkspaceRecord {
            id: "saved".to_owned(),
            name: "Saved".to_owned(),
            created_at: "2026-09-02T00:00:00Z".to_owned(),
            updated_at: "2026-09-02T00:00:00Z".to_owned(),
            terminals: vec![terminal],
            layout: None,
        }
    }

    fn authority(
        terminal: TerminalWorkspaceTerminal,
        root: &Path,
        key: &str,
    ) -> TerminalReopenAuthority {
        TerminalReopenAuthority {
            root: root.to_string_lossy().into_owned(),
            managed_worktree_key: Some(key.to_owned()),
            terminal,
        }
    }

    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("worktree");
        let cwd = root.join("src");
        std::fs::create_dir_all(&cwd).unwrap();
        let file = temporary.path().join(".terminal-workspaces.host-authority");
        let mut store = TerminalReopenAuthorityStore::open(&file).unwrap();
        store.load().unwrap();
        let id: TerminalId =
            serde_json::from_str("\"44444444-4444-4444-8444-444444444444\"").unwrap();
        let original = authority(terminal(id, &cwd), &root, "claude#saved");
        assert!(store.upsert(&original).unwrap());
        crate::golden::assert_golden(
            "terminal-reopen-authority.saved.json",
            temporary.path(),
            &file,
        );
        drop(store);
        let mut reopened = TerminalReopenAuthorityStore::open(&file).unwrap();
        assert_eq!(reopened.load().unwrap(), vec![original]);
    }

    #[test]
    fn upsert_skips_the_durable_write_for_a_title_only_change() {
        use std::os::unix::fs::MetadataExt;

        let temporary = TempDir::new().expect("temporary authority store");
        let root = temporary.path().join("worktree");
        let cwd = root.join("src");
        std::fs::create_dir_all(&cwd).expect("worktree cwd");
        let file = temporary.path().join(".terminal-workspaces.host-authority");
        let mut store = TerminalReopenAuthorityStore::open(&file).expect("authority store");
        store.load().expect("empty authority store");
        let id = TerminalId::random();
        let original = authority(terminal(id, &cwd), &root, "claude#saved");
        assert!(store.upsert(&original).expect("record host authority"));
        let inode = |path: &Path| std::fs::metadata(path).expect("sidecar").ino();
        let written = inode(&file);

        let mut retitled = original.clone();
        retitled.terminal.title = "~/worktree/src — make test".to_owned();
        assert!(!store.upsert(&retitled).expect("title-only upsert"));
        assert_eq!(
            inode(&file),
            written,
            "a title-only change rewrote the sidecar"
        );
        assert_eq!(store.get(id), Some(original.clone()));

        let mut advanced = retitled;
        advanced.terminal.launch_session = Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some("22222222-2222-4222-8222-222222222222".to_owned()),
            resume: true,
        });
        assert!(store.upsert(&advanced).expect("launch-session upsert"));
        assert_ne!(inode(&file), written, "a real change was not saved");
        assert_eq!(store.get(id), Some(advanced));
    }

    #[test]
    fn provider_binding_can_advance_without_losing_stable_reopen_authority() {
        let temporary = TempDir::new().expect("temporary authority store");
        let root = temporary.path().join("worktree");
        let cwd = root.join("src");
        std::fs::create_dir_all(&cwd).expect("worktree cwd");
        let id = TerminalId::random();
        let original = terminal(id, &cwd);
        let mut store = TerminalReopenAuthorityStore::open(
            &temporary.path().join(".terminal-workspaces.host-authority"),
        )
        .expect("authority store");
        store.load().expect("empty authority store");
        store
            .upsert(&TerminalReopenAuthority {
                root: root.to_string_lossy().into_owned(),
                managed_worktree_key: Some("claude#saved".to_owned()),
                terminal: original.clone(),
            })
            .expect("record host authority");

        let mut advanced = original.clone();
        advanced.launch_session = Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some("22222222-2222-4222-8222-222222222222".to_owned()),
            resume: true,
        });
        assert!(terminal_identity_matches(&original, &advanced));
        assert!(
            !store
                .narrow_to_saved(&[workspace(advanced)])
                .expect("keep stable authority")
        );
        assert!(store.get(id).is_some());
    }

    #[test]
    fn renderer_cannot_rebind_stable_reopen_authority() {
        let temporary = TempDir::new().expect("temporary authority store");
        let root = temporary.path().join("worktree");
        let cwd = root.join("src");
        std::fs::create_dir_all(&cwd).expect("worktree cwd");
        let id = TerminalId::random();
        let original = terminal(id, &cwd);
        let mut store = TerminalReopenAuthorityStore::open(
            &temporary.path().join(".terminal-workspaces.host-authority"),
        )
        .expect("authority store");
        store.load().expect("empty authority store");
        store
            .upsert(&TerminalReopenAuthority {
                root: root.to_string_lossy().into_owned(),
                managed_worktree_key: Some("claude#saved".to_owned()),
                terminal: original.clone(),
            })
            .expect("record host authority");

        let mut forged = original;
        forged.cwd = root.join("other").to_string_lossy().into_owned();
        assert!(
            store
                .narrow_to_saved(&[workspace(forged)])
                .expect("narrow forged identity")
        );
        assert!(store.get(id).is_none());
    }

    #[test]
    fn a_confirmed_moved_folder_carries_its_proofs_to_the_new_path() {
        let temporary = TempDir::new().expect("temporary authority store");
        let from = temporary.path().join("Projects/Interface-Rust");
        let to = temporary.path().join("Developer/Interface-Rust");
        let sibling = temporary.path().join("Projects/Interface-Rust-archive");
        let file = temporary.path().join(".terminal-workspaces.host-authority");
        let mut store = TerminalReopenAuthorityStore::open(&file).expect("authority store");
        store.load().expect("empty authority store");
        let plain = |terminal, root: &Path| TerminalReopenAuthority {
            root: root.to_string_lossy().into_owned(),
            managed_worktree_key: None,
            terminal,
        };
        let at_root = TerminalId::random();
        let nested = TerminalId::random();
        let unrelated = TerminalId::random();
        let managed = TerminalId::random();
        let pending = TerminalId::random();
        store
            .upsert(&plain(terminal(at_root, &from), &from))
            .expect("root proof");
        store
            .upsert(&plain(terminal(nested, &from.join("src")), &from))
            .expect("nested proof");
        store
            .upsert(&plain(terminal(unrelated, &sibling), &sibling))
            .expect("sibling proof");
        store
            .upsert(&authority(
                terminal(managed, &from),
                &from,
                "claude#managed",
            ))
            .expect("managed proof");
        store
            .compare_exchange(
                pending,
                &TerminalReopenAuthoritySlot::Absent,
                &TerminalReopenAuthoritySlot::PendingMigration(terminal(pending, &from)),
            )
            .expect("pending candidate");

        assert!(store.follow_moved_folder(&from, &to).expect("follow move"));

        let moved = store.get(at_root).expect("root proof kept");
        assert_eq!(Path::new(&moved.root), to);
        assert_eq!(Path::new(&moved.terminal.cwd), to);
        let moved = store.get(nested).expect("nested proof kept");
        assert_eq!(Path::new(&moved.root), to);
        assert_eq!(Path::new(&moved.terminal.cwd), to.join("src"));
        assert_eq!(
            Path::new(&store.get(unrelated).expect("sibling kept").root),
            sibling
        );
        // A managed worktree's identity belongs to its ledger, not to a link.
        assert_eq!(
            Path::new(&store.get(managed).expect("managed kept").root),
            from
        );
        assert_eq!(
            store
                .pending_migration(pending)
                .map(|terminal| terminal.cwd),
            Some(to.to_string_lossy().into_owned())
        );
        drop(store);

        let mut reopened = TerminalReopenAuthorityStore::open(&file).expect("reopen authority");
        reopened.load().expect("load moved proofs");
        assert_eq!(
            reopened.get(at_root).map(|proof| proof.root),
            Some(to.to_string_lossy().into_owned())
        );
        assert!(
            !reopened
                .follow_moved_folder(&from, &to)
                .expect("second follow is a no-op")
        );
    }

    #[test]
    fn compare_exchange_preserves_a_third_authority_value() {
        let temporary = TempDir::new().expect("temporary authority store");
        let root = temporary.path().join("worktree");
        std::fs::create_dir_all(&root).expect("worktree root");
        let id = TerminalId::random();
        let prior = authority(terminal(id, &root), &root, "claude#prior");
        let mut attempted = prior.clone();
        attempted.managed_worktree_key = Some("claude#attempted".to_owned());
        let mut winner = prior.clone();
        winner.managed_worktree_key = Some("claude#winner".to_owned());
        let file = temporary.path().join(".terminal-workspaces.host-authority");
        let mut store = TerminalReopenAuthorityStore::open(&file).expect("authority store");
        store.load().expect("empty authority store");
        store.upsert(&winner).expect("install winner");

        assert_eq!(
            store
                .compare_exchange(
                    id,
                    &TerminalReopenAuthoritySlot::Authority(attempted),
                    &TerminalReopenAuthoritySlot::Authority(prior),
                )
                .expect("conflict is a typed outcome"),
            TerminalReopenAuthorityCompareExchange::Conflict(
                TerminalReopenAuthoritySlot::Authority(winner.clone())
            )
        );
        drop(store);

        let mut reopened = TerminalReopenAuthorityStore::open(&file).expect("reopen authority");
        reopened.load().expect("load preserved winner");
        assert_eq!(
            reopened.slot(id),
            TerminalReopenAuthoritySlot::Authority(winner)
        );
    }

    #[test]
    fn compare_exchange_preserves_a_third_pending_migration_value() {
        let temporary = TempDir::new().expect("temporary authority store");
        let root = temporary.path().join("worktree");
        std::fs::create_dir_all(&root).expect("worktree root");
        let id = TerminalId::random();
        let pending = terminal(id, &root);
        let attempted = authority(pending.clone(), &root, "claude#attempted");
        let file = temporary.path().join(".terminal-workspaces.host-authority");
        let mut store = TerminalReopenAuthorityStore::open(&file).expect("authority store");
        store.load().expect("empty authority store");
        store
            .initialize_pending_migration(std::slice::from_ref(&pending))
            .expect("install pending migration");

        assert_eq!(
            store
                .compare_exchange(
                    id,
                    &TerminalReopenAuthoritySlot::Authority(attempted),
                    &TerminalReopenAuthoritySlot::Absent,
                )
                .expect("conflict is a typed outcome"),
            TerminalReopenAuthorityCompareExchange::Conflict(
                TerminalReopenAuthoritySlot::PendingMigration(pending.clone())
            )
        );
        drop(store);

        let mut reopened = TerminalReopenAuthorityStore::open(&file).expect("reopen authority");
        reopened.load().expect("load preserved migration");
        assert_eq!(
            reopened.slot(id),
            TerminalReopenAuthoritySlot::PendingMigration(pending)
        );
    }
}
