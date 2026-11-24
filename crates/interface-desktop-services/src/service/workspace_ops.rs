//! Workspace, editor and Review operations: directory grants, file reads and
//! saves, Quick Open and search, customization, Review checks and checkpoints.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use interface_contracts::ipc::ReviewCheckDiscoveryError;
use interface_contracts::projects::{
    PROJECTS_FOLDER_VIEW_VERSION, ProjectsFolderRequest, ProjectsFolderSource, ProjectsFolderView,
};
use interface_review::{
    RepositoryReviewOptions, ReviewCheckError, ReviewCheckRunOptions, discover_review_checks,
    read_repository_review_with, run_review_checks,
};
use interface_settings::{
    ProjectsFolderOrigin, ResolvedProjectsFolder, directory_picker_start_path,
    resolve_projects_folder_with,
};
use interface_workspace::{
    WorkspaceAuthority, WorkspaceEditorError, WorkspaceReadError, list_project_files_bounded,
    read_customization_inventory, search_project_bounded,
};
use serde_json::Value;

use super::{
    DesktopService, display_path, encode, lexical_absolute, map_editor_error, shorten_path,
};
use crate::DesktopServiceError;
use crate::payload::{
    CreateProjectRequest, CreateReviewCheckpointPayload, DirectoryPickerStartPathPayload,
    OptionalPaths, PathPayload, PickDirectoryPayload, RestoreReviewCheckpointPayload, RootPayload,
    RunReviewChecksPayload, SaveFilePayload, SearchProjectPayload, SetWorkspaceOpenPayload, decode,
    decode_empty,
};

impl DesktopService {
    pub(super) fn get_default_directory(
        &self,
        operation: &str,
        payload: &Value,
    ) -> Result<Value, DesktopServiceError> {
        decode_empty(operation, payload)?;
        let root = self.grant_workspace_root(&self.default_directory)?;
        encode(operation, display_path(&root)?)
    }
    pub(super) fn create_project(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = match decode::<CreateProjectRequest>(operation, payload)? {
            CreateProjectRequest::Create(request) => request,
            CreateProjectRequest::Folder(request) => {
                return self.projects_folder_request(operation, request);
            }
        };
        let folder = self.resolved_projects_folder();
        let label = home_relative(&folder.path, &self.home_directory);
        let project = crate::project::create_project(&folder.path, &label, &request.name)?;
        self.trust_new_project(&project);
        let root = self.grant_workspace_root(&project).map_err(|_| {
            DesktopServiceError::new(
                "project-open-failed",
                format!(
                    "Created {} but could not open it. Choose that folder in a new workspace.",
                    project.display()
                ),
                false,
            )
            .outcome_unknown()
        })?;
        encode(operation, display_path(&root)?)
    }
    /// The folder New project creates in; `~/Developer/Projects` when the
    /// setting's file could not be opened.
    fn resolved_projects_folder(&self) -> ResolvedProjectsFolder {
        let guard = self.projects_folder.lock();
        match guard.as_ref().ok().and_then(|store| store.as_ref()) {
            Some(store) => store.resolve(&self.home_directory),
            None => resolve_projects_folder_with(&self.home_directory, None, None, Path::is_dir),
        }
    }
    fn projects_folder_request(
        &self,
        operation: &str,
        request: ProjectsFolderRequest,
    ) -> Result<Value, DesktopServiceError> {
        request
            .validate()
            .map_err(|_| DesktopServiceError::invalid_payload(operation))?;
        if let ProjectsFolderRequest::Choose {
            selected_path: Some(selected),
        } = request
        {
            let chosen = std::fs::canonicalize(&selected)
                .ok()
                .filter(|path| path.is_dir())
                .ok_or_else(|| {
                    DesktopServiceError::refused(
                        "That folder is not available.",
                        "Choose a folder that exists. The Projects folder was not changed.",
                    )
                })?;
            if chosen.parent().is_none() {
                return Err(DesktopServiceError::refused(
                    "Choose a folder inside a disk.",
                    "The disk's root cannot be the Projects folder. The setting was not changed.",
                ));
            }
            let mut guard = self.projects_folder.lock().map_err(|_| {
                DesktopServiceError::new(
                    "desktop-state-unavailable",
                    "The Projects folder setting is unavailable.",
                    false,
                )
            })?;
            let store = guard.as_mut().ok_or_else(|| {
                DesktopServiceError::new(
                    "desktop-state-unavailable",
                    "The Projects folder setting could not be opened; New project keeps using ~/Developer/Projects.",
                    false,
                )
            })?;
            store.choose(&chosen).map_err(super::map_settings_error)?;
        }
        let folder = self.resolved_projects_folder();
        encode(
            operation,
            ProjectsFolderView {
                version: PROJECTS_FOLDER_VIEW_VERSION,
                path: display_path(&folder.path)?,
                display: home_relative(&folder.path, &self.home_directory),
                source: match folder.origin {
                    ProjectsFolderOrigin::Chosen => ProjectsFolderSource::Chosen,
                    ProjectsFolderOrigin::Default => ProjectsFolderSource::Default,
                    ProjectsFolderOrigin::LastPicked => ProjectsFolderSource::LastPicked,
                },
            },
        )
    }
    /// Notes the folder holding a folder the owner picked: New project's
    /// default when `~/Developer/Projects` does not exist. Best effort: a pick
    /// never fails because this could not be written.
    fn remember_picked_folder(&self, picked: &Path) {
        if let Ok(mut guard) = self.projects_folder.lock()
            && let Some(store) = guard.as_mut()
            && let Err(error) = store.remember_picked(picked)
        {
            eprintln!("[desktop] could not note the last picked folder: {error}");
        }
    }
    pub(super) fn pick_directory(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<PickDirectoryPayload>(operation, payload)?;
        let picked = match request.selected_path {
            Value::Null => None,
            Value::String(path) if !path.is_empty() && !path.contains('\0') => {
                Some(PathBuf::from(path))
            }
            _ => return Err(DesktopServiceError::invalid_payload(operation)),
        };
        let Some(picked) = picked else {
            return Ok(Value::Null);
        };
        let root = self.grant_workspace_root(&picked)?;
        self.remember_picked_folder(&root);
        encode(operation, display_path(&root)?)
    }
    pub(super) fn native_directory_picker_start_path(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<DirectoryPickerStartPathPayload>(operation, payload)?;
        require_path(operation, &request.preferred_path)?;
        let start =
            directory_picker_start_path(Path::new(&request.preferred_path), &self.home_directory);
        encode(operation, display_path(&start)?)
    }
    pub(super) fn read_file(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<PathPayload>(operation, payload)?;
        require_path(operation, &request.path)?;
        let (authority, canonical) = self.authorized_path(&request.path, session_roots)?;
        let text = authority
            .read_text(&canonical)
            .map_err(|error| map_editor_error(error, &request.path, "open"))?;
        encode(operation, text)
    }
    pub(super) fn read_file_snapshot(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<PathPayload>(operation, payload)?;
        require_path(operation, &request.path)?;
        let (authority, canonical) = self.authorized_path(&request.path, session_roots)?;
        let snapshot = authority
            .read_snapshot(&canonical)
            .map_err(|error| map_editor_error(error, &request.path, "open"))?;
        encode(operation, snapshot)
    }
    pub(super) fn save_file(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<SaveFilePayload>(operation, payload)?;
        require_path(operation, &request.path)?;
        if request.text.len() > interface_workspace::EDITOR_FILE_BYTE_LIMIT {
            return Err(DesktopServiceError::refused(
                "Draft is too large.",
                "Interface saves text files up to 5 MB. Nothing was changed.",
            ));
        }
        let (authority, canonical) = self.authorized_path(&request.path, session_roots)?;
        let snapshot = authority
            .save_text(&canonical, &request.text, &request.expected_revision)
            .map_err(|error| {
                let mapped = if error == WorkspaceEditorError::InvalidSave {
                    WorkspaceEditorError::StaleRevision
                } else {
                    error
                };
                map_editor_error(mapped, &request.path, "save")
            })?;
        encode(operation, snapshot)
    }
    pub(super) fn set_workspace_open(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<SetWorkspaceOpenPayload>(operation, payload).map_err(|_| {
            DesktopServiceError::refused(
                "Could not protect that draft.",
                "The editor sent invalid workspace state; no worktree was changed.",
            )
        })?;
        let root = lexical_absolute(Path::new(&request.root)).ok_or_else(|| {
            DesktopServiceError::refused(
                "Could not protect that draft.",
                "The editor sent invalid workspace state; no worktree was changed.",
            )
        })?;
        if request.open {
            self.acquire_workspace_lease(&root, "editor")?;
        } else {
            self.release_workspace_lease(&root, "editor")?;
        }
        Ok(Value::Null)
    }
    pub(super) fn list_dir(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<PathPayload>(operation, payload)?;
        require_path(operation, &request.path)?;
        let (authority, canonical) = self.authorized_path(&request.path, session_roots)?;
        let entries = authority
            .list_directory(&canonical)
            .map_err(|error| map_editor_error(error, &request.path, "open"))?;
        encode(operation, entries)
    }
    pub(super) fn list_project_files(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<RootPayload>(operation, payload)?;
        if request.root.is_empty() {
            return Err(DesktopServiceError::refused(
                "Could not list project files.",
                "Choose an open project before using Quick Open.",
            ));
        }
        let (_, root) = self.authorized_path(&request.root, session_roots)?;
        let result = list_project_files_bounded(&root).map_err(|error| match error {
            WorkspaceReadError::StillDownloading => DesktopServiceError::refused(
                "Couldn't list files in this folder.",
                &format!(
                    "Part of {} is still downloading from iCloud. Try again once it has downloaded.",
                    shorten_path(&request.root)
                ),
            ),
            _ => DesktopServiceError::refused(
                "Couldn't list files in this folder.",
                &format!(
                    "{} could not be indexed. Check that the folder still exists and that Interface may read it.",
                    shorten_path(&request.root)
                ),
            ),
        })?;
        encode(operation, result)
    }
    pub(super) fn search_project(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<SearchProjectPayload>(operation, payload)?;
        validate_search_request(&request)?;
        let (_, root) = self.authorized_path(&request.root, session_roots)?;
        let result = search_project_bounded(&root, &request.query)
            .map_err(|error| map_search_error(error, &request.root))?;
        encode(operation, result)
    }
    pub(super) fn list_customization(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<RootPayload>(operation, payload)?;
        let root = self.exact_root(
            &request.root,
            session_roots,
            (
                "Could not list agent customization.",
                "Choose an open project before looking for its rules, skills and hooks.",
            ),
            "list agent customization",
        )?;
        let result = read_customization_inventory(&root, self.customization_user_root.as_deref());
        encode(operation, result)
    }
    pub(super) fn read_repository_review(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<RootPayload>(operation, payload)?;
        if request.root.is_empty() {
            return Err(DesktopServiceError::refused(
                "Could not review that folder.",
                "Choose a currently open project before opening Changes.",
            ));
        }
        let (_, root) = self.authorized_path(&request.root, session_roots)?;
        let result = read_repository_review_with(
            &root,
            &RepositoryReviewOptions {
                runner: self.review.git_runner.as_deref(),
                ..RepositoryReviewOptions::default()
            },
        );
        encode(operation, result)
    }
    pub(super) fn discover_review_checks(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<RootPayload>(operation, payload)?;
        let root = self.exact_root(
            &request.root,
            session_roots,
            (
                "Could not discover Review checks.",
                "Choose an open project before looking for package scripts.",
            ),
            "discover Review checks",
        )?;
        let result =
            discover_review_checks(&root).map_err(|error| map_review_check_error(&error))?;
        encode(operation, result)
    }
    pub(super) fn run_review_checks(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        const REFUSED: (&str, &str) = (
            "Could not run Review checks.",
            "Choose checks offered for an exact open project and try again.",
        );
        let request = decode::<RunReviewChecksPayload>(operation, payload)?;
        if request.selected.len() > 5
            || request
                .selected
                .iter()
                .any(|name| name.encode_utf16().count() > 16)
        {
            return Err(DesktopServiceError::refused(REFUSED.0, REFUSED.1));
        }
        let root = self.exact_root(&request.root, session_roots, REFUSED, "run Review checks")?;
        let result = run_review_checks(
            &root,
            &request.selected,
            &ReviewCheckRunOptions {
                runner: self.review.check_runner.as_deref(),
                ..ReviewCheckRunOptions::default()
            },
        )
        .map_err(|error| map_review_check_error(&error))?;
        encode(operation, result)
    }
    pub(super) fn create_review_checkpoint(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        const REFUSED: (&str, &str) = (
            "Could not create a Review checkpoint.",
            "Choose an exact open project first.",
        );
        let request = decode::<CreateReviewCheckpointPayload>(operation, payload)
            .map_err(|_| DesktopServiceError::refused(REFUSED.0, REFUSED.1))?;
        let root = self.exact_root(
            &request.root,
            session_roots,
            REFUSED,
            "create a Review checkpoint",
        )?;
        let fresh = read_repository_review_with(
            &root,
            &RepositoryReviewOptions {
                runner: self.review.git_runner.as_deref(),
                skip_patches: true,
                ..RepositoryReviewOptions::default()
            },
        );
        if !fresh.repository {
            return Err(DesktopServiceError::refused(
                REFUSED.0,
                fresh.error.as_ref().map_or(
                    "Git could not confirm this open folder as a repository.",
                    |error| error.message.as_str(),
                ),
            ));
        }
        let kind = if request.kind.as_ref().is_some_and(|kind| kind == "turn") {
            interface_registry::CheckpointKind::Turn
        } else {
            interface_registry::CheckpointKind::Review
        };
        encode(operation, self.checkpoints.create(&fresh, kind)?)
    }
    pub(super) fn list_review_checkpoints(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        const REFUSED: (&str, &str) = (
            "Could not list Review checkpoints.",
            "Choose an exact open project first.",
        );
        let request = decode::<RootPayload>(operation, payload)
            .map_err(|_| DesktopServiceError::refused(REFUSED.0, REFUSED.1))?;
        let root = self.exact_root(
            &request.root,
            session_roots,
            REFUSED,
            "list Review checkpoints",
        )?;
        encode(operation, self.checkpoints.list(&root)?)
    }
    pub(super) fn restore_review_checkpoint(
        &self,
        operation: &str,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        const REFUSED: (&str, &str) = (
            "Could not restore that Review checkpoint.",
            "The restore request was invalid; no files were changed.",
        );
        let invalid = || DesktopServiceError::refused(REFUSED.0, REFUSED.1);
        let request =
            decode::<RestoreReviewCheckpointPayload>(operation, payload).map_err(|_| invalid())?;
        // Present paths must be an array of strings; anything else is refused.
        let paths = match request.paths {
            OptionalPaths::Missing => None,
            OptionalPaths::Present(paths) => {
                Some(serde_json::from_value::<Vec<String>>(paths).map_err(|_| invalid())?)
            }
        };
        let paths_valid = paths.as_ref().is_none_or(|paths| {
            paths.len() <= 200
                && paths
                    .iter()
                    .all(|path| path.encode_utf16().count() <= 4_096)
        });
        if request.id.is_empty() || request.id.encode_utf16().count() > 128 || !paths_valid {
            return Err(invalid());
        }
        let root = self.exact_root(
            &request.root,
            session_roots,
            REFUSED,
            "restore a Review checkpoint",
        )?;
        encode(
            operation,
            self.checkpoints
                .restore(&root, &request.id, paths.as_deref())?,
        )
    }
}

impl DesktopService {
    /// `target` canonicalized inside the current authorized roots, with the
    /// authority snapshot that admitted it.
    fn authorized_path(
        &self,
        target: &str,
        session_roots: &[PathBuf],
    ) -> Result<(Arc<WorkspaceAuthority>, PathBuf), DesktopServiceError> {
        let authority = self.workspace_authority_snapshot(session_roots);
        let canonical = Self::canonical_authorized_path(&authority, target)?;
        Ok((authority, canonical))
    }

    /// `root` as an exact authorized workspace root for `what`. A blank root is
    /// refused first with the operation's own `(lead, detail)`.
    pub(super) fn exact_root(
        &self,
        root: &str,
        session_roots: &[PathBuf],
        blank: (&str, &str),
        what: &str,
    ) -> Result<PathBuf, DesktopServiceError> {
        if root.is_empty() {
            return Err(DesktopServiceError::refused(blank.0, blank.1));
        }
        let authority = self.workspace_authority_snapshot(session_roots);
        Self::exact_authorized_root(&authority, root, what)
    }
}

fn require_path(operation: &str, path: &str) -> Result<(), DesktopServiceError> {
    if path.is_empty() || path.contains('\0') {
        Err(DesktopServiceError::invalid_payload(operation))
    } else {
        Ok(())
    }
}

fn validate_search_request(request: &SearchProjectPayload) -> Result<(), DesktopServiceError> {
    if request.root.is_empty() || request.query.is_empty() {
        return Err(map_search_error(
            WorkspaceReadError::EmptyQuery,
            &request.root,
        ));
    }
    if request.query.encode_utf16().count()
        > interface_workspace::PROJECT_SEARCH_LIMITS.max_query_chars
    {
        return Err(map_search_error(
            WorkspaceReadError::QueryTooLong,
            &request.root,
        ));
    }
    Ok(())
}

fn map_search_error(error: WorkspaceReadError, root: &str) -> DesktopServiceError {
    match error {
        WorkspaceReadError::EmptyQuery => DesktopServiceError::refused(
            "Could not start that search.",
            "Choose an open project and type some text to search for.",
        ),
        WorkspaceReadError::QueryTooLong => DesktopServiceError::refused(
            "That search is too long.",
            &format!(
                "Use at most {} characters.",
                interface_workspace::PROJECT_SEARCH_LIMITS.max_query_chars
            ),
        ),
        WorkspaceReadError::StillDownloading => DesktopServiceError::refused(
            "Couldn't search this folder.",
            &format!(
                "Part of {} is still downloading from iCloud. Try again once it has downloaded.",
                shorten_path(root)
            ),
        ),
        WorkspaceReadError::UnsafeRoot => DesktopServiceError::refused(
            "Couldn't search this folder.",
            &format!(
                "{} could not be searched. Check that the folder still exists and that Interface may read it.",
                shorten_path(root)
            ),
        ),
    }
}

fn map_review_check_error(error: &ReviewCheckError) -> DesktopServiceError {
    let message = match error {
        ReviewCheckError::UnsafeRoot => {
            "The authorized directory changed during the operation.".to_owned()
        }
        ReviewCheckError::TooManySelected => "Too many Review checks were selected.".to_owned(),
        ReviewCheckError::DuplicateSelection => {
            "A Review check was selected more than once.".to_owned()
        }
        ReviewCheckError::DiscoveryFailed(reason) => format!(
            "Review check discovery failed: {}.",
            match reason {
                ReviewCheckDiscoveryError::InvalidPackage => "invalid-package",
                ReviewCheckDiscoveryError::PackageTooLarge => "package-too-large",
                ReviewCheckDiscoveryError::UnsafePackage => "unsafe-package",
            }
        ),
        ReviewCheckError::CheckUnavailable(name) => {
            format!("Review check is not available: {name}.")
        }
    };
    DesktopServiceError::new("review-check-failed", message, false)
}

/// `path` with the home folder written `~` (either spelling of home, so a
/// canonical `/private/var/…` home matches too); otherwise unchanged.
fn home_relative(path: &Path, home: &Path) -> String {
    let canonical_home = std::fs::canonicalize(home).ok();
    for home in std::iter::once(home).chain(canonical_home.as_deref()) {
        if let Ok(rest) = path.strip_prefix(home) {
            return if rest.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}
