//! Explicit-file delegation packets. Preparation copies current source bytes;
//! collection returns proposed changes and never writes into the source tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::editor::sha256_hex as digest;
use crate::fsid::{FileIdentity, identity};
use crate::{is_likely_binary_path, is_search_excluded_path};

const MAX_FILES: usize = 48;
const MAX_FILE_BYTES: usize = 128 * 1024;
const MAX_PACKET_BYTES: usize = 2 * 1024 * 1024;
// JSON can escape one source byte as six bytes. 128 KiB leaves room inside
// the 1 MiB durable result for the worker's bounded summary and metadata.
const MAX_RESULT_BYTES: usize = 128 * 1024;
const MAX_PATH_BYTES: usize = 1024;
const MAX_COMPONENTS: usize = 32;

/// One proposed file change. A missing `after` means deletion. Modes contain
/// only ordinary rwx bits; the caller must review and apply results separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub path: String,
    #[serde(default)]
    pub before: Option<String>,
    pub before_sha256: Option<String>,
    pub after: Option<String>,
    pub source_changed: bool,
    pub before_mode: Option<u32>,
    pub after_mode: Option<u32>,
}

#[path = "delegation_apply.rs"]
mod apply;
pub use apply::apply_changes;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Contents {
    text: String,
    mode: u32,
}

#[derive(Debug)]
struct Entry {
    writable: bool,
    before: Option<Contents>,
}

#[derive(Debug)]
struct Directory {
    path: PathBuf,
    identity: FileIdentity,
}

/// A bounded in-memory baseline plus its private on-disk worker workspace.
/// The host owns lifetime, worker shutdown, and freezing the collected result.
#[derive(Debug)]
pub struct Packet {
    source: Directory,
    workspace: Directory,
    entries: BTreeMap<String, Entry>,
}

#[path = "delegation_proposal.rs"]
mod proposal;
pub use proposal::ProposedFile;

impl Packet {
    /// Returns the captured read-only inputs, rechecking the live source before
    /// admission. The caller hashes and freezes this value before dispatch.
    /// # Errors
    /// Refuses changed roots, changed inputs or a writable packet.
    pub fn read_only_contents(&self) -> Result<BTreeMap<String, String>, String> {
        assert_directory(&self.source)?;
        let mut files = BTreeMap::new();
        for (path, entry) in &self.entries {
            if entry.writable || read_file(&self.source, path)? != entry.before {
                return Err("read-only packet changed during capture".into());
            }
            files.insert(
                path.clone(),
                entry.before.as_ref().ok_or("missing input")?.text.clone(),
            );
        }
        assert_directory(&self.source)?;
        Ok(files)
    }

    #[must_use]
    pub fn workspace(&self) -> &Path {
        &self.workspace.path
    }

    /// Collects changes after the worker has stopped. Unrequested outputs or
    /// context edits refuse the entire result, including otherwise valid edits.
    ///
    /// # Errors
    /// Refuses unsafe paths, changed roots, out-of-scope changes, invalid text,
    /// or exceeded bounds. No source file is modified, even on success.
    pub fn collect(&self) -> Result<Vec<Change>, String> {
        assert_directory(&self.source)?;
        assert_directory(&self.workspace)?;
        inspect_outputs(&self.workspace, &self.entries)?;
        let mut changes = Vec::new();
        let mut output_bytes = 0;
        let mut packet_bytes = 0;
        for (path, entry) in &self.entries {
            let after = read_file(&self.workspace, path)?;
            add_bytes(&mut packet_bytes, after.as_ref(), MAX_PACKET_BYTES)?;
            if after == entry.before {
                continue;
            }
            if !entry.writable {
                return Err(format!("worker changed a context file: {path}"));
            }
            add_bytes(&mut output_bytes, after.as_ref(), MAX_RESULT_BYTES)?;
            let current = read_file(&self.source, path)?;
            changes.push(Change {
                path: path.clone(),
                before: entry.before.as_ref().map(|c| c.text.clone()),
                before_sha256: entry.before.as_ref().map(|value| digest(&value.text)),
                after: after.as_ref().map(|value| value.text.clone()),
                source_changed: current != entry.before,
                before_mode: entry.before.as_ref().map(|value| value.mode),
                after_mode: after.map(|value| value.mode),
            });
        }
        // Recheck the directory inventory after reading; lifecycle must keep
        // the worker stopped throughout collection.
        inspect_outputs(&self.workspace, &self.entries)?;
        assert_directory(&self.source)?;
        Ok(changes)
    }
}

/// Copies only explicitly named files into a new private packet directory.
/// Write files may be absent; context files must exist. Existing rwx bits are
/// retained in the workspace. The baseline stays in memory: nothing reads an
/// on-disk copy of it.
///
/// # Errors
/// Refuses invalid/duplicate paths, unsafe filesystem objects, missing context,
/// oversized packets, existing destinations, or incomplete writes. The caller
/// owns removal of an incomplete destination after a failure.
pub fn prepare(
    source: &Path,
    destination: &Path,
    write_files: &[String],
    context_files: &[String],
) -> Result<Packet, String> {
    let names = validate_names(write_files, context_files)?;
    prepare_names(source, destination, names)
}

/// Captures only existing context files; never admits a writable path.
/// An empty packet is useful for public or synthetic inline analysis.
/// # Errors
/// Refuses unsafe paths, duplicates, missing inputs or unstable source bytes.
pub fn prepare_read_only(
    source: &Path,
    destination: &Path,
    files: &[String],
) -> Result<Packet, String> {
    let names = validate_read_only_names(files)?;
    prepare_names(source, destination, names)
}

/// Reads the named context files without writing a packet, for callers that
/// only need the frozen text. Each file is read twice around the directory
/// checks, the same unchanged-source recheck [`Packet::read_only_contents`]
/// makes, so a source that changes during capture is refused.
/// # Errors
/// Refuses unsafe paths, duplicates, missing inputs, oversized input or
/// unstable source bytes.
pub fn capture_read_only(
    source: &Path,
    files: &[String],
) -> Result<BTreeMap<String, String>, String> {
    let names = validate_read_only_names(files)?;
    let source = directory(source)?;
    let mut total_bytes = 0;
    let mut captured = Vec::with_capacity(names.len());
    for name in names.into_keys() {
        let before =
            read_file(&source, &name)?.ok_or_else(|| format!("context file is missing: {name}"))?;
        add_bytes(&mut total_bytes, Some(&before), MAX_PACKET_BYTES)?;
        captured.push((name, before));
    }
    assert_directory(&source)?;
    let mut contents = BTreeMap::new();
    for (name, before) in captured {
        if read_file(&source, &name)?.as_ref() != Some(&before) {
            return Err("read-only packet changed during capture".into());
        }
        contents.insert(name, before.text);
    }
    assert_directory(&source)?;
    Ok(contents)
}

fn validate_read_only_names(files: &[String]) -> Result<BTreeMap<String, bool>, String> {
    if files.len() > MAX_FILES {
        return Err("at most 48 context files are allowed".into());
    }
    let mut names = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for path in files {
        validate_path(path)?;
        if !folded.insert(path.to_lowercase()) {
            return Err("duplicate context path".into());
        }
        names.insert(path.clone(), false);
    }
    Ok(names)
}

fn prepare_names(
    source: &Path,
    destination: &Path,
    names: BTreeMap<String, bool>,
) -> Result<Packet, String> {
    let source = directory(source)?;
    let parent = directory(
        destination
            .parent()
            .ok_or_else(|| "packet destination must have an existing parent".to_owned())?,
    )?;
    if !destination.is_absolute()
        || destination.file_name().is_none()
        || destination.starts_with(&source.path)
        || source.path.starts_with(destination)
        || destination.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err("packet destination must be absolute and separate from the source".into());
    }
    let mut entries = BTreeMap::new();
    let mut total_bytes = 0;
    let mut writable_bytes = 0;
    for (name, writable) in names {
        let before = read_file(&source, &name)?;
        if !writable && before.is_none() {
            return Err(format!("context file is missing: {name}"));
        }
        add_bytes(&mut total_bytes, before.as_ref(), MAX_PACKET_BYTES)?;
        if writable {
            add_bytes(&mut writable_bytes, before.as_ref(), MAX_RESULT_BYTES)?;
        }
        entries.insert(name, Entry { writable, before });
    }
    assert_directory(&source)?;
    assert_directory(&parent)?;
    let destination = create_directory(destination)?;
    assert_directory(&parent)?;
    let workspace = create_directory(&destination.path.join("workspace"))?;
    for (path, entry) in &entries {
        make_parents(&workspace, path)?;
        if let Some(contents) = &entry.before {
            write_scratch_file(&workspace, path, contents, contents.mode)?;
        }
    }
    assert_directory(&destination)?;
    Ok(Packet {
        source,
        workspace,
        entries,
    })
}

fn validate_names(
    write_files: &[String],
    context_files: &[String],
) -> Result<BTreeMap<String, bool>, String> {
    if write_files.is_empty() || write_files.len() + context_files.len() > MAX_FILES {
        return Err("name at least one write file and at most 48 files in total".into());
    }
    let mut names = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for (path, writable) in write_files
        .iter()
        .map(|path| (path, true))
        .chain(context_files.iter().map(|path| (path, false)))
    {
        validate_path(path)?;
        let lower = path.to_lowercase();
        if !folded.insert(lower.clone()) {
            return Err("each packet file must be named only once, ignoring case".into());
        }
        names.insert(path.clone(), writable);
    }
    for path in &folded {
        let mut prefix = String::new();
        for part in parent_parts(path) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if folded.contains(&prefix) {
                return Err("a packet file cannot also be another file's directory".into());
            }
        }
    }
    Ok(names)
}

/// The directory segments above a packet path's leaf; none for a bare name.
fn parent_parts(path: &str) -> impl Iterator<Item = &str> {
    path.rsplit_once('/')
        .into_iter()
        .flat_map(|(parent, _)| parent.split('/'))
}

fn validate_path(path: &str) -> Result<(), String> {
    let parts: Vec<_> = path.split('/').collect();
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || parts.len() > MAX_COMPONENTS
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || parts.iter().any(|part| {
            part.is_empty() || matches!(*part, "." | "..") || part.contains(['*', '?', '[', ']'])
        })
    {
        return Err("packet paths must be normalized relative literal file paths".into());
    }
    if is_search_excluded_path(path, false)
        || is_likely_binary_path(path)
        || parts.iter().any(|part| is_search_excluded_path(part, true))
    {
        return Err("credential, generated, and binary paths are not allowed in packets".into());
    }
    Ok(())
}

fn add_bytes(total: &mut usize, value: Option<&Contents>, maximum: usize) -> Result<(), String> {
    *total += value.map_or(0, |value| value.text.len());
    if *total > maximum {
        return Err(format!("packet content exceeds the {maximum} byte limit"));
    }
    Ok(())
}

fn directory(path: &Path) -> Result<Directory, String> {
    let before = fs::symlink_metadata(path).map_err(|_| "packet directory is unavailable")?;
    if !path.is_absolute() || !before.is_dir() || before.file_type().is_symlink() {
        return Err("packet directories must be real absolute directories".into());
    }
    let canonical = fs::canonicalize(path).map_err(|_| "packet directory is unavailable")?;
    let after = fs::symlink_metadata(path).map_err(|_| "packet directory changed")?;
    if canonical != path || !after.is_dir() || identity(&before) != identity(&after) {
        return Err("packet directory changed or crossed a symbolic link".into());
    }
    Ok(Directory {
        path: path.to_owned(),
        identity: identity(&after),
    })
}

fn assert_directory(expected: &Directory) -> Result<(), String> {
    if directory(&expected.path)?.identity != expected.identity {
        return Err("packet directory was replaced".into());
    }
    Ok(())
}

/// Verifies every existing ancestor, including before a missing new file.
fn parents(root: &Directory, relative: &str) -> Result<Vec<Directory>, String> {
    assert_directory(root)?;
    let mut verified = Vec::new();
    let mut path = root.path.clone();
    for part in parent_parts(relative) {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(_) => verified.push(directory(&path)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err("packet file's parent is unavailable".into()),
        }
    }
    Ok(verified)
}

fn assert_parents(root: &Directory, parents: &[Directory]) -> Result<(), String> {
    assert_directory(root)?;
    for parent in parents {
        assert_directory(parent)?;
    }
    Ok(())
}

#[derive(Eq, PartialEq)]
struct FileState {
    identity: FileIdentity,
    length: u64,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn file_state(metadata: &Metadata) -> FileState {
    FileState {
        identity: identity(metadata),
        length: metadata.len(),
        mode: metadata.mode(),
        links: metadata.nlink(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

fn read_file(root: &Directory, relative: &str) -> Result<Option<Contents>, String> {
    validate_path(relative)?;
    let verified = parents(root, relative)?;
    let path = root.path.join(relative);
    let before = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert_parents(root, &verified)?;
            return Ok(None);
        }
        Err(_) => return Err(format!("packet file is unavailable: {relative}")),
    };
    if !before.is_file() || before.file_type().is_symlink() || before.nlink() != 1 {
        return Err(format!(
            "packet file must be a regular file without links: {relative}"
        ));
    }
    if before.len() > MAX_FILE_BYTES as u64 {
        return Err(format!("packet file exceeds 128 KiB: {relative}"));
    }
    if fs::canonicalize(&path).map_err(|_| "packet file changed")? != path {
        return Err("packet file crossed a symbolic link".into());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|_| "packet file changed before reading")?;
    let opened = file.metadata().map_err(|_| "packet file is unavailable")?;
    if !opened.is_file() || file_state(&opened) != file_state(&before) {
        return Err("packet file changed before reading".into());
    }
    assert_parents(root, &verified)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "packet file could not be read")?;
    let final_open = file.metadata().map_err(|_| "packet file changed")?;
    let final_path = fs::symlink_metadata(&path).map_err(|_| "packet file changed")?;
    assert_parents(root, &verified)?;
    if bytes.len() > MAX_FILE_BYTES
        || file_state(&final_open) != file_state(&opened)
        || file_state(&final_path) != file_state(&opened)
        || fs::canonicalize(&path).map_err(|_| "packet file changed")? != path
    {
        return Err("packet file changed while reading or exceeded its byte limit".into());
    }
    if bytes.contains(&0) {
        return Err(format!("packet file is not plain UTF-8 text: {relative}"));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("packet file is not plain UTF-8 text: {relative}"))?;
    Ok(Some(Contents {
        text,
        mode: opened.mode() & 0o777,
    }))
}

fn create_directory(path: &Path) -> Result<Directory, String> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|_| "packet directory already exists or could not be created")?;
    directory(path)
}

fn make_parents(root: &Directory, relative: &str) -> Result<(), String> {
    assert_directory(root)?;
    let mut path = root.path.clone();
    for part in parent_parts(relative) {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                directory(&path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_directory(&path)?;
            }
            Err(_) => return Err("packet directory could not be created".into()),
        }
    }
    assert_directory(root)
}

/// Creates a file in the source tree durably (an applied proposal).
fn write_file(root: &Directory, path: &str, contents: &Contents, mode: u32) -> Result<(), String> {
    create_file(root, path, contents, mode, true)
}

/// Creates a file in a disposable packet or validation snapshot. Nothing
/// resumes these after a crash, so they skip the per-file drive flush.
fn write_scratch_file(
    root: &Directory,
    path: &str,
    contents: &Contents,
    mode: u32,
) -> Result<(), String> {
    create_file(root, path, contents, mode, false)
}

fn create_file(
    root: &Directory,
    path: &str,
    contents: &Contents,
    mode: u32,
    durable: bool,
) -> Result<(), String> {
    let verified = parents(root, path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.path.join(path))
        .map_err(|_| "packet file already exists or could not be created")?;
    assert_parents(root, &verified)?;
    file.write_all(contents.text.as_bytes())
        .map_err(|_| "packet copy failed")?;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| "packet file permissions could not be set")?;
    if durable {
        file.sync_all()
            .map_err(|_| "packet file could not be synced")?;
    }
    assert_parents(root, &verified)
}

fn inspect_outputs(root: &Directory, entries: &BTreeMap<String, Entry>) -> Result<(), String> {
    let mut directories = BTreeSet::new();
    for name in entries.keys() {
        let mut parent = Path::new(name).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            directories.insert(path.to_path_buf());
            parent = path.parent();
        }
    }
    let mut pending = vec![root.path.clone()];
    let mut count = 0;
    while let Some(path) = pending.pop() {
        let before = directory(&path)?;
        for row in fs::read_dir(&path).map_err(|_| "worker output directory is unavailable")? {
            count += 1;
            if count > MAX_FILES * MAX_COMPONENTS {
                return Err("worker created too many output entries".into());
            }
            let row = row.map_err(|_| "worker output entry is unavailable")?;
            let absolute = row.path();
            let relative = absolute
                .strip_prefix(&root.path)
                .map_err(|_| "worker output escaped its workspace")?;
            let name = relative.to_str().ok_or("worker output path is not UTF-8")?;
            validate_path(name)?;
            let metadata = fs::symlink_metadata(&absolute).map_err(|_| "worker output changed")?;
            if metadata.file_type().is_symlink() {
                return Err("worker output contains a symbolic link".into());
            }
            if metadata.is_dir() && directories.contains(relative) {
                pending.push(absolute);
            } else if !metadata.is_file() || !entries.contains_key(name) {
                return Err(format!("worker created an out-of-scope output: {name}"));
            }
        }
        assert_directory(&before)?;
    }
    assert_directory(root)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn parent_parts_are_every_segment_but_the_leaf() {
        for path in ["", "a", "a/b", "a/b/c", "a/", "/a", "a//b", "//", "a/b/"] {
            let dropped_leaf = path.split('/').take(path.split('/').count() - 1);
            assert_eq!(
                parent_parts(path).collect::<Vec<_>>(),
                dropped_leaf.collect::<Vec<_>>(),
                "{path:?}"
            );
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        source: PathBuf,
        destination: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let source = root.join("source");
            fs::create_dir(&source).unwrap();
            Self {
                _temp: temp,
                source,
                destination: root.join("packet"),
            }
        }

        fn write(&self, path: &str, text: &str) {
            let target = self.source.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text).unwrap();
        }

        fn prepare(&self, writes: &[&str], context: &[&str]) -> Result<Packet, String> {
            prepare(
                &self.source,
                &self.destination,
                &writes
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>(),
                &context
                    .iter()
                    .map(|value| (*value).to_owned())
                    .collect::<Vec<_>>(),
            )
        }
    }

    #[test]
    fn current_dirty_bytes_are_the_baseline_and_parent_edits_are_reported() {
        let fixture = Fixture::new();
        fixture.write("src/task.rs", "parent's uncommitted bytes");
        fixture.write("README.md", "context");
        let packet = fixture.prepare(&["src/task.rs"], &["README.md"]).unwrap();
        assert_eq!(
            fs::read_to_string(packet.workspace().join("src/task.rs")).unwrap(),
            "parent's uncommitted bytes"
        );
        assert!(packet.collect().unwrap().is_empty());
        fixture.write("src/task.rs", "parent continued independently");
        fs::write(packet.workspace().join("src/task.rs"), "worker result").unwrap();
        let changes = packet.collect().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].before_sha256,
            Some(digest("parent's uncommitted bytes"))
        );
        assert_eq!(changes[0].after.as_deref(), Some("worker result"));
        assert!(changes[0].source_changed);
        assert_eq!(
            fs::read_to_string(fixture.source.join("src/task.rs")).unwrap(),
            "parent continued independently"
        );
        let wire = serde_json::to_value(&changes[0]).unwrap();
        assert_eq!(wire["sourceChanged"], true);
        assert!(wire.get("beforeSha256").is_some());
    }

    #[test]
    fn creates_untracked_files_and_reports_deletions_without_touching_source() {
        let fixture = Fixture::new();
        fixture.write("old.txt", "delete me");
        let packet = fixture.prepare(&["old.txt", "new/note.txt"], &[]).unwrap();
        fs::remove_file(packet.workspace().join("old.txt")).unwrap();
        fs::write(packet.workspace().join("new/note.txt"), "created").unwrap();
        let changes = packet.collect().unwrap();
        assert_eq!(changes.len(), 2);
        let created = changes
            .iter()
            .find(|change| change.path == "new/note.txt")
            .unwrap();
        assert_eq!(created.before_sha256, None);
        assert_eq!(created.after.as_deref(), Some("created"));
        assert!(!created.source_changed);
        let deleted = changes
            .iter()
            .find(|change| change.path == "old.txt")
            .unwrap();
        assert_eq!(deleted.after, None);
        assert!(!deleted.source_changed);
        assert!(fixture.source.join("old.txt").exists());
        assert!(!fixture.source.join("new/note.txt").exists());
    }

    #[test]
    fn snapshots_untracked_context_and_rejects_context_edits_or_deletion() {
        for delete in [false, true] {
            let fixture = Fixture::new();
            fixture.write("context.txt", "read only");
            let packet = fixture.prepare(&["new.txt"], &["context.txt"]).unwrap();
            let context = packet.workspace().join("context.txt");
            if delete {
                fs::remove_file(context).unwrap();
            } else {
                fs::write(context, "modified").unwrap();
            }
            assert!(packet.collect().unwrap_err().contains("context file"));
        }
    }

    #[test]
    fn rejects_missing_context_existing_destination_and_ambiguous_names() {
        let fixture = Fixture::new();
        assert!(fixture.prepare(&["a.txt"], &["missing.txt"]).is_err());
        assert!(!fixture.destination.exists());
        for names in [vec!["A.txt", "a.txt"], vec!["a", "a/b.txt"]] {
            assert!(fixture.prepare(&names, &[]).is_err());
        }
        fixture.prepare(&["a.txt"], &[]).unwrap();
        assert!(fixture.prepare(&["a.txt"], &[]).is_err());
    }

    #[test]
    fn rejects_sensitive_generated_binary_and_nonliteral_names_before_creating_packet() {
        let fixture = Fixture::new();
        // These are path strings only. No credential files are created or read.
        for path in [
            ".env",
            ".env.local",
            "keys/client.key",
            "service-account.json",
            "credentials/config.txt",
            "node_modules/a.js",
            "src/target/result.txt",
            ".git",
            "photo.png",
            "../escape.txt",
            "/absolute.txt",
            "a//b",
            "a/./b",
            "a\\b",
            "src/*.rs",
            "src/[ab].rs",
            "new\nfile",
        ] {
            assert!(fixture.prepare(&[path], &[]).is_err(), "accepted {path}");
            assert!(!fixture.destination.exists());
        }
    }

    #[test]
    fn refuses_symlink_files_ancestors_roots_and_hard_links() {
        let fixture = Fixture::new();
        fixture.write("real/note.txt", "original");
        symlink(fixture.source.join("real"), fixture.source.join("linked")).unwrap();
        symlink(
            fixture.source.join("real/note.txt"),
            fixture.source.join("link.txt"),
        )
        .unwrap();
        for path in ["link.txt", "linked/note.txt", "linked/new.txt"] {
            assert!(fixture.prepare(&[path], &[]).is_err());
        }
        fs::hard_link(
            fixture.source.join("real/note.txt"),
            fixture.source.join("hard.txt"),
        )
        .unwrap();
        assert!(fixture.prepare(&["hard.txt"], &[]).is_err());
        let alias = fixture.source.parent().unwrap().join("source-alias");
        symlink(&fixture.source, &alias).unwrap();
        assert!(prepare(&alias, &fixture.destination, &["new.txt".into()], &[]).is_err());
    }

    #[test]
    fn refuses_worker_symlink_escape_and_unrequested_outputs() {
        for path in ["extra.txt", "unrequested/extra.txt", "node_modules/a.js"] {
            let fixture = Fixture::new();
            let packet = fixture.prepare(&["allowed.txt"], &[]).unwrap();
            let target = packet.workspace().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, "unrequested").unwrap();
            assert!(packet.collect().is_err(), "accepted {path}");
        }
        let fixture = Fixture::new();
        fixture.write("outside.txt", "parent");
        let packet = fixture.prepare(&["allowed.txt"], &[]).unwrap();
        symlink(
            fixture.source.join("outside.txt"),
            packet.workspace().join("allowed.txt"),
        )
        .unwrap();
        assert!(packet.collect().is_err());
    }

    #[test]
    fn limits_files_individual_content_total_packet_and_result_bytes() {
        let fixture = Fixture::new();
        let names: Vec<String> = (0..49).map(|number| format!("file{number}.txt")).collect();
        assert!(prepare(&fixture.source, &fixture.destination, &names, &[]).is_err());
        fixture.write("large.txt", &"x".repeat(MAX_FILE_BYTES + 1));
        assert!(fixture.prepare(&["large.txt"], &[]).is_err());
        assert!(!fixture.destination.exists());

        let fixture = Fixture::new();
        let names: Vec<String> = (0..17).map(|number| format!("file{number}.txt")).collect();
        for name in &names {
            fixture.write(name, &"x".repeat(MAX_FILE_BYTES));
        }
        assert!(
            prepare(
                &fixture.source,
                &fixture.destination,
                &["new.txt".into()],
                &names,
            )
            .is_err()
        );

        let fixture = Fixture::new();
        fixture.write("one.txt", &"x".repeat(MAX_RESULT_BYTES));
        fixture.write("two.txt", "one byte too many");
        assert!(fixture.prepare(&["one.txt", "two.txt"], &[]).is_err());
        assert!(!fixture.destination.exists());

        let packet = fixture.prepare(&["one.txt"], &["two.txt"]).unwrap();
        assert!(packet.collect().unwrap().is_empty());

        let fixture = Fixture::new();
        let names: Vec<String> = (0..5).map(|number| format!("file{number}.txt")).collect();
        let packet = prepare(&fixture.source, &fixture.destination, &names, &[]).unwrap();
        for name in &names {
            fs::write(packet.workspace().join(name), "x".repeat(MAX_FILE_BYTES)).unwrap();
        }
        assert!(packet.collect().unwrap_err().contains("byte limit"));
        fs::write(
            packet.workspace().join(&names[0]),
            "x".repeat(MAX_FILE_BYTES + 1),
        )
        .unwrap();
        assert!(packet.collect().unwrap_err().contains("128 KiB"));
    }

    #[test]
    fn read_only_capture_returns_the_text_without_writing_a_packet() {
        let fixture = Fixture::new();
        fixture.write("notes/a.md", "alpha\n");
        fixture.write("b.txt", "beta");
        let files = vec!["notes/a.md".to_owned(), "b.txt".to_owned()];
        let captured = capture_read_only(&fixture.source, &files).unwrap();
        assert_eq!(captured["notes/a.md"], "alpha\n");
        assert_eq!(captured["b.txt"], "beta");
        assert!(!fixture.destination.exists());
        let missing = vec!["gone.txt".to_owned()];
        assert!(
            capture_read_only(&fixture.source, &missing)
                .unwrap_err()
                .contains("missing")
        );
        let duplicate = vec!["b.txt".to_owned(), "B.txt".to_owned()];
        assert!(capture_read_only(&fixture.source, &duplicate).is_err());
    }

    #[test]
    fn preserves_modes_and_reports_mode_only_edits() {
        let fixture = Fixture::new();
        fixture.write("run.sh", "#!/bin/sh\n");
        fs::set_permissions(
            fixture.source.join("run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let packet = fixture.prepare(&["run.sh"], &[]).unwrap();
        for path in [&fixture.destination, &packet.workspace().to_owned()] {
            assert_eq!(fs::metadata(path).unwrap().mode() & 0o777, 0o700);
        }
        assert!(!fixture.destination.join("baseline").exists());
        assert_eq!(
            fs::metadata(packet.workspace().join("run.sh"))
                .unwrap()
                .mode()
                & 0o777,
            0o755
        );
        fs::set_permissions(
            packet.workspace().join("run.sh"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let change = packet.collect().unwrap().remove(0);
        assert_eq!(change.before_mode, Some(0o755));
        assert_eq!(change.after_mode, Some(0o644));
        assert!(!change.source_changed);
    }

    #[test]
    fn rejects_non_text_bytes_and_replaced_workspace() {
        let fixture = Fixture::new();
        for bytes in [vec![0], vec![0xff]] {
            fs::write(fixture.source.join("invalid.txt"), bytes).unwrap();
            assert!(fixture.prepare(&["invalid.txt"], &[]).is_err());
        }
        let packet = fixture.prepare(&["new.txt"], &[]).unwrap();
        fs::rename(
            packet.workspace(),
            fixture.destination.join("original-workspace"),
        )
        .unwrap();
        fs::create_dir(packet.workspace()).unwrap();
        assert!(packet.collect().is_err());
    }
}

#[path = "delegation_validation.rs"]
mod validation;
pub use validation::{ValidationSnapshot, prepare_validation, verify_inputs};
