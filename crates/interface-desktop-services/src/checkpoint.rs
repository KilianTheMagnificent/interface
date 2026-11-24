use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use interface_contracts::canonical_timestamp;
use interface_contracts::ipc::{
    RepositoryReview, RepositoryReviewFileStatus, RepositoryReviewOmission, ReviewCheckpointDetail,
    ReviewCheckpointFile, ReviewCheckpointKind, ReviewCheckpointOmission,
    ReviewCheckpointOmissionReason, ReviewCheckpointSummary,
};
use interface_registry::{
    CheckpointBlobReader, CheckpointDraft, CheckpointFile, CheckpointInputFile, CheckpointKind,
    CheckpointLimits, CheckpointManifest, CheckpointOmission, CheckpointOmissionReason,
    CheckpointStore, CheckpointSummary, RegistryError, bounded_unsafe_path,
    inspect_checkpoint_read_only, is_utf8_text, list_checkpoints_read_only,
};
use interface_workspace::{is_likely_binary_path, is_search_excluded_path};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use interface_common::fsid::{DirectorySnapshot, FileIdentity};

use crate::{
    DesktopServiceError,
    service::{display_path, lexical_absolute},
};

use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

/// Deterministic scheduler-gap hooks used by adversarial restore tests.
/// Production leaves this unset.
pub trait CheckpointHooks: Send + Sync {
    fn after_source_validated(&self, _relative: &str, _absolute: &Path) {}
    fn before_restore_rename(&self, _relative: &str, _absolute: &Path) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreSkipReason {
    NotInCheckpoint,
    UnsafePath,
    Credential,
    Binary,
    Symlink,
    NotRegular,
    ParentMissing,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreConflictReason {
    CheckpointCorrupt,
    RecoveryUnavailable,
    ChangedDuringRestore,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReviewRestoreIssue<T> {
    pub path: String,
    pub reason: T,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCheckpointRestoreResult {
    pub checkpoint_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_checkpoint_id: Option<String>,
    pub restored: Vec<String>,
    pub skipped: Vec<ReviewRestoreIssue<RestoreSkipReason>>,
    pub conflicts: Vec<ReviewRestoreIssue<RestoreConflictReason>>,
}

pub(crate) struct CheckpointService {
    store_path: Option<PathBuf>,
    writer: Mutex<Option<CheckpointStore>>,
    limits: CheckpointLimits,
    hooks: Option<Arc<dyn CheckpointHooks>>,
}

impl CheckpointService {
    pub(crate) fn new(
        store_path: Option<PathBuf>,
        hooks: Option<Arc<dyn CheckpointHooks>>,
    ) -> Self {
        Self {
            store_path: store_path.filter(|path| !path.as_os_str().is_empty()),
            writer: Mutex::new(None),
            limits: CheckpointLimits::default(),
            hooks,
        }
    }

    pub(crate) fn create(
        &self,
        review: &RepositoryReview,
        kind: CheckpointKind,
    ) -> Result<ReviewCheckpointDetail, DesktopServiceError> {
        self.store_path()?;
        let root = stable_directory(Path::new(&review.root)).map_err(checkpoint_fs_error)?;
        if root.path != Path::new(&review.root) {
            return Err(checkpoint_failure(
                "The reviewed root is no longer authorized.",
            ));
        }
        if review.files.len() > self.limits.max_input_files {
            return Err(checkpoint_failure(
                "Repository review has too many paths for a checkpoint.",
            ));
        }

        let mut files = Vec::new();
        let mut omitted = Vec::new();
        let mut total_bytes = 0usize;
        for item in &review.files {
            let Some(relative) = safe_relative(&item.path) else {
                omitted.push(CheckpointOmission {
                    path: bounded_unsafe_path(&item.path),
                    reason: CheckpointOmissionReason::UnsafePath,
                });
                continue;
            };
            if is_search_excluded_path(&relative, false) {
                omitted.push(omission(relative, CheckpointOmissionReason::Credential));
                continue;
            }
            if item.status == RepositoryReviewFileStatus::Deleted {
                omitted.push(omission(relative, CheckpointOmissionReason::Deleted));
                continue;
            }
            if let Some(reason) = item.omitted {
                omitted.push(omission(relative, review_omission(reason)));
                continue;
            }
            if item.binary || is_likely_binary_path(&relative) {
                omitted.push(omission(relative, CheckpointOmissionReason::Binary));
                continue;
            }
            if files.len() >= self.limits.max_files {
                omitted.push(omission(relative, CheckpointOmissionReason::FileLimit));
                continue;
            }
            let remaining = self.limits.max_total_bytes.saturating_sub(total_bytes);
            if remaining == 0 {
                omitted.push(omission(relative, CheckpointOmissionReason::ByteLimit));
                continue;
            }
            let maximum = self.limits.max_file_bytes.min(remaining);
            match read_verified_file(&root, &relative, maximum, self.hooks.as_deref()) {
                Ok(opened) if is_utf8_text(&opened.content) => {
                    total_bytes += opened.content.len();
                    files.push(CheckpointInputFile {
                        path: relative,
                        content: opened.content,
                        mode: opened.mode,
                    });
                }
                Ok(_) => omitted.push(omission(relative, CheckpointOmissionReason::Binary)),
                Err(error) => omitted.push(omission(relative, capture_omission(error))),
            }
        }
        assert_directory(&root).map_err(checkpoint_fs_error)?;

        let (created_at, id) = checkpoint_identity();
        let manifest = self.create_draft(CheckpointDraft {
            id,
            kind,
            created_at,
            repository_root: root.path,
            repository_head: review.head.clone(),
            source_checkpoint_id: None,
            files,
            omitted,
        })?;
        checkpoint_detail(&manifest)
    }

    pub(crate) fn list(
        &self,
        root: &Path,
    ) -> Result<Vec<ReviewCheckpointSummary>, DesktopServiceError> {
        let store_path = self.store_path()?;
        let summaries = {
            let guard = self
                .writer
                .lock()
                .map_err(|_| DesktopServiceError::state_unavailable())?;
            if let Some(store) = guard.as_ref() {
                store.list(Some(root))
            } else {
                list_checkpoints_read_only(store_path, self.limits, Some(root))
            }
        }
        .map_err(|error| checkpoint_registry_error(&error))?;
        summaries.iter().map(checkpoint_summary).collect()
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn restore(
        &self,
        authorized_root: &Path,
        id: &str,
        selected_paths: Option<&[String]>,
    ) -> Result<ReviewCheckpointRestoreResult, DesktopServiceError> {
        let store_path = self.store_path()?;
        let manifest = inspect_checkpoint_read_only(store_path, id, self.limits)
            .map_err(|error| checkpoint_registry_error(&error))?;
        let root = stable_directory(authorized_root).map_err(checkpoint_fs_error)?;
        if manifest.repository_root != root.path {
            return Err(checkpoint_failure(
                "Checkpoint belongs to a different repository root.",
            ));
        }

        let requested = selected_paths.map_or_else(
            || {
                manifest
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>()
            },
            <[String]>::to_vec,
        );
        if requested.len() > self.limits.max_files
            || requested.iter().collect::<HashSet<_>>().len() != requested.len()
        {
            return Err(checkpoint_failure("Invalid restore selection."));
        }

        let by_path = manifest
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect::<HashMap<_, _>>();
        let blob_reader = CheckpointBlobReader::open_read_only(store_path, &manifest, self.limits)
            .map_err(|error| checkpoint_registry_error(&error))?;
        let mut result = ReviewCheckpointRestoreResult {
            checkpoint_id: manifest.id.clone(),
            recovery_checkpoint_id: None,
            restored: Vec::new(),
            skipped: Vec::new(),
            conflicts: Vec::new(),
        };
        let mut plans = Vec::new();

        for raw_path in requested {
            let Some(relative) = safe_relative(&raw_path) else {
                result.skipped.push(skip(
                    bounded_unsafe_path(&raw_path),
                    RestoreSkipReason::UnsafePath,
                ));
                continue;
            };
            if is_search_excluded_path(&relative, false) {
                result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::Credential));
                continue;
            }
            if is_likely_binary_path(&relative) {
                result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::Binary));
                continue;
            }
            let Some(entry) = by_path.get(relative.as_str()).copied() else {
                result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::NotInCheckpoint));
                continue;
            };
            let Ok(content) = blob_reader.read(entry) else {
                result
                    .conflicts
                    .push(conflict(relative, RestoreConflictReason::CheckpointCorrupt));
                continue;
            };
            match current_target(&root, entry, self.limits, self.hooks.as_deref()) {
                Ok(current) => {
                    if current.existing.as_ref().is_some_and(|existing| {
                        digest(&existing.content) == entry.sha256 && existing.mode == entry.mode
                    }) {
                        result
                            .skipped
                            .push(skip(relative, RestoreSkipReason::Unchanged));
                        continue;
                    }
                    let (existing, existing_content) = match current.existing {
                        Some(existing) => (
                            Some((existing.snapshot, existing.mode)),
                            Some(existing.content),
                        ),
                        None => (None, None),
                    };
                    plans.push(RestorePlan {
                        entry: (*entry).clone(),
                        content,
                        parent: current.parent,
                        existing,
                        existing_content,
                    });
                }
                Err(TargetError::Symlink) => result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::Symlink)),
                Err(TargetError::NotRegular) => result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::NotRegular)),
                Err(TargetError::Binary) => result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::Binary)),
                Err(TargetError::ParentMissing) => result
                    .skipped
                    .push(skip(relative, RestoreSkipReason::ParentMissing)),
                Err(TargetError::Changed | TargetError::ByteLimit) => {
                    result.conflicts.push(conflict(
                        relative,
                        RestoreConflictReason::RecoveryUnavailable,
                    ));
                }
            }
        }

        let recovery_files = plans
            .iter_mut()
            .filter_map(|plan| {
                let mode = plan.existing.as_ref().map(|(_, mode)| *mode)?;
                plan.existing_content
                    .take()
                    .map(|content| CheckpointInputFile {
                        path: plan.entry.path.clone(),
                        content,
                        mode,
                    })
            })
            .collect::<Vec<_>>();
        if !recovery_files.is_empty() {
            let (created_at, recovery_id) = checkpoint_identity();
            let recovery = self.create_draft(CheckpointDraft {
                id: recovery_id,
                kind: CheckpointKind::Recovery,
                created_at,
                repository_root: root.path.clone(),
                repository_head: manifest.repository_head.clone(),
                source_checkpoint_id: Some(manifest.id.clone()),
                files: recovery_files,
                omitted: Vec::new(),
            })?;
            result.recovery_checkpoint_id = Some(recovery.id);
        }

        let mut renamed_into = Vec::<DirectorySnapshot>::new();
        for plan in plans {
            let relative = plan.entry.path.clone();
            match restore_one(&root, &plan, self.hooks.as_deref()) {
                Ok(true) => {
                    result.restored.push(relative);
                    if !renamed_into
                        .iter()
                        .any(|parent| parent.identity() == plan.parent.identity())
                    {
                        renamed_into.push(plan.parent);
                    }
                }
                Ok(false) | Err(_) => result.conflicts.push(conflict(
                    relative,
                    RestoreConflictReason::ChangedDuringRestore,
                )),
            }
        }
        // A rename is durable once its directory is. Sync each parent once,
        // best effort: the outcome is already decided, and the recovery
        // checkpoint was durable before the first rename.
        for parent in renamed_into {
            let _ = open_no_follow(&parent.path).and_then(|directory| directory.sync_all());
        }
        Ok(result)
    }

    fn store_path(&self) -> Result<&Path, DesktopServiceError> {
        self.store_path.as_deref().ok_or_else(|| {
            DesktopServiceError::refused(
                "Review checkpoints are unavailable.",
                "This Interface build has no private checkpoint storage configured.",
            )
        })
    }

    fn create_draft(
        &self,
        draft: CheckpointDraft,
    ) -> Result<CheckpointManifest, DesktopServiceError> {
        let store_path = self.store_path()?.to_path_buf();
        let mut guard = self
            .writer
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())?;
        if guard.is_none() {
            *guard = Some(
                CheckpointStore::open(&store_path, self.limits)
                    .map_err(|error| checkpoint_registry_error(&error))?,
            );
        }
        let store = guard
            .as_mut()
            .ok_or_else(DesktopServiceError::state_unavailable)?;
        let revision = store.revision();
        store
            .create(revision, draft)
            .map_err(|error| checkpoint_registry_error(&error))
    }
}

fn checkpoint_identity() -> (String, String) {
    let now = canonical_timestamp(Utc::now());
    let id = format!("{}-{}", now.replace(':', "-"), Uuid::new_v4());
    (now, id)
}

fn checkpoint_summary(
    value: &CheckpointSummary,
) -> Result<ReviewCheckpointSummary, DesktopServiceError> {
    Ok(ReviewCheckpointSummary {
        id: value.id.clone(),
        kind: checkpoint_kind(value.kind),
        created_at: value.created_at.clone(),
        repository_root: display_path(&value.repository_root)?,
        repository_head: value.repository_head.clone(),
        source_checkpoint_id: value.source_checkpoint_id.clone(),
        file_count: u64::try_from(value.file_count).unwrap_or(u64::MAX),
        omitted_count: u64::try_from(value.omitted_count).unwrap_or(u64::MAX),
        total_bytes: value.total_bytes,
    })
}

fn checkpoint_detail(
    manifest: &CheckpointManifest,
) -> Result<ReviewCheckpointDetail, DesktopServiceError> {
    Ok(ReviewCheckpointDetail {
        summary: ReviewCheckpointSummary {
            id: manifest.id.clone(),
            kind: checkpoint_kind(manifest.kind),
            created_at: manifest.created_at.clone(),
            repository_root: display_path(&manifest.repository_root)?,
            repository_head: manifest.repository_head.clone(),
            source_checkpoint_id: manifest.source_checkpoint_id.clone(),
            file_count: u64::try_from(manifest.files.len()).unwrap_or(u64::MAX),
            omitted_count: u64::try_from(manifest.omitted.len()).unwrap_or(u64::MAX),
            total_bytes: manifest.total_bytes,
        },
        files: manifest
            .files
            .iter()
            .map(|file| ReviewCheckpointFile {
                path: file.path.clone(),
                bytes: file.bytes,
            })
            .collect(),
        omitted: manifest
            .omitted
            .iter()
            .map(|item| ReviewCheckpointOmission {
                path: item.path.clone(),
                reason: checkpoint_omission(item.reason),
            })
            .collect(),
    })
}

const fn checkpoint_kind(kind: CheckpointKind) -> ReviewCheckpointKind {
    match kind {
        CheckpointKind::Review => ReviewCheckpointKind::Review,
        CheckpointKind::Recovery => ReviewCheckpointKind::Recovery,
        CheckpointKind::Turn => ReviewCheckpointKind::Turn,
    }
}

const fn checkpoint_omission(reason: CheckpointOmissionReason) -> ReviewCheckpointOmissionReason {
    match reason {
        CheckpointOmissionReason::Credential => ReviewCheckpointOmissionReason::Credential,
        CheckpointOmissionReason::Binary => ReviewCheckpointOmissionReason::Binary,
        CheckpointOmissionReason::Symlink => ReviewCheckpointOmissionReason::Symlink,
        CheckpointOmissionReason::NotRegular => ReviewCheckpointOmissionReason::NotRegular,
        CheckpointOmissionReason::UnsafePath => ReviewCheckpointOmissionReason::UnsafePath,
        CheckpointOmissionReason::OutputLimit => ReviewCheckpointOmissionReason::OutputLimit,
        CheckpointOmissionReason::GitError => ReviewCheckpointOmissionReason::GitError,
        CheckpointOmissionReason::Deleted => ReviewCheckpointOmissionReason::Deleted,
        CheckpointOmissionReason::Changed => ReviewCheckpointOmissionReason::Changed,
        CheckpointOmissionReason::FileLimit => ReviewCheckpointOmissionReason::FileLimit,
        CheckpointOmissionReason::ByteLimit => ReviewCheckpointOmissionReason::ByteLimit,
    }
}

const fn review_omission(reason: RepositoryReviewOmission) -> CheckpointOmissionReason {
    match reason {
        RepositoryReviewOmission::Credential => CheckpointOmissionReason::Credential,
        RepositoryReviewOmission::Binary => CheckpointOmissionReason::Binary,
        RepositoryReviewOmission::Symlink => CheckpointOmissionReason::Symlink,
        RepositoryReviewOmission::NotRegular => CheckpointOmissionReason::NotRegular,
        RepositoryReviewOmission::UnsafePath => CheckpointOmissionReason::UnsafePath,
        RepositoryReviewOmission::OutputLimit => CheckpointOmissionReason::OutputLimit,
        RepositoryReviewOmission::GitError => CheckpointOmissionReason::GitError,
    }
}

fn omission(path: String, reason: CheckpointOmissionReason) -> CheckpointOmission {
    CheckpointOmission { path, reason }
}

fn skip(path: String, reason: RestoreSkipReason) -> ReviewRestoreIssue<RestoreSkipReason> {
    ReviewRestoreIssue { path, reason }
}

fn conflict(
    path: String,
    reason: RestoreConflictReason,
) -> ReviewRestoreIssue<RestoreConflictReason> {
    ReviewRestoreIssue { path, reason }
}

fn checkpoint_failure(message: &str) -> DesktopServiceError {
    DesktopServiceError::new("review-checkpoint-failed", message, false)
}

fn checkpoint_registry_error(error: &RegistryError) -> DesktopServiceError {
    checkpoint_failure(&error.to_string())
}

fn checkpoint_fs_error(_error: TargetError) -> DesktopServiceError {
    checkpoint_failure("The authorized directory changed during the operation.")
}

fn safe_relative(raw: &str) -> Option<String> {
    if raw.is_empty()
        || raw.encode_utf16().count() > 4_096
        || raw.contains('\0')
        || raw.contains(['\r', '\n'])
        || raw.starts_with('/')
        || raw.contains('\\')
        || raw
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return None;
    }
    Some(raw.to_owned())
}

fn digest(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileSnapshot {
    identity: FileIdentity,
    size: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[derive(Clone, Debug)]
struct OpenedFile {
    content: Vec<u8>,
    mode: u32,
    snapshot: FileSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TargetError {
    Symlink,
    NotRegular,
    ParentMissing,
    Binary,
    ByteLimit,
    Changed,
}

fn identity(metadata: &Metadata) -> FileIdentity {
    FileIdentity::of(metadata)
}

fn file_snapshot(metadata: &Metadata) -> FileSnapshot {
    FileSnapshot {
        identity: identity(metadata),
        size: metadata.len(),
        mode: metadata.mode() & 0o777,
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

fn stable_directory(path: &Path) -> Result<DirectorySnapshot, TargetError> {
    let requested = lexical_absolute(path).ok_or(TargetError::Changed)?;
    DirectorySnapshot::capture(&requested).map_err(|_| TargetError::Changed)
}

fn assert_directory(snapshot: &DirectorySnapshot) -> Result<(), TargetError> {
    snapshot
        .assert_unchanged()
        .map_err(|_| TargetError::Changed)
}

fn stable_child_directory(
    root: &DirectorySnapshot,
    path: &Path,
) -> Result<DirectorySnapshot, TargetError> {
    let child = match stable_directory(path) {
        Ok(child) => child,
        Err(_) if !path.exists() => return Err(TargetError::ParentMissing),
        Err(error) => return Err(error),
    };
    if child.path != root.path && !child.path.starts_with(&root.path) {
        return Err(TargetError::Changed);
    }
    assert_directory(root)?;
    Ok(child)
}

fn open_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    options.open(path)
}

fn read_verified_file(
    root: &DirectorySnapshot,
    relative: &str,
    maximum: usize,
    hooks: Option<&dyn CheckpointHooks>,
) -> Result<OpenedFile, TargetError> {
    let target = root.path.join(relative);
    if !target.starts_with(&root.path) {
        return Err(TargetError::Changed);
    }
    assert_directory(root)?;
    let parent = stable_child_directory(root, target.parent().ok_or(TargetError::Changed)?)?;
    let before = fs::symlink_metadata(&target).map_err(|_| TargetError::NotRegular)?;
    if before.file_type().is_symlink() {
        return Err(TargetError::Symlink);
    }
    if !before.file_type().is_file() {
        return Err(TargetError::NotRegular);
    }
    if before.len() > u64::try_from(maximum).unwrap_or(u64::MAX) {
        return Err(TargetError::ByteLimit);
    }
    let snapshot = file_snapshot(&before);
    if let Some(hooks) = hooks {
        hooks.after_source_validated(relative, &target);
    }
    assert_directory(root)?;
    assert_directory(&parent)?;

    let mut file = open_no_follow(&target).map_err(|_| TargetError::Changed)?;
    let opened = file.metadata().map_err(|_| TargetError::Changed)?;
    let after = fs::symlink_metadata(&target).map_err(|_| TargetError::Changed)?;
    if !opened.file_type().is_file()
        || opened.file_type().is_symlink()
        || file_snapshot(&opened) != snapshot
        || file_snapshot(&after) != snapshot
    {
        return Err(TargetError::Changed);
    }
    let size = usize::try_from(opened.len()).map_err(|_| TargetError::ByteLimit)?;
    let mut content = Vec::with_capacity(size);
    file.read_to_end(&mut content)
        .map_err(|_| TargetError::Changed)?;
    let final_opened = file.metadata().map_err(|_| TargetError::Changed)?;
    let final_path = fs::symlink_metadata(&target).map_err(|_| TargetError::Changed)?;
    if content.len() != size
        || final_path.file_type().is_symlink()
        || file_snapshot(&final_opened) != snapshot
        || file_snapshot(&final_path) != snapshot
    {
        return Err(TargetError::Changed);
    }
    assert_directory(&parent)?;
    assert_directory(root)?;
    Ok(OpenedFile {
        content,
        mode: snapshot.mode,
        snapshot,
    })
}

const fn capture_omission(error: TargetError) -> CheckpointOmissionReason {
    match error {
        TargetError::Symlink => CheckpointOmissionReason::Symlink,
        TargetError::NotRegular => CheckpointOmissionReason::NotRegular,
        TargetError::ByteLimit => CheckpointOmissionReason::ByteLimit,
        TargetError::Binary => CheckpointOmissionReason::Binary,
        TargetError::ParentMissing | TargetError::Changed => CheckpointOmissionReason::Changed,
    }
}

struct CurrentTarget {
    parent: DirectorySnapshot,
    existing: Option<OpenedFile>,
}

fn current_target(
    root: &DirectorySnapshot,
    entry: &CheckpointFile,
    limits: CheckpointLimits,
    hooks: Option<&dyn CheckpointHooks>,
) -> Result<CurrentTarget, TargetError> {
    let target = root.path.join(&entry.path);
    let parent = stable_child_directory(root, target.parent().ok_or(TargetError::Changed)?)?;
    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert_directory(root)?;
            assert_directory(&parent)?;
            Ok(CurrentTarget {
                parent,
                existing: None,
            })
        }
        Err(_) => Err(TargetError::Changed),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(TargetError::Symlink),
        Ok(metadata) if !metadata.file_type().is_file() => Err(TargetError::NotRegular),
        Ok(_) => {
            let existing = read_verified_file(root, &entry.path, limits.max_file_bytes, hooks)?;
            if !is_utf8_text(&existing.content) {
                return Err(TargetError::Binary);
            }
            Ok(CurrentTarget {
                parent,
                existing: Some(existing),
            })
        }
    }
}

struct RestorePlan {
    entry: CheckpointFile,
    content: Vec<u8>,
    parent: DirectorySnapshot,
    existing: Option<(FileSnapshot, u32)>,
    existing_content: Option<Vec<u8>>,
}

fn target_still_matches(root: &DirectorySnapshot, plan: &RestorePlan) -> Result<bool, TargetError> {
    assert_directory(root)?;
    assert_directory(&plan.parent)?;
    let target = root.path.join(&plan.entry.path);
    match fs::symlink_metadata(&target) {
        Ok(current) => Ok(plan.existing.as_ref().is_some_and(|(snapshot, _)| {
            current.file_type().is_file()
                && !current.file_type().is_symlink()
                && file_snapshot(&current) == *snapshot
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(plan.existing.is_none()),
        Err(_) => Err(TargetError::Changed),
    }
}

fn safe_remove_temp(parent: &DirectorySnapshot, temp: &Path, identity: FileIdentity) {
    if assert_directory(parent).is_ok()
        && let Ok(current) = fs::symlink_metadata(temp)
        && current.file_type().is_file()
        && !current.file_type().is_symlink()
        && crate::checkpoint::identity(&current) == identity
    {
        let _ = fs::remove_file(temp);
    }
}

fn restore_one(
    root: &DirectorySnapshot,
    plan: &RestorePlan,
    hooks: Option<&dyn CheckpointHooks>,
) -> Result<bool, TargetError> {
    if !target_still_matches(root, plan)? {
        return Ok(false);
    }
    let target = root.path.join(&plan.entry.path);
    let temp = plan
        .parent
        .path
        .join(format!(".interface-restore-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options.open(&temp).map_err(|_| TargetError::Changed)?;
    file.write_all(&plan.content)
        .map_err(|_| TargetError::Changed)?;
    file.set_permissions(fs::Permissions::from_mode(plan.entry.mode & 0o777))
        .map_err(|_| TargetError::Changed)?;
    file.sync_all().map_err(|_| TargetError::Changed)?;
    let temp_identity = identity(&file.metadata().map_err(|_| TargetError::Changed)?);
    drop(file);

    let renamed = (|| -> Result<bool, TargetError> {
        if let Some(hooks) = hooks {
            hooks.before_restore_rename(&plan.entry.path, &target);
        }
        if !target_still_matches(root, plan)? {
            return Ok(false);
        }
        assert_directory(&plan.parent)?;
        assert_directory(root)?;
        fs::rename(&temp, &target).map_err(|_| TargetError::Changed)?;
        Ok(true)
    })();
    if !renamed.as_ref().is_ok_and(|renamed| *renamed) {
        safe_remove_temp(&plan.parent, &temp, temp_identity);
    }
    renamed
}
