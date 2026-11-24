use std::borrow::Cow;
use std::collections::VecDeque;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use interface_common::bounded_read::{self, is_dataless};
use interface_common::paths::{BINARY_EXTENSIONS, SENSITIVE_DIRECTORIES};
use interface_contracts::ipc::{
    ProjectFileEntry, ProjectFileListResponse, ProjectSearchMatch, ProjectSearchResponse,
};
use regex::{Regex, RegexBuilder};
use thiserror::Error;

use crate::fsid::{FileIdentity, identity, inside};

use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

/// Hard defaults that keep project discovery and content search bounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectSearchLimits {
    pub max_files: usize,
    pub max_entries: usize,
    pub max_file_bytes: usize,
    pub max_total_bytes: usize,
    pub max_results: usize,
    pub max_query_chars: usize,
}

pub const PROJECT_SEARCH_LIMITS: ProjectSearchLimits = ProjectSearchLimits {
    max_files: 10_000,
    max_entries: 50_000,
    max_file_bytes: 1024 * 1024,
    max_total_bytes: 32 * 1024 * 1024,
    max_results: 200,
    max_query_chars: 256,
};

/// Deterministic seam used by adversarial race tests. Production callers leave
/// this unset.
pub trait ProjectWalkHooks: Send + Sync {
    fn after_directory_validated(&self, _directory: &Path) {}
    fn after_file_validated(&self, _file: &Path) {}
}

/// Optional per-call ceilings. A supplied zero is raised to one, matching the
/// legacy positive-limit behavior.
#[derive(Default)]
pub struct ProjectWalkOptions<'a> {
    pub max_files: Option<usize>,
    pub max_entries: Option<usize>,
    pub max_file_bytes: Option<usize>,
    pub max_total_bytes: Option<usize>,
    pub max_results: Option<usize>,
    pub hooks: Option<&'a dyn ProjectWalkHooks>,
}

/// Sanitized workspace-read failures. Individual unreadable candidates are
/// skipped; only unsafe roots and invalid queries fail the whole operation.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum WorkspaceReadError {
    #[error("type something to search for")]
    EmptyQuery,
    #[error("search text is limited to 256 characters")]
    QueryTooLong,
    #[error("the project root changed or crossed a symbolic link")]
    UnsafeRoot,
    /// The listing did not finish within its deadline: part of the project
    /// is most likely still downloading from iCloud.
    #[error("part of this project is still downloading from iCloud")]
    StillDownloading,
}

/// How long Quick Open's listing may take before it says the project is
/// still downloading. A local listing within the walk's limits takes well
/// under a second.
pub const PROJECT_LISTING_DEADLINE: Duration = Duration::from_secs(10);

/// How long the retained `searchProject` read may take before it says the
/// project is still downloading. A local search within its byte budget takes
/// well under a second.
pub const PROJECT_SEARCH_DEADLINE: Duration = Duration::from_secs(10);

const GENERATED_DIRECTORIES: &[&str] = &[
    ".git",
    ".direnv",
    ".next",
    ".output",
    ".parcel-cache",
    ".svelte-kit",
    ".superpowers",
    ".turbo",
    ".vite",
    ".worktrees",
    "target",
    "node_modules",
    "dist",
    "dist-main",
    "dist-renderer",
    "build",
    "coverage",
    "deriveddata",
    "out",
    "release",
];

const GENERATED_FILES: &[&str] = &[".ds_store", ".eslintcache", "thumbs.db"];

// Not shared with the checkpoint classifier: that copy folds `.envrc` into this
// table, and its leaf rules are prefix/suffix tests rather than these regexes.
const SENSITIVE_BASENAMES: &[&str] = &[
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "auth.json",
    "credentials",
    "credentials.json",
    "application_default_credentials.json",
    "phone-connection.v1.json",
    ".pgpass",
    "pgpass.conf",
    ".my.cnf",
    "my.cnf",
];

static BACKUP_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:(?:\.(?:bak|backup|old|orig|save|tmp|copy|swp|swo|swx|rej|[0-9]+))|~)+$")
        .expect("static backup regex")
});
static DOTENV_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.env([._-]|$)|\.env($|\.)").expect("static dotenv regex"));
static SERVICE_ACCOUNT_JSON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:client[_-]?secret|service[_-]?account).*\.json$|(?:^|[-_.])service[-_]?account(?:[-_.][^/]*)?\.json$")
        .expect("static service account regex")
});
static CREDENTIAL_MANIFEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"credentials?(?:[-_.][^/]*)?\.(?:json|ya?ml|ini)$")
        .expect("static credential manifest regex")
});
static KEY_EXTENSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(?:key|pem|p12|pfx|jks|keystore|ppk)$").expect("static key regex")
});
static PRIVATE_KEY_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^id_(?:rsa|dsa|ecdsa|ed25519)(?:_sk)?(?:\.(?:pub|bak|backup|old|orig|save|tmp|copy|[0-9]+))*$")
        .expect("static private key regex")
});
static SSH_HOST_KEY_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ssh_host_[a-z0-9_-]+_key(?:\.(?:pub|bak|backup|old|orig|save|tmp|copy|[0-9]+))*$")
        .expect("static host key regex")
});
static NAMED_PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:private[-_]?key|identity)(?:\.(?:pub|bak|backup|old|orig|save|tmp|copy|[0-9]+))*$",
    )
    .expect("static named private key regex")
});
static SECRET_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^secrets?(?:\.[^/]*)?$").expect("static secret regex"));

fn credential_stem(segment: &str) -> Cow<'_, str> {
    BACKUP_SUFFIX.replace(segment, "")
}

fn is_credential_leaf_name(name: &str) -> bool {
    name == ".envrc"
        || DOTENV_NAME.is_match(name)
        || SENSITIVE_BASENAMES.contains(&name)
        || SERVICE_ACCOUNT_JSON.is_match(name)
        || CREDENTIAL_MANIFEST.is_match(name)
        || KEY_EXTENSION.is_match(name)
        || PRIVATE_KEY_NAME.is_match(name)
        || SSH_HOST_KEY_NAME.is_match(name)
        || NAMED_PRIVATE_KEY.is_match(name)
        || SECRET_NAME.is_match(name)
}

fn last_path_segment(relative: &str) -> &str {
    relative
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or_default()
}

fn segment_is_sensitive(segment: &str) -> bool {
    let stem = credential_stem(segment);
    let stem: &str = stem.as_ref();
    SENSITIVE_DIRECTORIES.contains(&stem) || is_credential_leaf_name(stem)
}

/// Shared path-name classifier used by listing, search, review, inventory, and
/// prompt-context authorization. It never opens the candidate first.
#[must_use]
pub fn is_search_excluded_path(relative_path: &str, directory: bool) -> bool {
    let normalized = relative_path.replace('\\', "/").to_lowercase();
    let name = last_path_segment(&normalized);
    if directory && GENERATED_DIRECTORIES.contains(&name) {
        return true;
    }
    if !directory && GENERATED_FILES.contains(&name) {
        return true;
    }
    normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .any(segment_is_sensitive)
}

/// True when any segment names credential material. Unlike `is_search_excluded_path`,
/// generated directories (`node_modules`, `target`) are not excluded.
#[must_use]
pub fn is_credential_path(path: &str) -> bool {
    path.replace('\\', "/")
        .to_lowercase()
        .split('/')
        .filter(|s| !s.is_empty())
        .any(segment_is_sensitive)
}

// Walks classify only the leaf; parents were admitted when they were enqueued.
fn is_search_excluded_walk_entry(relative: &str, directory: bool) -> bool {
    is_search_excluded_path(last_path_segment(relative), directory)
}

/// Metadata-only binary/container exclusion shared by search and Quick Open.
#[must_use]
pub fn is_likely_binary_path(relative_path: &str) -> bool {
    Path::new(relative_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            BINARY_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

#[derive(Clone, Debug)]
struct RootSnapshot {
    path: PathBuf,
    identity: FileIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StableFileState {
    identity: FileIdentity,
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

struct OpenedFile {
    file: File,
    state: StableFileState,
}

fn positive_limit(value: Option<usize>, fallback: usize) -> usize {
    value.unwrap_or(fallback).max(1)
}

fn stable_file_state(metadata: &Metadata) -> StableFileState {
    StableFileState {
        identity: identity(metadata),
        len: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
    }
}

fn canonical_project_root(root: &Path) -> Result<RootSnapshot, WorkspaceReadError> {
    if !root.is_absolute() {
        return Err(WorkspaceReadError::UnsafeRoot);
    }
    let before = fs::symlink_metadata(root).map_err(|_| WorkspaceReadError::UnsafeRoot)?;
    if !before.file_type().is_dir() || before.file_type().is_symlink() {
        return Err(WorkspaceReadError::UnsafeRoot);
    }
    let canonical = fs::canonicalize(root).map_err(|_| WorkspaceReadError::UnsafeRoot)?;
    if canonical != root {
        return Err(WorkspaceReadError::UnsafeRoot);
    }
    let after = fs::symlink_metadata(&canonical).map_err(|_| WorkspaceReadError::UnsafeRoot)?;
    if !after.file_type().is_dir()
        || after.file_type().is_symlink()
        || identity(&before) != identity(&after)
    {
        return Err(WorkspaceReadError::UnsafeRoot);
    }
    Ok(RootSnapshot {
        path: canonical,
        identity: identity(&after),
    })
}

fn directory_identity(root: &RootSnapshot, directory: &Path) -> Result<FileIdentity, ()> {
    let metadata = fs::symlink_metadata(directory).map_err(|_| ())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(());
    }
    let canonical = fs::canonicalize(directory).map_err(|_| ())?;
    if canonical != directory || !inside(&root.path, &canonical) {
        return Err(());
    }
    let current_root = fs::symlink_metadata(&root.path).map_err(|_| ())?;
    if !current_root.file_type().is_dir()
        || current_root.file_type().is_symlink()
        || identity(&current_root) != root.identity
    {
        return Err(());
    }
    Ok(identity(&metadata))
}

fn read_verified_directory(
    root: &RootSnapshot,
    directory: &Path,
    hooks: Option<&dyn ProjectWalkHooks>,
    remaining_entries: usize,
) -> Result<(Vec<fs::DirEntry>, bool), ()> {
    let before = directory_identity(root, directory)?;
    if let Some(hooks) = hooks {
        hooks.after_directory_validated(directory);
    }
    // Read at most the remaining budget plus one lookahead. On truncation the
    // filesystem chooses the prefix; sort that prefix for presentation. Never
    // enumerate an unbounded directory merely to choose a lexical prefix.
    let mut reader = fs::read_dir(directory).map_err(|_| ())?;
    let mut entries = reader
        .by_ref()
        .take(remaining_entries)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    let truncated = reader.next().transpose().map_err(|_| ())?.is_some();
    let after = directory_identity(root, directory)?;
    if before != after {
        return Err(());
    }
    entries.sort_by_cached_key(fs::DirEntry::file_name);
    Ok((entries, truncated))
}

fn regular_file_identity(root: &RootSnapshot, file: &Path) -> Result<FileIdentity, ()> {
    let metadata = fs::symlink_metadata(file).map_err(|_| ())?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(());
    }
    let canonical = fs::canonicalize(file).map_err(|_| ())?;
    if canonical != file || !inside(&root.path, &canonical) {
        return Err(());
    }
    Ok(identity(&metadata))
}

fn open_verified_regular(
    root: &RootSnapshot,
    file: &Path,
    hooks: Option<&dyn ProjectWalkHooks>,
) -> Result<OpenedFile, ()> {
    let parent = file.parent().ok_or(())?;
    let parent_before = directory_identity(root, parent)?;
    let before = regular_file_identity(root, file)?;
    if let Some(hooks) = hooks {
        hooks.after_file_validated(file);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let opened = options.open(file).map_err(|_| ())?;
    let opened_metadata = opened.metadata().map_err(|_| ())?;
    let after = regular_file_identity(root, file)?;
    let parent_after = directory_identity(root, parent)?;
    if !opened_metadata.file_type().is_file()
        || before != identity(&opened_metadata)
        || identity(&opened_metadata) != after
        || parent_before != parent_after
    {
        return Err(());
    }
    Ok(OpenedFile {
        file: opened,
        state: stable_file_state(&opened_metadata),
    })
}

/// The size of a file Quick Open may list. Quick Open reads no content, so it
/// proves less than search: the walk already refused links and pinned each
/// directory before and after reading it. One `lstat` names a regular file;
/// a no-follow open and `fstat` prove it is still that file, and readable.
///
/// A file iCloud has evicted (dataless) is listed from its `lstat` alone:
/// opening it would wait for a download Quick Open has no use for.
fn listed_file_len(file: &Path, hooks: Option<&dyn ProjectWalkHooks>) -> Result<u64, ()> {
    let named = fs::symlink_metadata(file).map_err(|_| ())?;
    if !named.file_type().is_file() {
        return Err(());
    }
    if is_dataless(&named) {
        return Ok(named.len());
    }
    if let Some(hooks) = hooks {
        hooks.after_file_validated(file);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    let handle = options
        .open(file)
        .and_then(|opened| opened.metadata())
        .map_err(|_| ())?;
    if !handle.file_type().is_file() || identity(&handle) != identity(&named) {
        return Err(());
    }
    Ok(handle.len())
}

fn verify_opened_unchanged(
    root: &RootSnapshot,
    path: &Path,
    opened: &OpenedFile,
) -> Result<(), ()> {
    let handle = opened.file.metadata().map_err(|_| ())?;
    let named = fs::symlink_metadata(path).map_err(|_| ())?;
    if stable_file_state(&handle) != opened.state
        || stable_file_state(&named) != opened.state
        || regular_file_identity(root, path)? != opened.state.identity
    {
        return Err(());
    }
    Ok(())
}

/// A regular file the walk admitted: not a link, not excluded, and with a
/// UTF-8 path relative to the root.
struct WalkFile {
    absolute: PathBuf,
    relative: String,
}

/// The bounded, link-refusing breadth-first walk Quick Open and search share.
/// Each directory is read with the remaining entry budget; admitted
/// subdirectories are queued and admitted regular files are yielded. The
/// callers keep their own file, byte and result budgets.
struct ProjectWalk<'a> {
    root: RootSnapshot,
    hooks: Option<&'a dyn ProjectWalkHooks>,
    directories: VecDeque<PathBuf>,
    entries: std::vec::IntoIter<fs::DirEntry>,
    max_entries: usize,
    entries_scanned: usize,
    truncated: bool,
}

impl<'a> ProjectWalk<'a> {
    fn open(root: &Path, options: &ProjectWalkOptions<'a>) -> Result<Self, WorkspaceReadError> {
        let root = canonical_project_root(root)?;
        Ok(Self {
            directories: VecDeque::from([root.path.clone()]),
            root,
            hooks: options.hooks,
            entries: Vec::new().into_iter(),
            max_entries: positive_limit(options.max_entries, PROJECT_SEARCH_LIMITS.max_entries),
            entries_scanned: 0,
            truncated: false,
        })
    }

    const fn entries_spent(&self) -> bool {
        self.entries_scanned >= self.max_entries
    }

    /// Marks the walk incomplete. It still yields the rest of the directory
    /// being read, then ends before reading another.
    const fn truncate(&mut self) {
        self.truncated = true;
    }

    /// Yields the next admitted regular file, or `None` once the walk is over.
    /// Before each entry the walk ends truncated when the entry budget or the
    /// caller's `spent` budget is used up; before each directory read it ends
    /// truncated when it is already marked truncated or `spent` holds.
    fn next_file(
        &mut self,
        spent: impl Fn(&Self) -> bool,
    ) -> Result<Option<WalkFile>, WorkspaceReadError> {
        loop {
            let Some(entry) = self.entries.next() else {
                if self.read_next_directory(&spent)? {
                    continue;
                }
                return Ok(None);
            };
            if self.entries_spent() || spent(self) {
                self.truncated = true;
                return Ok(None);
            }
            self.entries_scanned = self.entries_scanned.saturating_add(1);
            let absolute = entry.path();
            let Some(relative) = absolute
                .strip_prefix(&self.root.path)
                .ok()
                .and_then(Path::to_str)
            else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() || is_search_excluded_walk_entry(relative, file_type.is_dir())
            {
                continue;
            }
            if file_type.is_dir() {
                self.directories.push_back(absolute);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let relative = relative.to_owned();
            return Ok(Some(WalkFile { absolute, relative }));
        }
    }

    /// Reads the next readable queued directory into the entry buffer, or
    /// returns false once nothing is queued or the walk is truncated or spent.
    /// An unreadable subdirectory is skipped; an unreadable root fails.
    fn read_next_directory(
        &mut self,
        spent: &impl Fn(&Self) -> bool,
    ) -> Result<bool, WorkspaceReadError> {
        while let Some(directory) = self.directories.pop_front() {
            if self.truncated || spent(self) {
                self.truncated = true;
                return Ok(false);
            }
            match read_verified_directory(
                &self.root,
                &directory,
                self.hooks,
                self.max_entries - self.entries_scanned,
            ) {
                Ok((entries, incomplete)) => {
                    self.truncated |= incomplete;
                    self.entries = entries.into_iter();
                    return Ok(true);
                }
                Err(()) if directory == self.root.path => {
                    return Err(WorkspaceReadError::UnsafeRoot);
                }
                Err(()) => {}
            }
        }
        Ok(false)
    }
}

/// Quick Open's listing with default limits, through the bounded project
/// read: a folder still downloading from iCloud fails with
/// [`WorkspaceReadError::StillDownloading`] after
/// [`PROJECT_LISTING_DEADLINE`] instead of holding the caller.
///
/// # Errors
///
/// [`WorkspaceReadError::UnsafeRoot`], or `StillDownloading` past the deadline.
pub fn list_project_files_bounded(
    root: &Path,
) -> Result<ProjectFileListResponse, WorkspaceReadError> {
    list_bounded(root.to_path_buf(), PROJECT_LISTING_DEADLINE, None)
}

fn list_bounded(
    root: PathBuf,
    deadline: Duration,
    hooks: Option<Arc<dyn ProjectWalkHooks>>,
) -> Result<ProjectFileListResponse, WorkspaceReadError> {
    bounded_read::within(deadline, move || {
        let options = ProjectWalkOptions {
            hooks: hooks.as_deref(),
            ..ProjectWalkOptions::default()
        };
        list_project_files(&root, &options)
    })
    .map_err(|_| WorkspaceReadError::StillDownloading)?
}

/// Project search with default limits, through the bounded project read: a
/// file still downloading from iCloud fails the search with
/// [`WorkspaceReadError::StillDownloading`] after [`PROJECT_SEARCH_DEADLINE`]
/// instead of holding the caller.
///
/// # Errors
///
/// As [`search_project`], or `StillDownloading` past the deadline.
pub fn search_project_bounded(
    root: &Path,
    query: &str,
) -> Result<ProjectSearchResponse, WorkspaceReadError> {
    search_bounded(
        root.to_path_buf(),
        query.to_owned(),
        PROJECT_SEARCH_DEADLINE,
        None,
    )
}

fn search_bounded(
    root: PathBuf,
    query: String,
    deadline: Duration,
    hooks: Option<Arc<dyn ProjectWalkHooks>>,
) -> Result<ProjectSearchResponse, WorkspaceReadError> {
    bounded_read::within(deadline, move || {
        let options = ProjectWalkOptions {
            hooks: hooks.as_deref(),
            ..ProjectWalkOptions::default()
        };
        search_project(&root, &query, &options)
    })
    .map_err(|_| WorkspaceReadError::StillDownloading)?
}

/// Builds a metadata-only Quick Open index without following links or opening
/// credential-shaped and known binary paths.
///
/// # Errors
///
/// Returns an error only when the authorized project root itself is missing,
/// linked, non-canonical, or changes identity during the walk.
pub fn list_project_files(
    root: &Path,
    options: &ProjectWalkOptions<'_>,
) -> Result<ProjectFileListResponse, WorkspaceReadError> {
    let max_files = positive_limit(options.max_files, PROJECT_SEARCH_LIMITS.max_files);
    let max_file_bytes =
        positive_limit(options.max_file_bytes, PROJECT_SEARCH_LIMITS.max_file_bytes);
    let mut walk = ProjectWalk::open(root, options)?;
    let mut files = Vec::new();
    let mut files_scanned = 0_usize;

    // Quick Open checks its file budget before every entry, and stops before
    // reading another directory once either budget is spent.
    while let Some(file) =
        walk.next_file(|walk| files_scanned >= max_files || walk.entries_spent())?
    {
        files_scanned = files_scanned.saturating_add(1);
        if is_likely_binary_path(&file.relative) {
            continue;
        }
        let Ok(len) = listed_file_len(&file.absolute, options.hooks) else {
            continue;
        };
        if len > u64::try_from(max_file_bytes).unwrap_or(u64::MAX) {
            continue;
        }
        let Some(path) = file.absolute.to_str().map(str::to_owned) else {
            continue;
        };
        files.push(ProjectFileEntry {
            path,
            relative_path: file.relative,
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(ProjectFileListResponse {
        files,
        files_scanned: u64::try_from(files_scanned).unwrap_or(u64::MAX),
        entries_scanned: u64::try_from(walk.entries_scanned).unwrap_or(u64::MAX),
        truncated: walk.truncated,
    })
}

/// Literal, Unicode-aware, case-insensitive project search over bounded safe
/// regular files.
///
/// # Errors
///
/// Returns an error for an empty/oversized query or an unsafe changing root.
pub fn search_project(
    root: &Path,
    query: &str,
    options: &ProjectWalkOptions<'_>,
) -> Result<ProjectSearchResponse, WorkspaceReadError> {
    let query_chars = query.encode_utf16().count();
    if query_chars == 0 {
        return Err(WorkspaceReadError::EmptyQuery);
    }
    if query_chars > PROJECT_SEARCH_LIMITS.max_query_chars {
        return Err(WorkspaceReadError::QueryTooLong);
    }
    let pattern = RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .unicode(true)
        .build()
        .map_err(|_| WorkspaceReadError::QueryTooLong)?;
    let max_files = positive_limit(options.max_files, PROJECT_SEARCH_LIMITS.max_files);
    let max_file_bytes =
        positive_limit(options.max_file_bytes, PROJECT_SEARCH_LIMITS.max_file_bytes);
    let max_total_bytes = positive_limit(
        options.max_total_bytes,
        PROJECT_SEARCH_LIMITS.max_total_bytes,
    );
    let max_results = positive_limit(options.max_results, PROJECT_SEARCH_LIMITS.max_results);
    let mut walk = ProjectWalk::open(root, options)?;
    let mut found_matches = Vec::new();
    let mut files_scanned = 0_usize;
    let mut bytes_scanned = 0_usize;

    // Search checks its file and byte budgets only at regular files, and
    // still reads a queued directory once the entry budget is spent.
    while let Some(file) = walk.next_file(|_| false)? {
        if files_scanned >= max_files || bytes_scanned >= max_total_bytes {
            walk.truncate();
            break;
        }
        files_scanned = files_scanned.saturating_add(1);
        if is_likely_binary_path(&file.relative) {
            continue;
        }
        let Ok(mut opened) = open_verified_regular(&walk.root, &file.absolute, options.hooks)
        else {
            continue;
        };
        let Ok(size) = usize::try_from(opened.state.len) else {
            continue;
        };
        if size > max_file_bytes {
            continue;
        }
        let Some(next_total) = bytes_scanned.checked_add(size) else {
            walk.truncate();
            break;
        };
        if next_total > max_total_bytes {
            walk.truncate();
            break;
        }
        let read_ceiling = max_file_bytes.min(max_total_bytes - bytes_scanned);
        let mut data = Vec::with_capacity(size.min(read_ceiling));
        let read_result = opened
            .file
            .by_ref()
            .take(
                u64::try_from(read_ceiling)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut data);
        if read_result.is_err()
            || data.len() > read_ceiling
            || verify_opened_unchanged(&walk.root, &file.absolute, &opened).is_err()
        {
            if read_ceiling < max_file_bytes {
                walk.truncate();
            }
            continue;
        }
        bytes_scanned = bytes_scanned.saturating_add(data.len());
        if data.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&data);
        let Some(path) = file.absolute.to_str() else {
            continue;
        };
        append_matches(path, &text, &pattern, max_results, &mut found_matches);
        if found_matches.len() >= max_results {
            walk.truncate();
            break;
        }
    }

    Ok(ProjectSearchResponse {
        matches: found_matches,
        files_scanned: u64::try_from(files_scanned).unwrap_or(u64::MAX),
        truncated: walk.truncated,
    })
}

fn append_matches(
    file: &str,
    text: &str,
    matcher: &Regex,
    limit: usize,
    output: &mut Vec<ProjectSearchMatch>,
) {
    // Both byte and UTF-16 cursors advance once, even for many matches on one
    // long line. Preview work is bounded and shared by matches on that line.
    let mut scanned = 0;
    let mut line = 1_usize;
    let mut line_start = 0;
    let mut column = 1_usize;
    let mut preview_start = None;
    let mut preview = String::new();
    for found in matcher
        .find_iter(text)
        .take(limit.saturating_sub(output.len()))
    {
        for (offset, character) in text[scanned..found.start()].char_indices() {
            if character == '\n' {
                line += 1;
                line_start = scanned + offset + 1;
                column = 1;
            } else {
                column += character.len_utf16();
            }
        }
        scanned = found.start();
        if preview_start != Some(line_start) {
            let end = text[found.start()..]
                .find('\n')
                .map_or(text.len(), |offset| found.start() + offset);
            preview = preview_for(&text[line_start..end]);
            preview_start = Some(line_start);
        }
        output.push(ProjectSearchMatch {
            path: file.to_owned(),
            line: u64::try_from(line).unwrap_or(u64::MAX),
            column: u64::try_from(column).unwrap_or(u64::MAX),
            preview: preview.clone(),
        });
    }
}

fn preview_for(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut used = 0;
    let mut shortened_end = 0;
    let mut preview = String::new();
    // Expand tabs lazily: a multi-megabyte line needs at most 242 UTF-16 units.
    for character in line.chars().flat_map(|character| {
        std::iter::repeat_n(
            if character == '\t' { ' ' } else { character },
            if character == '\t' { 2 } else { 1 },
        )
    }) {
        used += character.len_utf16();
        if used > 240 {
            preview.truncate(shortened_end);
            preview.push('…');
            return preview;
        }
        preview.push(character);
        if used <= 237 {
            shortened_end = preview.len();
        }
    }
    preview
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn previews_preserve_utf16_tab_and_crlf_boundaries() {
        // Independent legacy formula pins exact bytes around both limits.
        let legacy = |line: &str| {
            let expanded = line.strip_suffix('\r').unwrap_or(line).replace('\t', "  ");
            if expanded.encode_utf16().count() <= 240 {
                return expanded;
            }
            let mut used = 0;
            let mut result = String::new();
            for ch in expanded.chars() {
                if used + ch.len_utf16() > 237 {
                    break;
                }
                used += ch.len_utf16();
                result.push(ch);
            }
            result.push('…');
            result
        };
        for prefix in ["a", "😀", "\t", "e\u{301}"] {
            for length in 0..245 {
                for suffix in ["", "\r", "\t😀z\r", "abcdef"] {
                    let text = format!("{}{suffix}", prefix.repeat(length));
                    assert_eq!(
                        preview_for(&text),
                        legacy(&text),
                        "{prefix:?} {length} {suffix:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn match_locations_and_previews_preserve_unicode_and_multiline_queries() {
        let text = "😀 needle\tneedle\r\nneedle\nneedle 😀\nneedle\tneedle\n";
        for query in ["needle", "needle\nneedle", "\n", "e\tneedle"] {
            let matcher = Regex::new(&regex::escape(query)).unwrap();
            let mut actual = Vec::new();
            append_matches("fixture", text, &matcher, 100, &mut actual);
            let expected: Vec<_> = matcher
                .find_iter(text)
                .map(|found| {
                    let prefix = &text[..found.start()];
                    let start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
                    let end = text[found.start()..]
                        .find('\n')
                        .map_or(text.len(), |offset| found.start() + offset);
                    ProjectSearchMatch {
                        path: "fixture".into(),
                        line: (prefix.bytes().filter(|byte| *byte == b'\n').count() + 1) as u64,
                        column: (text[start..found.start()].encode_utf16().count() + 1) as u64,
                        preview: preview_for(&text[start..end]),
                    }
                })
                .collect();
            assert_eq!(actual, expected, "query {query:?}");
        }
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_dense_long_line_search() {
        let text = format!("{}{}", "needle ".repeat(200), "x".repeat(1024 * 1024));
        let matcher = Regex::new("needle").unwrap();
        let mut timings = Vec::new();
        for _ in 0..5 {
            let started = std::time::Instant::now();
            let mut output = Vec::new();
            append_matches("fixture.txt", &text, &matcher, 200, &mut output);
            assert_eq!(output.len(), 200);
            std::hint::black_box(output);
            timings.push(started.elapsed());
        }
        timings.sort();
        eprintln!(
            "dense long-line search: median {:?}, {} input bytes, 200 matches",
            timings[2],
            text.len()
        );
    }

    #[test]
    fn enumeration_bounds_allocation_and_reports_partial_prefix() {
        let root = project();
        for index in 0..100 {
            fs::write(root.path().join(format!("{index:03}.txt")), "needle").unwrap();
        }
        let snapshot = canonical_project_root(root.path()).unwrap();
        let (entries, truncated) =
            read_verified_directory(&snapshot, root.path(), None, 7).unwrap();
        assert!(truncated);
        assert_eq!(entries.len(), 7);
        let options = ProjectWalkOptions {
            max_entries: Some(7),
            ..ProjectWalkOptions::default()
        };
        let list = list_project_files(root.path(), &options).unwrap();
        let search = search_project(root.path(), "needle", &options).unwrap();
        assert!(list.truncated && search.truncated);
        assert_eq!(list.entries_scanned, 7);
        assert_eq!(search.files_scanned, 7);
        assert!(
            list.files
                .windows(2)
                .all(|pair| pair[0].relative_path < pair[1].relative_path)
        );
    }

    /// Records every directory and file the walk validates, relative to root.
    struct WalkLog {
        root: PathBuf,
        seen: std::sync::Mutex<Vec<String>>,
    }

    impl WalkLog {
        fn note(&self, kind: &str, path: &Path) {
            let relative = path.strip_prefix(&self.root).unwrap().to_str().unwrap();
            let relative = if relative.is_empty() { "." } else { relative };
            self.seen.lock().unwrap().push(format!("{kind}:{relative}"));
        }

        fn take(&self) -> String {
            std::mem::take(&mut *self.seen.lock().unwrap()).join(" ")
        }
    }

    impl ProjectWalkHooks for WalkLog {
        fn after_directory_validated(&self, directory: &Path) {
            self.note("dir", directory);
        }

        fn after_file_validated(&self, file: &Path) {
            self.note("file", file);
        }
    }

    /// Pins where each walk stops and what it reports at every budget edge.
    /// Quick Open checks its file budget before every entry and stops before
    /// reading a directory once any budget is spent; search checks its file
    /// and byte budgets only at regular files and still reads a queued
    /// directory with an empty entry budget, reporting truncation only when
    /// that directory holds an entry.
    #[test]
    fn walk_budgets_stop_each_entry_point_where_they_always_have() {
        let root = project();
        for directory in ["b/d/h", "g"] {
            fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        for file in ["a.txt", "f.txt", "b/c.txt", "b/d/e.txt", "b/logo.png"] {
            fs::write(root.path().join(file), "needle\n").unwrap();
        }
        let log = WalkLog {
            root: root.path().to_path_buf(),
            seen: std::sync::Mutex::new(Vec::new()),
        };
        let relative = |path: &str| {
            Path::new(path)
                .strip_prefix(root.path())
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned()
        };
        // (name, max_files, max_entries, max_total_bytes, max_results)
        let cases = [
            ("default", None, None, None, None),
            ("files=1", Some(1), None, None, None),
            ("files=2", Some(2), None, None, None),
            ("entries=4", None, Some(4), None, None),
            ("entries=5", None, Some(5), None, None),
            ("entries=9", None, Some(9), None, None),
            ("entries=10", None, Some(10), None, None),
            ("bytes=10", None, None, Some(10), None),
            ("results=2", None, None, None, Some(2)),
        ];
        let mut actual = Vec::new();
        for (name, max_files, max_entries, max_total_bytes, max_results) in cases {
            let options = ProjectWalkOptions {
                max_files,
                max_entries,
                max_total_bytes,
                max_results,
                hooks: Some(&log),
                ..ProjectWalkOptions::default()
            };
            let list = list_project_files(root.path(), &options).unwrap();
            actual.push(format!(
                "{name} list [{}] files={} entries={} truncated={} | {}",
                list.files
                    .iter()
                    .map(|file| file.relative_path.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                list.files_scanned,
                list.entries_scanned,
                list.truncated,
                log.take()
            ));
            let search = search_project(root.path(), "needle", &options).unwrap();
            actual.push(format!(
                "{name} search [{}] files={} truncated={} | {}",
                search
                    .matches
                    .iter()
                    .map(|found| relative(&found.path))
                    .collect::<Vec<_>>()
                    .join(" "),
                search.files_scanned,
                search.truncated,
                log.take()
            ));
        }
        let everything =
            "dir:. file:a.txt file:f.txt dir:b file:b/c.txt dir:g dir:b/d file:b/d/e.txt dir:b/d/h";
        let listed = "[a.txt b/c.txt b/d/e.txt f.txt] files=5 entries=9";
        let expected = [
            format!("default list {listed} truncated=false | {everything}"),
            format!("default search [a.txt f.txt b/c.txt b/d/e.txt] files=5 truncated=false | {everything}"),
            "files=1 list [a.txt] files=1 entries=1 truncated=true | dir:. file:a.txt".into(),
            "files=1 search [a.txt] files=1 truncated=true | dir:. file:a.txt".into(),
            "files=2 list [a.txt f.txt] files=2 entries=3 truncated=true | dir:. file:a.txt file:f.txt".into(),
            "files=2 search [a.txt f.txt] files=2 truncated=true | dir:. file:a.txt file:f.txt dir:b".into(),
            "entries=4 list [a.txt f.txt] files=2 entries=4 truncated=true | dir:. file:a.txt file:f.txt".into(),
            "entries=4 search [a.txt f.txt] files=2 truncated=true | dir:. file:a.txt file:f.txt dir:b".into(),
            "entries=5 list [a.txt b/c.txt f.txt] files=3 entries=5 truncated=true | dir:. file:a.txt file:f.txt dir:b file:b/c.txt".into(),
            "entries=5 search [a.txt f.txt b/c.txt] files=3 truncated=true | dir:. file:a.txt file:f.txt dir:b file:b/c.txt".into(),
            // Every entry is read and only the empty b/d/h is left queued:
            // Quick Open stops before it and reports truncation; search reads
            // it with an empty budget, finds nothing and reports none.
            "entries=9 list [a.txt b/c.txt b/d/e.txt f.txt] files=5 entries=9 truncated=true | dir:. file:a.txt file:f.txt dir:b file:b/c.txt dir:g dir:b/d file:b/d/e.txt".into(),
            format!("entries=9 search [a.txt f.txt b/c.txt b/d/e.txt] files=5 truncated=false | {everything}"),
            format!("entries=10 list {listed} truncated=false | {everything}"),
            format!("entries=10 search [a.txt f.txt b/c.txt b/d/e.txt] files=5 truncated=false | {everything}"),
            format!("bytes=10 list {listed} truncated=false | {everything}"),
            "bytes=10 search [a.txt] files=2 truncated=true | dir:. file:a.txt file:f.txt".into(),
            format!("results=2 list {listed} truncated=false | {everything}"),
            "results=2 search [a.txt f.txt] files=2 truncated=true | dir:. file:a.txt file:f.txt".into(),
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn rust_builds_and_nested_worktrees_are_excluded() {
        let root = project();
        for name in ["target", ".worktrees"] {
            fs::create_dir(root.path().join(name)).unwrap();
            fs::write(root.path().join(name).join("copy.rs"), "needle").unwrap();
            assert!(is_search_excluded_path(name, true));
        }
        fs::write(root.path().join("main.rs"), "needle").unwrap();
        let result = search_project(root.path(), "needle", &ProjectWalkOptions::default()).unwrap();
        assert_eq!(result.matches.len(), 1);
        assert!(result.matches[0].path.ends_with("main.rs"));
    }

    use std::os::unix::fs::symlink;

    struct TestProject {
        _temporary: TempDir,
        canonical: PathBuf,
    }

    impl TestProject {
        fn path(&self) -> &Path {
            &self.canonical
        }
    }

    fn project() -> TestProject {
        let temporary = tempfile::tempdir().expect("project");
        let canonical = fs::canonicalize(temporary.path()).expect("canonical project");
        TestProject {
            _temporary: temporary,
            canonical,
        }
    }

    #[test]
    fn finds_literal_text_case_insensitively_with_one_based_locations() {
        let root = project();
        fs::create_dir(root.path().join("src")).expect("src");
        fs::write(
            root.path().join("src/app.ts"),
            "const first = true;\nNeedle + needle[0]\n",
        )
        .expect("source");
        let options = ProjectWalkOptions {
            max_results: Some(10),
            ..ProjectWalkOptions::default()
        };
        let result = search_project(root.path(), "NEEDLE", &options).expect("search");
        assert!(!result.truncated);
        assert_eq!(result.files_scanned, 1);
        assert_eq!(result.matches.len(), 2);
        assert_eq!((result.matches[0].line, result.matches[0].column), (2, 1));
        assert_eq!((result.matches[1].line, result.matches[1].column), (2, 10));
        assert_eq!(result.matches[0].preview, "Needle + needle[0]");
    }

    #[test]
    fn regex_punctuation_is_literal_and_bounds_are_enforced() {
        let root = project();
        fs::write(root.path().join("a.txt"), "a[b] a.b a*b hit hit hit").expect("a");
        fs::write(root.path().join("b.txt"), "hit").expect("b");
        let literal =
            search_project(root.path(), "a.b", &ProjectWalkOptions::default()).expect("literal");
        assert_eq!(literal.matches.len(), 1);
        assert_eq!(literal.matches[0].column, 6);

        let bounded = search_project(
            root.path(),
            "hit",
            &ProjectWalkOptions {
                max_results: Some(2),
                max_files: Some(1),
                max_file_bytes: Some(40),
                max_total_bytes: Some(40),
                ..ProjectWalkOptions::default()
            },
        )
        .expect("bounded");
        assert_eq!(bounded.matches.len(), 2);
        assert_eq!(bounded.files_scanned, 1);
        assert!(bounded.truncated);
    }

    #[test]
    fn excludes_credentials_generated_trees_binaries_and_links() {
        let root = project();
        let outside = project();
        fs::write(root.path().join("visible.ts"), "MARKER\n").expect("visible");
        fs::write(root.path().join(".env.local"), "MARKER\n").expect("env");
        fs::write(root.path().join("service-account.json"), "MARKER\n").expect("secret");
        fs::write(root.path().join("blob.bin"), [0, 77, 65, 82, 75, 69, 82]).expect("binary");
        fs::create_dir(root.path().join("node_modules")).expect("node_modules");
        fs::write(root.path().join("node_modules/package.js"), "MARKER\n").expect("generated");
        fs::create_dir(root.path().join("secret")).expect("secret dir");
        fs::write(root.path().join("secret/development.json"), "MARKER\n").expect("nested");
        fs::write(outside.path().join("outside.txt"), "MARKER\n").expect("outside");
        symlink(
            outside.path().join("outside.txt"),
            root.path().join("linked-file"),
        )
        .expect("file link");
        symlink(outside.path(), root.path().join("linked-directory")).expect("dir link");
        let result =
            search_project(root.path(), "marker", &ProjectWalkOptions::default()).expect("search");
        assert_eq!(result.matches.len(), 1);
        assert!(result.matches[0].path.ends_with("visible.ts"));
    }

    #[test]
    fn quick_open_returns_only_safe_small_source_files() {
        let root = project();
        fs::create_dir(root.path().join("src")).expect("src");
        fs::write(root.path().join("README.md"), "ok").expect("readme");
        fs::write(root.path().join("src/app.ts"), "export {};").expect("app");
        fs::write(root.path().join("credentials.json"), "not indexed").expect("credentials");
        fs::write(root.path().join("image.png"), "not really an image").expect("image");
        fs::write(root.path().join("too-large.txt"), "x".repeat(40)).expect("large");
        let result = list_project_files(
            root.path(),
            &ProjectWalkOptions {
                max_file_bytes: Some(20),
                ..ProjectWalkOptions::default()
            },
        )
        .expect("list");
        assert_eq!(
            result
                .files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["README.md", "src/app.ts"]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn rejects_a_root_symlink_and_file_ancestor_swap() {
        let root = project();
        let holder = project();
        let alias = holder.path().join("alias");
        symlink(root.path(), &alias).expect("root alias");
        assert_eq!(
            list_project_files(&alias, &ProjectWalkOptions::default()),
            Err(WorkspaceReadError::UnsafeRoot)
        );

        let outside = project();
        let source = root.path().join("src");
        fs::create_dir(&source).expect("src");
        fs::write(source.join("target.txt"), "safe").expect("safe");
        fs::write(outside.path().join("target.txt"), "OUTSIDE_MARKER").expect("outside");
        let hook = FileSwapHook {
            source: source.clone(),
            parked: root.path().join("src-parked"),
            outside: outside.path().to_path_buf(),
            target: source.join("target.txt"),
            swapped: AtomicBool::new(false),
        };
        let result = search_project(
            root.path(),
            "OUTSIDE_MARKER",
            &ProjectWalkOptions {
                hooks: Some(&hook),
                ..ProjectWalkOptions::default()
            },
        )
        .expect("race-safe search");
        assert!(hook.swapped.load(Ordering::SeqCst));
        assert!(result.matches.is_empty());
    }

    #[test]
    fn quick_open_skips_a_file_swapped_or_unreadable_after_it_is_named() {
        use std::os::unix::fs::PermissionsExt;

        let root = project();
        let outside = project();
        let source = root.path().join("src");
        fs::create_dir(&source).expect("src");
        fs::write(source.join("target.txt"), "safe").expect("target");
        fs::write(source.join("kept.txt"), "safe").expect("kept");
        fs::write(outside.path().join("target.txt"), "outside").expect("outside");
        fs::write(root.path().join("locked.txt"), "no read").expect("locked");
        fs::set_permissions(
            root.path().join("locked.txt"),
            fs::Permissions::from_mode(0o000),
        )
        .expect("mode 000");
        // The file's directory becomes a link outside the root once named.
        let hook = FileSwapHook {
            source: source.clone(),
            parked: root.path().join("src-parked"),
            outside: outside.path().to_path_buf(),
            target: source.join("target.txt"),
            swapped: AtomicBool::new(false),
        };
        let listed = list_project_files(
            root.path(),
            &ProjectWalkOptions {
                hooks: Some(&hook),
                ..ProjectWalkOptions::default()
            },
        )
        .expect("list");
        assert!(hook.swapped.load(Ordering::SeqCst));
        let names = listed
            .files
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect::<Vec<_>>();
        assert!(!names.contains(&"src/target.txt"), "{names:?}");
        assert!(!names.contains(&"locked.txt"), "{names:?}");

        // A file replaced by a link after it was named is not listed either.
        let root = project();
        fs::write(root.path().join("named.txt"), "safe").expect("named");
        fs::write(outside.path().join("elsewhere.txt"), "outside").expect("elsewhere");
        let swap = LinkSwap(
            root.path().join("named.txt"),
            outside.path().join("elsewhere.txt"),
        );
        let listed = list_project_files(
            root.path(),
            &ProjectWalkOptions {
                hooks: Some(&swap),
                ..ProjectWalkOptions::default()
            },
        )
        .expect("list");
        assert!(listed.files.is_empty());
    }

    #[test]
    fn credential_classifier_matches_all_legacy_name_families() {
        for path in [
            ".env",
            ".envrc",
            ".env.production",
            "experiment.env.local",
            "credentials.json.bak",
            "client_secret.json",
            "id_ed25519_sk",
            "ssh_host_rsa_key",
            "tls.key.bak",
            ".aws/credentials",
            ".docker/config.json",
            ".kube/config",
            ".secrets/runtime.json",
            "credentials.backup/prod.yaml",
            ".pgpass",
            "credentials.ini",
            ".env-production",
            ".env/config.yaml",
            "secrets",
            "src/secrets/runtime.json",
        ] {
            assert!(is_search_excluded_path(path, false), "{path}");
        }
        for path in [
            "src/environment.ts",
            "docs/credentials.md",
            "src/keymap.ts",
            "src/env.ts",
            "env.d.ts",
            "vite-env.d.ts",
        ] {
            assert!(!is_search_excluded_path(path, false), "{path}");
        }
        for directory in [".git", ".direnv", "node_modules", "dist-main", "release"] {
            assert!(is_search_excluded_path(directory, true), "{directory}");
        }
        assert!(is_search_excluded_path(".ssh/config", false));
        assert!(!is_search_excluded_path("config", false));
        assert!(!is_search_excluded_walk_entry(".ssh/config", false));
        assert!(is_likely_binary_path("image.PNG"));
        assert!(is_likely_binary_path("image.png"));
        assert!(!is_likely_binary_path("app.ts"));
    }

    #[test]
    fn credential_path_matches_any_sensitive_segment_without_generated_exclusions() {
        for path in [
            "/Users/x/.ssh/id_ed25519",
            "/p/.env",
            "/p/.env.local",
            "/p/secrets/token.txt",
            "/p/service-account.json",
            "C:\\Users\\x\\.aws\\config",
            "/P/.SSH/config",
        ] {
            assert!(is_credential_path(path), "{path}");
        }
        for path in [
            "/p/src/env.rs",
            "/p/node_modules/x.js",
            "/p/target/debug/app",
            "",
            "/",
        ] {
            assert!(!is_credential_path(path), "{path}");
        }
    }

    #[test]
    fn project_walk_does_not_descend_into_sensitive_directories() {
        let root = project();
        fs::create_dir(root.path().join(".ssh")).expect("ssh");
        fs::write(root.path().join(".ssh/config"), "MARKER\n").expect("config");
        fs::create_dir(root.path().join("src")).expect("src");
        fs::write(root.path().join("src/app.ts"), "MARKER\n").expect("app");
        fs::write(root.path().join("secrets"), "MARKER\n").expect("secrets file");
        let listed = list_project_files(root.path(), &ProjectWalkOptions::default()).expect("list");
        assert_eq!(
            listed
                .files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["src/app.ts"]
        );
        let found =
            search_project(root.path(), "MARKER", &ProjectWalkOptions::default()).expect("search");
        assert_eq!(found.matches.len(), 1);
        assert!(found.matches[0].path.ends_with("src/app.ts"));
    }

    /// Replaces the named file with a link once the walk has named it.
    struct LinkSwap(PathBuf, PathBuf);

    impl ProjectWalkHooks for LinkSwap {
        fn after_file_validated(&self, file: &Path) {
            if file == self.0 {
                fs::remove_file(&self.0).expect("remove named");
                symlink(&self.1, &self.0).expect("link named");
            }
        }
    }

    struct FileSwapHook {
        source: PathBuf,
        parked: PathBuf,
        outside: PathBuf,
        target: PathBuf,
        swapped: AtomicBool,
    }

    impl ProjectWalkHooks for FileSwapHook {
        fn after_file_validated(&self, file: &Path) {
            if file != self.target || self.swapped.swap(true, Ordering::SeqCst) {
                return;
            }
            fs::rename(&self.source, &self.parked).expect("park source");
            symlink(&self.outside, &self.source).expect("swap source");
        }
    }

    /// Stands in for a folder whose files iCloud has evicted: validating a
    /// file opens a FIFO, which blocks forever, as reading a dataless file
    /// waits for its download.
    struct EvictedProject {
        fifo: PathBuf,
    }

    impl ProjectWalkHooks for EvictedProject {
        fn after_file_validated(&self, _file: &Path) {
            let _ = File::open(&self.fifo);
        }
    }

    #[test]
    fn quick_open_says_a_project_still_downloading_from_icloud_instead_of_hanging() {
        let project = TempDir::new().expect("project");
        let root = fs::canonicalize(project.path()).expect("canonical root");
        fs::write(root.join("main.rs"), "fn main() {}").expect("file");
        let outside = TempDir::new().expect("fifo directory");
        let fifo = outside.path().join("download");
        assert!(
            std::process::Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        let started = std::time::Instant::now();
        let stalled = list_bounded(
            root.clone(),
            Duration::from_millis(300),
            Some(Arc::new(EvictedProject { fifo: fifo.clone() })),
        );
        assert_eq!(stalled.err(), Some(WorkspaceReadError::StillDownloading));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            WorkspaceReadError::StillDownloading
                .to_string()
                .contains("still downloading from iCloud")
        );
        // A project on disk lists as before through the same bounded read.
        let listed = list_project_files_bounded(&root).expect("listing");
        assert_eq!(listed.files.len(), 1);
        let _ = fs::OpenOptions::new().write(true).open(&fifo);
    }

    #[test]
    fn project_search_says_a_project_still_downloading_from_icloud_instead_of_hanging() {
        let project = TempDir::new().expect("project");
        let root = fs::canonicalize(project.path()).expect("canonical root");
        fs::write(root.join("main.rs"), "fn needle() {}").expect("file");
        let outside = TempDir::new().expect("fifo directory");
        let fifo = outside.path().join("download");
        assert!(
            std::process::Command::new("/usr/bin/mkfifo")
                .arg(&fifo)
                .status()
                .expect("mkfifo")
                .success()
        );
        let started = std::time::Instant::now();
        let stalled = search_bounded(
            root.clone(),
            "needle".to_owned(),
            Duration::from_millis(300),
            Some(Arc::new(EvictedProject { fifo: fifo.clone() })),
        );
        assert_eq!(stalled.err(), Some(WorkspaceReadError::StillDownloading));
        assert!(started.elapsed() < Duration::from_secs(3));
        // A project on disk searches as before through the same bounded read,
        // and a bad query is still refused as one.
        let found = search_project_bounded(&root, "needle").expect("search");
        assert_eq!(found.matches.len(), 1);
        assert_eq!(
            search_project_bounded(&root, "").err(),
            Some(WorkspaceReadError::EmptyQuery)
        );
        let _ = fs::OpenOptions::new().write(true).open(&fifo);
    }
}
