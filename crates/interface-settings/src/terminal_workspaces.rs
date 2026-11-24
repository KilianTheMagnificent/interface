use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use interface_contracts::terminal::{
    TERMINAL_LAUNCH_SESSION_VERSION, TERMINAL_WORKSPACE_RECORD_VERSION, TerminalId,
    TerminalLaunchProfileId, TerminalLaunchRequestV1, TerminalLaunchSessionV1,
    TerminalWorkspaceRecord, TerminalWorkspaceTerminal,
};
use interface_storage::{JsonLimits, read_json};
use serde::{Deserialize, Serialize};

use crate::leased::write_document;
use crate::private_path::{SettingsError, map_document_write, prepare_store};
use crate::terminal_reopen_authority::moved_path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalWorkspaceLimits {
    pub json_bytes: usize,
    pub workspaces: usize,
    pub terminals_per_workspace: usize,
    pub name_characters: usize,
    pub layout_characters: usize,
}

pub const TERMINAL_WORKSPACE_LIMITS: TerminalWorkspaceLimits = TerminalWorkspaceLimits {
    json_bytes: 512 * 1024,
    workspaces: 64,
    // The renderer caps a layout at 6 panes of 32 tabs; this is the store's
    // own ceiling, not a second opinion about the pane tree.
    terminals_per_workspace: 64,
    name_characters: 80,
    layout_characters: 64 * 1024,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredTerminalWorkspaces {
    version: u8,
    workspaces: Vec<TerminalWorkspaceRecord>,
}

fn invalid(message: impl Into<String>) -> SettingsError {
    SettingsError::InvalidTerminalWorkspace(message.into())
}

fn provider_session_is_confirmed(launch_session: &TerminalLaunchSessionV1) -> bool {
    launch_session.version == TERMINAL_LAUNCH_SESSION_VERSION
        && launch_session.resume
        && launch_session
            .id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
}

fn bounded_text(value: &str, characters: usize, field: &str) -> Result<String, SettingsError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{field} must not be empty")));
    }
    if trimmed.chars().count() > characters {
        return Err(invalid(format!(
            "{field} must be at most {characters} characters"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(trimmed.to_owned())
}

/// Validate and normalize one saved terminal.
///
/// # Errors
///
/// Refuses empty/oversized/control-bearing text and a relative working
/// directory. The launch binding itself is re-validated at spawn time by the
/// terminal service, which owns argv.
pub fn validate_terminal_workspace_terminal(
    terminal: &TerminalWorkspaceTerminal,
) -> Result<TerminalWorkspaceTerminal, SettingsError> {
    if !Path::new(&terminal.cwd).is_absolute() {
        return Err(invalid("terminal.cwd must be an absolute path"));
    }
    Ok(TerminalWorkspaceTerminal {
        title: bounded_text(&terminal.title, 120, "terminal.title")?,
        cwd: bounded_text(&terminal.cwd, 4_096, "terminal.cwd")?,
        ..terminal.clone()
    })
}

/// Validate and normalize one workspace.
///
/// # Errors
///
/// Refuses invalid text, a duplicate terminal id, an over-budget terminal
/// count, or an over-budget layout blob.
pub fn validate_terminal_workspace(
    workspace: &TerminalWorkspaceRecord,
) -> Result<TerminalWorkspaceRecord, SettingsError> {
    if workspace.terminals.len() > TERMINAL_WORKSPACE_LIMITS.terminals_per_workspace {
        return Err(invalid(format!(
            "a workspace may hold at most {} terminals",
            TERMINAL_WORKSPACE_LIMITS.terminals_per_workspace
        )));
    }
    if let Some(layout) = workspace.layout.as_deref() {
        if layout.chars().count() > TERMINAL_WORKSPACE_LIMITS.layout_characters {
            return Err(invalid("workspace.layout exceeds its size budget"));
        }
    }
    let mut ids = BTreeSet::new();
    let mut terminals = Vec::with_capacity(workspace.terminals.len());
    for terminal in &workspace.terminals {
        if !ids.insert(terminal.id) {
            return Err(invalid("a workspace must not list one terminal twice"));
        }
        terminals.push(validate_terminal_workspace_terminal(terminal)?);
    }
    Ok(TerminalWorkspaceRecord {
        id: bounded_text(&workspace.id, 128, "workspace.id")?,
        name: bounded_text(
            &workspace.name,
            TERMINAL_WORKSPACE_LIMITS.name_characters,
            "workspace.name",
        )?,
        created_at: bounded_text(&workspace.created_at, 64, "workspace.createdAt")?,
        updated_at: bounded_text(&workspace.updated_at, 64, "workspace.updatedAt")?,
        terminals,
        layout: workspace.layout.clone(),
    })
}

/// Validate the complete roster.
///
/// # Errors
///
/// Refuses a duplicate workspace id, a terminal claimed by two workspaces, or
/// more workspaces than the limit allows.
pub fn validate_terminal_workspaces(
    workspaces: &[TerminalWorkspaceRecord],
) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
    if workspaces.len() > TERMINAL_WORKSPACE_LIMITS.workspaces {
        return Err(invalid(format!(
            "at most {} saved terminal workspaces are supported",
            TERMINAL_WORKSPACE_LIMITS.workspaces
        )));
    }
    let mut ids = BTreeSet::new();
    let mut claimed = BTreeSet::new();
    let mut validated = Vec::with_capacity(workspaces.len());
    for workspace in workspaces {
        let workspace = validate_terminal_workspace(workspace)?;
        if !ids.insert(workspace.id.clone()) {
            return Err(invalid("two saved workspaces share one id"));
        }
        // A terminal belongs to exactly one workspace. Without this, reopening
        // two workspaces would race to recreate the same terminal id.
        for terminal in &workspace.terminals {
            if !claimed.insert(terminal.id) {
                return Err(invalid("one terminal is claimed by two workspaces"));
            }
        }
        validated.push(workspace);
    }
    Ok(validated)
}

fn default_workspace_name(name: &str) -> bool {
    name.to_ascii_lowercase()
        .strip_prefix("workspace ")
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

fn useful_workspace_title(terminal: &TerminalWorkspaceTerminal) -> bool {
    let title = terminal.title.trim();
    let lower = title.to_lowercase();
    let base = lower
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end();
    !title.is_empty()
        && !title.contains(" | ")
        && !title.starts_with('[')
        && ![
            "claude",
            "claude code",
            "codex",
            "grok",
            "qwen",
            "gemini",
            "antigravity",
            "opencode",
            "omp",
            "muse",
            "muse code",
            "kimi",
            "kimi code",
            "terminal",
            "shell",
            "projects",
            "claude · resume",
        ]
        .contains(&base)
        && ![
            "ready",
            "starting",
            "working",
            "thinking",
            "waiting",
            "action required",
            "fast on",
            "fast off",
        ]
        .contains(&lower.as_str())
        && Path::new(&terminal.cwd)
            .file_name()
            .and_then(|s| s.to_str())
            .is_none_or(|name| !title.eq_ignore_ascii_case(name))
        && !(title.len() == 36 && title.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}

/// Pick a useful initial task once; later pane focus, activity and renames do
/// not change workspace identity. The roster's existing name field is durable.
fn retain_workspace_names(
    workspaces: &mut [TerminalWorkspaceRecord],
    previous: &[TerminalWorkspaceRecord],
) {
    for workspace in workspaces {
        if !default_workspace_name(&workspace.name) {
            continue;
        }
        if let Some(saved) = previous
            .iter()
            .find(|saved| saved.id == workspace.id && !default_workspace_name(&saved.name))
        {
            workspace.name.clone_from(&saved.name);
        } else if let Some(terminal) = workspace
            .terminals
            .iter()
            .find(|t| useful_workspace_title(t))
        {
            workspace.name = terminal.title.chars().take(80).collect();
        }
    }
}

/// Owner-private version-1 roster of saved terminal workspaces.
pub struct TerminalWorkspaceStore {
    location: crate::private_path::StoreLocation,
    workspaces: Vec<TerminalWorkspaceRecord>,
    /// True only after a successful load or save, so an identical save may
    /// skip the write. A fresh open or a failed load/save never skips.
    mirrors_disk: bool,
}

impl TerminalWorkspaceStore {
    /// # Errors
    ///
    /// Refuses relative, linked, hard-linked, non-regular, or unstable storage.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        Ok(Self {
            location: prepare_store(file)?,
            workspaces: Vec::new(),
            mirrors_disk: false,
        })
    }

    #[must_use]
    pub fn get(&self) -> Vec<TerminalWorkspaceRecord> {
        self.workspaces.clone()
    }

    /// Borrows the last loaded or saved roster without cloning it.
    #[must_use]
    pub fn workspaces(&self) -> &[TerminalWorkspaceRecord] {
        &self.workspaces
    }

    /// # Errors
    ///
    /// Returns an error for invalid or over-budget JSON, or unsafe storage; the
    /// last in-memory roster remains unchanged on failure.
    pub fn load(&mut self) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        self.mirrors_disk = false;
        let limits = JsonLimits::new(TERMINAL_WORKSPACE_LIMITS.json_bytes, 16, 100_000)?;
        let Some(loaded) = read_json::<StoredTerminalWorkspaces>(
            &self.location.directory,
            &self.location.name,
            limits,
        )?
        else {
            self.workspaces.clear();
            self.mirrors_disk = true;
            return Ok(Vec::new());
        };
        if loaded.value().version != TERMINAL_WORKSPACE_RECORD_VERSION {
            return Err(invalid(format!(
                "document.version must be {TERMINAL_WORKSPACE_RECORD_VERSION}"
            )));
        }
        let validated = validate_terminal_workspaces(&loaded.value().workspaces)?;
        self.workspaces.clone_from(&validated);
        self.mirrors_disk = true;
        Ok(validated)
    }

    /// A roster identical to the last loaded or saved one is not rewritten,
    /// so an unchanged save no longer re-creates a file deleted or edited
    /// behind this store's back.
    ///
    /// # Errors
    ///
    /// Validates the complete snapshot before touching disk, then refuses
    /// drift, concurrent writers, unsafe targets, and uncertain durability.
    pub fn save(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        let mut workspaces = validate_terminal_workspaces(workspaces)?;
        retain_workspace_names(&mut workspaces, &self.workspaces);
        if self.mirrors_disk && workspaces == self.workspaces {
            return Ok(workspaces);
        }
        self.mirrors_disk = false;
        let document = StoredTerminalWorkspaces {
            version: TERMINAL_WORKSPACE_RECORD_VERSION,
            workspaces: workspaces.clone(),
        };
        write_document(
            &self.location,
            ".terminal-workspaces.lock",
            &document,
            JsonLimits::new(TERMINAL_WORKSPACE_LIMITS.json_bytes, 16, 100_000)?,
            |error| map_document_write(error, SettingsError::InvalidTerminalWorkspace),
        )?;
        self.workspaces.clone_from(&workspaces);
        self.mirrors_disk = true;
        Ok(workspaces)
    }

    /// Reconciles a renderer-authored full roster without allowing that
    /// snapshot to roll back provider reopen identity already filed by the
    /// host.
    ///
    /// Matching terminal IDs retain their current confirmed binding even when
    /// the incoming snapshot is id-less or names a different session. Omitting
    /// a terminal still removes it: this protects one canonical field, not
    /// membership. Only [`Self::reconcile_terminal_provider_reopen`] may move
    /// a saved terminal to a different provider-confirmed binding.
    ///
    /// # Errors
    ///
    /// Returns an error when the merged roster cannot be validated or committed
    /// safely.
    pub fn preserve_provider_reopen_identities(
        &self,
        workspaces: &[TerminalWorkspaceRecord],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        self.preserve_provider_reopen_identities_following(workspaces, &[])
    }

    /// [`Self::preserve_provider_reopen_identities`] for a save that carries
    /// host-verified folder moves: a preserved cwd inside a moved folder is
    /// kept at the same place under its new home instead of the old path.
    ///
    /// # Errors
    ///
    /// Returns an error when the merged roster cannot be validated.
    pub fn preserve_provider_reopen_identities_following(
        &self,
        workspaces: &[TerminalWorkspaceRecord],
        moved_folders: &[(PathBuf, PathBuf)],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        let current = self
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.terminals.iter())
            .map(|terminal| (terminal.id, terminal.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut merged = validate_terminal_workspaces(workspaces)?;
        for terminal in merged
            .iter_mut()
            .flat_map(|workspace| workspace.terminals.iter_mut())
        {
            let Some(current) = current.get(&terminal.id) else {
                continue;
            };
            let current_confirmed = current
                .launch_session
                .as_ref()
                .is_some_and(provider_session_is_confirmed);
            let completed_manual_provider = current
                .launch
                .is_none_or(|launch| launch.profile_id == TerminalLaunchProfileId::Shell)
                && current.launch_session.is_none()
                && terminal.launch.is_some_and(|launch| {
                    matches!(
                        launch.profile_id,
                        TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Codex
                    )
                })
                && terminal
                    .launch_session
                    .as_ref()
                    .is_some_and(provider_session_is_confirmed);
            if current_confirmed || completed_manual_provider {
                terminal.cwd = moved_folders
                    .iter()
                    .find_map(|(from, to)| moved_path(&current.cwd, from, to))
                    .unwrap_or_else(|| current.cwd.clone());
                terminal.launch = current.launch;
                terminal.launch_session.clone_from(&current.launch_session);
            }
        }
        validate_terminal_workspaces(&merged)
    }

    /// Saves the roster returned by [`Self::preserve_provider_reopen_identities`].
    /// Omitting a terminal still removes it; only matching live IDs retain the
    /// newer provider-owned tuple.
    ///
    /// # Errors
    ///
    /// Returns an error when the merged roster cannot be validated or committed
    /// safely.
    pub fn save_preserving_confirmed_provider_sessions(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        self.save_preserving_confirmed_provider_sessions_following(workspaces, &[])
    }

    /// [`Self::save_preserving_confirmed_provider_sessions`] with host-verified
    /// folder moves; see [`Self::preserve_provider_reopen_identities_following`].
    ///
    /// # Errors
    ///
    /// Returns an error when the merged roster cannot be validated or committed
    /// safely.
    pub fn save_preserving_confirmed_provider_sessions_following(
        &mut self,
        workspaces: &[TerminalWorkspaceRecord],
        moved_folders: &[(PathBuf, PathBuf)],
    ) -> Result<Vec<TerminalWorkspaceRecord>, SettingsError> {
        let merged =
            self.preserve_provider_reopen_identities_following(workspaces, moved_folders)?;
        self.save(&merged)
    }

    /// Durably replace the complete provider-owned reopen identity for one
    /// already-saved terminal. Promotion writes the provider launch,
    /// corroborated session, and cwd together; demotion clears the binding
    /// while restoring the shell launch. No renderer-authored caller reaches
    /// this narrow seam.
    ///
    /// Every profile the host captures a session for is accepted. A capture
    /// this refuses can never be retired, and the host then refuses every later
    /// launch of that pane: an Antigravity pane that had moved to a new
    /// conversation never reopened (21 Oct 2026).
    ///
    /// # Errors
    ///
    /// Returns an error when the reconciled roster cannot be validated or
    /// committed safely.
    pub fn reconcile_terminal_provider_reopen(
        &mut self,
        terminal_id: TerminalId,
        cwd: &str,
        launch: Option<TerminalLaunchRequestV1>,
        launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<bool, SettingsError> {
        let is_provider = launch.is_some_and(|launch| {
            matches!(
                launch.profile_id,
                TerminalLaunchProfileId::Claude
                    | TerminalLaunchProfileId::Codex
                    | TerminalLaunchProfileId::Opencode
                    | TerminalLaunchProfileId::Gemini
                    | TerminalLaunchProfileId::Muse
                    | TerminalLaunchProfileId::Kimi
            )
        }) && launch_session.is_some_and(provider_session_is_confirmed);
        let is_shell = launch
            .is_none_or(|launch| launch.profile_id == TerminalLaunchProfileId::Shell)
            && launch_session.is_none();
        if !is_provider && !is_shell {
            return Ok(false);
        }

        // Providers re-file an unchanged binding on every poll: answer that
        // from the saved roster before copying it.
        let Some((workspace, index)) =
            self.workspaces
                .iter()
                .enumerate()
                .find_map(|(workspace, record)| {
                    let index = record
                        .terminals
                        .iter()
                        .position(|terminal| terminal.id == terminal_id)?;
                    Some((workspace, index))
                })
        else {
            return Ok(false);
        };
        let saved = &self.workspaces[workspace].terminals[index];
        if saved.cwd == cwd
            && saved.launch == launch
            && saved.launch_session.as_ref() == launch_session
        {
            return Ok(false);
        }
        let mut workspaces = self.workspaces.clone();
        let saved = &mut workspaces[workspace].terminals[index];
        let launch_session = launch_session.cloned();
        cwd.clone_into(&mut saved.cwd);
        saved.launch = launch;
        saved.launch_session = launch_session;
        self.save(&workspaces)?;
        Ok(true)
    }

    #[must_use]
    pub fn terminal_provider_reopen_is_durable(
        &self,
        terminal_id: TerminalId,
        cwd: &str,
        launch: Option<TerminalLaunchRequestV1>,
        launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> bool {
        self.workspaces
            .iter()
            .flat_map(|workspace| workspace.terminals.iter())
            .any(|terminal| {
                terminal.id == terminal_id
                    && terminal.cwd == cwd
                    && terminal.launch == launch
                    && terminal.launch_session.as_ref() == launch_session
            })
    }
}

#[cfg(test)]
mod tests {
    use interface_contracts::terminal::{
        TerminalId, TerminalLaunchProfileId, TerminalLaunchRequestV1, TerminalLaunchSessionV1,
        TerminalWorkspaceRecord, TerminalWorkspaceTerminal,
    };
    use tempfile::TempDir;

    use super::{TerminalWorkspaceStore, validate_terminal_workspaces};

    fn terminal(id: TerminalId) -> TerminalWorkspaceTerminal {
        TerminalWorkspaceTerminal {
            id,
            title: "Claude".to_owned(),
            cwd: "/tmp/project".to_owned(),
            session_key: None,
            launch: None,
            launch_session: None,
        }
    }

    fn workspace(id: &str, terminals: Vec<TerminalWorkspaceTerminal>) -> TerminalWorkspaceRecord {
        TerminalWorkspaceRecord {
            id: id.to_owned(),
            name: "  Backend  ".to_owned(),
            created_at: "2026-08-28T00:00:00Z".to_owned(),
            updated_at: "2026-08-28T00:00:00Z".to_owned(),
            terminals,
            layout: None,
        }
    }

    #[test]
    fn written_bytes_match_the_pinned_format() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join("terminal-workspaces.json");
        let mut store = TerminalWorkspaceStore::open(&file).unwrap();
        let id: TerminalId =
            serde_json::from_str("\"33333333-3333-4333-8333-333333333333\"").unwrap();
        let saved = store.save(&[workspace("w-1", vec![terminal(id)])]).unwrap();
        crate::golden::assert_golden("terminal-workspaces.saved.json", directory.path(), &file);
        assert!(directory.path().join(".terminal-workspaces.lock").exists());
        let mut reopened = TerminalWorkspaceStore::open(&file).unwrap();
        assert_eq!(reopened.load().unwrap(), saved);
    }

    #[test]
    fn a_saved_roster_round_trips_and_normalizes_its_labels() {
        let directory = TempDir::new().expect("temporary directory");
        let file = directory.path().join("terminal-workspaces.json");
        let mut store = TerminalWorkspaceStore::open(&file).expect("open");
        assert_eq!(store.load().expect("empty load"), Vec::new());

        let saved = store
            .save(&[workspace("w-1", vec![terminal(TerminalId::random())])])
            .expect("save");
        assert_eq!(saved[0].name, "Backend");

        let mut reopened = TerminalWorkspaceStore::open(&file).expect("reopen");
        assert_eq!(reopened.load().expect("load"), saved);
    }

    #[test]
    fn an_identical_save_skips_the_write_only_while_memory_mirrors_disk() {
        use std::os::unix::fs::MetadataExt;

        let directory = TempDir::new().expect("temporary directory");
        let file = directory.path().join("terminal-workspaces.json");
        let inode = || std::fs::metadata(&file).expect("roster").ino();
        let roster = [workspace("w-1", vec![terminal(TerminalId::random())])];
        let mut store = TerminalWorkspaceStore::open(&file).expect("open");
        let saved = store.save(&roster).expect("first save");
        let written = inode();
        assert_eq!(store.save(&roster).expect("identical save"), saved);
        assert_eq!(inode(), written, "an unchanged roster is not rewritten");

        // A fresh store has not read the file, so even an empty roster writes.
        let mut fresh = TerminalWorkspaceStore::open(&file).expect("reopen");
        assert_eq!(fresh.save(&[]).expect("clear"), Vec::new());
        assert_ne!(inode(), written);
        let mut reopened = TerminalWorkspaceStore::open(&file).expect("reopen");
        assert_eq!(reopened.load().expect("load"), Vec::new());
    }

    #[test]
    fn workspace_names_are_durable_and_do_not_follow_later_pane_names() {
        let directory = TempDir::new().unwrap();
        let file = directory.path().join("terminal-workspaces.json");
        let mut store = TerminalWorkspaceStore::open(&file).unwrap();
        let mut first = terminal(TerminalId::random());
        first.title = "[ ! ] Action Required | codex | Fast off".into();
        let mut draft = workspace("w", vec![first]);
        draft.name = "Workspace 18".into();
        assert_eq!(
            store.save(&[draft.clone()]).unwrap()[0].name,
            "Workspace 18"
        );
        draft.terminals[0].title = "Study module improvements".into();
        assert_eq!(
            store.save(&[draft.clone()]).unwrap()[0].name,
            "Study module improvements"
        );
        drop(store);
        let mut reopened = TerminalWorkspaceStore::open(&file).unwrap();
        assert_eq!(
            reopened.load().unwrap()[0].name,
            "Study module improvements"
        );
        draft.terminals[0].title = "Different task".into();
        assert_eq!(
            reopened.save(&[draft.clone()]).unwrap()[0].name,
            "Study module improvements"
        );
        draft.name = "My learning plan".into();
        let saved = reopened.save(&[draft]).unwrap();
        assert_eq!(saved[0].name, "My learning plan");
        assert_eq!(saved[0].terminals[0].title, "Different task");
    }

    #[test]
    fn one_terminal_cannot_be_claimed_by_two_workspaces() {
        let shared = TerminalId::random();
        assert!(
            validate_terminal_workspaces(&[
                workspace("w-1", vec![terminal(shared)]),
                workspace("w-2", vec![terminal(shared)]),
            ])
            .is_err()
        );
        assert!(
            validate_terminal_workspaces(&[
                workspace("w-1", vec![terminal(TerminalId::random())]),
                workspace("w-1", vec![terminal(TerminalId::random())]),
            ])
            .is_err()
        );
        let mut relative = workspace("w-1", vec![terminal(TerminalId::random())]);
        relative.terminals[0].cwd = "project".to_owned();
        assert!(validate_terminal_workspaces(&[relative]).is_err());
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One scenario pins inertness, transitions, and stale-save merging.
    fn provider_session_reconciliation_is_confirmed_monotonic_and_durable() {
        let directory = TempDir::new().expect("temporary directory");
        let file = directory.path().join("terminal-workspaces.json");
        let target = TerminalId::random();
        let untouched = TerminalId::random();
        let mut store = TerminalWorkspaceStore::open(&file).expect("open");
        store
            .save(&[workspace(
                "w-1",
                vec![terminal(target), terminal(untouched)],
            )])
            .expect("seed roster");

        // The live seam files the provider launch, session and cwd together.
        let cwd = "/tmp/project";
        let claude = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Claude,
        });
        for provisional in [
            TerminalLaunchSessionV1 {
                version: 1,
                id: None,
                resume: true,
            },
            TerminalLaunchSessionV1 {
                version: 1,
                id: Some("fresh-request".to_owned()),
                resume: false,
            },
            TerminalLaunchSessionV1 {
                version: 2,
                id: Some("unsupported".to_owned()),
                resume: true,
            },
        ] {
            assert!(
                !store
                    .reconcile_terminal_provider_reopen(target, cwd, claude, Some(&provisional))
                    .expect("ignore provisional binding")
            );
        }

        let first = TerminalLaunchSessionV1 {
            version: 1,
            id: Some("provider-first".to_owned()),
            resume: true,
        };
        assert!(
            store
                .reconcile_terminal_provider_reopen(target, cwd, claude, Some(&first))
                .expect("persist first provider binding")
        );
        assert!(store.terminal_provider_reopen_is_durable(target, cwd, claude, Some(&first)));
        assert!(!store.terminal_provider_reopen_is_durable(untouched, cwd, claude, Some(&first)));
        assert!(
            !store
                .reconcile_terminal_provider_reopen(target, cwd, claude, Some(&first))
                .expect("same provider binding is already durable")
        );

        let transitioned = TerminalLaunchSessionV1 {
            version: 1,
            id: Some("provider-after-clear".to_owned()),
            resume: true,
        };
        assert!(
            store
                .reconcile_terminal_provider_reopen(target, cwd, claude, Some(&transitioned))
                .expect("persist provider transition")
        );
        assert!(!store.terminal_provider_reopen_is_durable(target, cwd, claude, Some(&first)));
        assert!(store.terminal_provider_reopen_is_durable(
            target,
            cwd,
            claude,
            Some(&transitioned)
        ));
        assert!(
            !store
                .reconcile_terminal_provider_reopen(
                    TerminalId::random(),
                    cwd,
                    claude,
                    Some(&transitioned)
                )
                .expect("missing terminal is inert")
        );

        let mut stale = store.get();
        stale[0].terminals[0].launch_session = Some(first);
        stale[0].terminals[1].launch_session = Some(TerminalLaunchSessionV1 {
            version: 1,
            id: None,
            resume: true,
        });
        let preserved = store
            .save_preserving_confirmed_provider_sessions(&stale)
            .expect("preserve host-confirmed binding from stale renderer snapshot");
        assert_eq!(
            preserved[0].terminals[0].launch_session,
            Some(transitioned.clone())
        );
        let untouched_binding = stale[0].terminals[1].launch_session.clone();
        assert_eq!(preserved[0].terminals[1].launch_session, untouched_binding);

        stale[0].terminals.remove(0);
        let removed = store
            .save_preserving_confirmed_provider_sessions(&stale)
            .expect("terminal removal remains authoritative");
        assert_eq!(removed[0].terminals.len(), 1);
        assert!(!store.terminal_provider_reopen_is_durable(
            target,
            cwd,
            claude,
            Some(&transitioned)
        ));

        let mut reopened = TerminalWorkspaceStore::open(&file).expect("reopen");
        let loaded = reopened.load().expect("reload reconciled roster");
        assert_eq!(loaded[0].terminals.len(), 1);
        assert_eq!(loaded[0].terminals[0].id, untouched);
        assert_eq!(loaded[0].terminals[0].launch_session, untouched_binding);
    }

    #[test]
    fn stale_claude_roster_cannot_undo_a_host_persisted_shell_demotion() {
        let directory = TempDir::new().expect("temporary directory");
        let file = directory.path().join("terminal-workspaces.json");
        let terminal_id = TerminalId::random();
        let confirmed = TerminalLaunchSessionV1 {
            version: 1,
            id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
            resume: true,
        };
        let mut claude = terminal(terminal_id);
        claude.launch = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Claude,
        });
        claude.launch_session = Some(confirmed.clone());
        let stale = vec![workspace("w-1", vec![claude])];
        let mut store = TerminalWorkspaceStore::open(&file).expect("open");
        store.save(&stale).expect("seed Claude roster");
        assert!(
            store
                .reconcile_terminal_provider_reopen(terminal_id, "/tmp/project", None, None,)
                .expect("persist shell demotion")
        );

        let preserved = store
            .save_preserving_confirmed_provider_sessions(&stale)
            .expect("reject stale Claude resurrection");
        assert_eq!(preserved[0].terminals[0].launch, None);
        assert_eq!(preserved[0].terminals[0].launch_session, None);
    }

    #[test]
    fn stale_codex_roster_cannot_undo_a_host_persisted_shell_demotion() {
        let directory = TempDir::new().expect("temporary directory");
        let file = directory.path().join("terminal-workspaces.json");
        let terminal_id = TerminalId::random();
        let confirmed = TerminalLaunchSessionV1 {
            version: 1,
            id: Some("22222222-2222-4222-8222-222222222222".to_owned()),
            resume: true,
        };
        let mut codex = terminal(terminal_id);
        codex.launch = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: TerminalLaunchProfileId::Codex,
        });
        codex.launch_session = Some(confirmed);
        let stale = vec![workspace("w-1", vec![codex])];
        let mut store = TerminalWorkspaceStore::open(&file).expect("open");
        store.save(&stale).expect("seed Codex roster");
        assert!(
            store
                .reconcile_terminal_provider_reopen(terminal_id, "/tmp/project", None, None)
                .expect("persist shell demotion")
        );

        let preserved = store
            .save_preserving_confirmed_provider_sessions(&stale)
            .expect("reject stale Codex resurrection");
        assert_eq!(preserved[0].terminals[0].launch, None);
        assert_eq!(preserved[0].terminals[0].launch_session, None);
    }
}
