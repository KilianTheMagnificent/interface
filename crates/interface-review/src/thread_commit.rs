//! Explicit commits from an immutable review. The real index is locked and
//! preserved; hooks run against a private selected index before the branch CAS.
use std::{
    collections::BTreeSet,
    fs,
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use interface_contracts::threads::{
    CommitChoice, CommitReview, CommitReviewFile, CommitReviewHunk, DiffFileStatus, DiffOmitted,
};
use sha2::{Digest, Sha256};

use super::ship::{ShipEffect, redact_credentials, working_tree_commit_blocked};
use super::{
    Call, PATCH_WORKERS, Patch, ThreadGitError, changes_between, classified, failed, git,
    git_with_input, head_commit, require_repo, working_tree,
};

const MAX_PATCH: usize = 256 * 1024;
const MAX_TOTAL: usize = 2 * 1024 * 1024;
const MAX_INDEX: u64 = 64 * 1024 * 1024;

struct Snapshot {
    repo: PathBuf,
    head: String,
    branch: String,
    index_path: PathBuf,
    index_bytes: Vec<u8>,
    index_tree: String,
    work: String,
    revision: String,
    files: Vec<ReviewedFile>,
}

struct ReviewedFile {
    view: CommitReviewFile,
    patch: String,
    header: String,
    hunks: Vec<String>,
}

fn invalid(message: impl Into<String>) -> ThreadGitError {
    ThreadGitError::new(ThreadGitError::INVALID_CODE, message)
}

#[allow(clippy::needless_pass_by_value)] // map_err consumes the error.
fn io_error(error: std::io::Error) -> ThreadGitError {
    ThreadGitError::new(
        ThreadGitError::FAILED_CODE,
        format!("The commit's private index could not be prepared ({error})."),
    )
}

fn checked(repo: &Path, args: &[&str], call: Call<'_>) -> Result<String, ThreadGitError> {
    let output = git(repo, args, call)?;
    if !output.succeeded() {
        return Err(failed("prepare the selected commit", &output));
    }
    Ok(output.line())
}

fn split_patch(patch: &str) -> (String, Vec<String>) {
    let mut header = String::new();
    let mut hunks: Vec<String> = Vec::new();
    for line in patch.split_inclusive('\n') {
        if line.starts_with("@@ ") {
            hunks.push(String::new());
        }
        if let Some(hunk) = hunks.last_mut() {
            hunk.push_str(line);
        } else {
            header.push_str(line);
        }
    }
    (header, hunks)
}

fn read_patch(
    repo: &Path,
    head: &str,
    work: &str,
    path: &str,
    old: Option<&str>,
) -> Result<Patch, ThreadGitError> {
    let mut specs = vec![format!(":(literal){path}")];
    if let Some(old) = old {
        specs.push(format!(":(literal){old}"));
    }
    let mut args = vec![
        "diff",
        "--binary",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "--unified=3",
        "-M",
        head,
        work,
        "--",
    ];
    args.extend(specs.iter().map(String::as_str));
    let output = git(
        repo,
        &args,
        Call {
            max_output: MAX_PATCH + 1,
            ..Call::read()
        },
    )?;
    if output.truncated || output.stdout.len() > MAX_PATCH {
        return Ok(Patch::Large);
    }
    if !output.succeeded() {
        return Err(failed("read the selected commit", &output));
    }
    String::from_utf8(output.stdout)
        .map(Patch::Text)
        .map_err(|_| invalid("A changed path is not valid UTF-8. Commit it in a terminal."))
}

fn snapshot(cwd: &Path, touched: Option<&BTreeSet<String>>) -> Result<Snapshot, ThreadGitError> {
    let repo = require_repo(cwd)?;
    if let Some(why) = working_tree_commit_blocked(&repo)? {
        return Err(invalid(why));
    }
    let head = head_commit(&repo, Call::read())?.ok_or_else(|| {
        invalid("Make the repository's first commit in a terminal before selecting hunks.")
    })?;
    let branch = checked(&repo, &["symbolic-ref", "-q", "HEAD"], Call::read())?;
    let index_path = repo.join(checked(
        &repo,
        &["rev-parse", "--git-path", "index"],
        Call::read(),
    )?);
    let index_bytes = read_index(&index_path)?;
    let scratch = tempfile::tempdir().map_err(io_error)?;
    let index = scratch.path().join("original");
    fs::write(&index, &index_bytes).map_err(io_error)?;
    let index_tree = checked(&repo, &["write-tree"], Call::read().indexed(&index))?;
    let work = working_tree(&repo)?;
    let mut hash = Sha256::new();
    for part in [
        head.as_bytes(),
        branch.as_bytes(),
        index_bytes.as_slice(),
        work.as_bytes(),
    ] {
        hash.update(part.len().to_le_bytes());
        hash.update(part);
    }
    let revision = format!("{:x}", hash.finalize());
    let files = reviewed_files(&repo, &head, &work, touched)?;
    Ok(Snapshot {
        repo,
        head,
        branch,
        index_path,
        index_bytes,
        index_tree,
        work,
        revision,
        files,
    })
}

fn reviewed_files(
    repo: &Path,
    head: &str,
    work: &str,
    touched: Option<&BTreeSet<String>>,
) -> Result<Vec<ReviewedFile>, ThreadGitError> {
    let changes = changes_between(repo, head, work, &[])?;
    if changes.len() > 3_000 {
        return Err(invalid(
            "Too many changed files to review here. Commit in a terminal.",
        ));
    }
    let classified_files = changes.into_iter().map(classified).collect::<Vec<_>>();
    let patchable = classified_files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.omitted != Some(DiffOmitted::Sensitive))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut patches = (0..classified_files.len())
        .map(|_| None)
        .collect::<Vec<_>>();
    for batch in patchable.chunks(PATCH_WORKERS) {
        let results = std::thread::scope(|scope| {
            let workers = batch
                .iter()
                .map(|&index| {
                    let file = &classified_files[index];
                    scope.spawn(move || {
                        read_patch(repo, head, work, &file.path, file.old_path.as_deref())
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| {
                    worker
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect::<Vec<_>>()
        });
        for (&index, patch) in batch.iter().zip(results) {
            patches[index] = Some(patch?);
        }
    }
    let mut bytes = 0;
    let mut files = Vec::with_capacity(classified_files.len());
    for (file, patch) in classified_files.into_iter().zip(patches) {
        let mut blocked = (file.omitted == Some(DiffOmitted::Sensitive))
            .then(|| "Excluded path: review and commit it in a terminal.".to_owned());
        let patch = if blocked.is_none() {
            match patch {
                Some(Patch::Text(patch)) if bytes + patch.len() <= MAX_TOTAL => {
                    bytes += patch.len();
                    if redact_credentials(&patch) == patch {
                        patch
                    } else {
                        blocked = Some(
                            "This patch contains credential-shaped text. Review it in a terminal."
                                .into(),
                        );
                        String::new()
                    }
                }
                _ => {
                    blocked = Some(
                        "This patch is too large to review here. Commit it in a terminal.".into(),
                    );
                    String::new()
                }
            }
        } else {
            String::new()
        };
        let (header, mut hunks) = split_patch(&patch);
        // Partial deletion/rename/mode changes cannot be represented by simply
        // dropping text hunks; offer these as explicit whole-file selections.
        if file.status != DiffFileStatus::Modified || file.binary || header.contains(" mode ") {
            hunks.clear();
        }
        files.push(ReviewedFile {
            view: CommitReviewFile {
                attributed: touched.is_some_and(|paths| {
                    paths.contains(&file.path)
                        || file
                            .old_path
                            .as_ref()
                            .is_some_and(|old| paths.contains(old))
                }),
                path: file.path,
                status: file.status,
                hunks: hunks
                    .iter()
                    .enumerate()
                    .map(|(id, patch)| CommitReviewHunk {
                        id: u32::try_from(id).unwrap_or(u32::MAX),
                        patch: patch.clone(),
                    })
                    .collect(),
                blocked,
            },
            patch,
            header,
            hunks,
        });
    }
    Ok(files)
}

/// Reads all changes, including unattributed shell changes, without staging any.
///
/// # Errors
/// Refuses conflicted/unfinished Git operations and unbounded snapshots.
pub fn review(
    cwd: &Path,
    touched: Option<&BTreeSet<String>>,
) -> Result<CommitReview, ThreadGitError> {
    let snapshot = snapshot(cwd, touched)?;
    Ok(CommitReview {
        revision: snapshot.revision,
        files: snapshot.files.into_iter().map(|file| file.view).collect(),
    })
}

fn read_index(path: &Path) -> Result<Vec<u8>, ThreadGitError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.len() > MAX_INDEX {
        return Err(invalid("The Git index is not a regular bounded file."));
    }
    let mut bytes = Vec::new();
    file.take(MAX_INDEX + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_INDEX {
        return Err(invalid("The Git index grew beyond its size limit."));
    }
    Ok(bytes)
}

struct IndexLock {
    path: PathBuf,
    identity: fs::Metadata,
    // Once a branch update is attempted, retain recovery bytes on every exit.
    preserve: bool,
}
impl IndexLock {
    fn owns_path(&self) -> bool {
        fs::symlink_metadata(&self.path).is_ok_and(|current| {
            current.is_file()
                && current.dev() == self.identity.dev()
                && current.ino() == self.identity.ino()
        })
    }
    fn contains(&self, bytes: &[u8]) -> bool {
        self.owns_path()
            && read_index(&self.path).is_ok_and(|current| current == bytes)
            && self.owns_path()
    }
    fn recovery_message(&self, bytes: &[u8]) -> String {
        if self.contains(bytes) {
            format!(
                "The prepared index remains at {}. Inspect the branch and index in a terminal before another commit.",
                self.path.display()
            )
        } else {
            "The prepared index lock changed externally. Inspect the branch and index in a terminal before another commit.".into()
        }
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        // A hook can unlink the lock and put its own file in its place. Never
        // clean up a pathname unless it still names the file we created.
        if !self.preserve && self.owns_path() {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn apply_selection(
    snapshot: &Snapshot,
    choices: &[CommitChoice],
    index: &Path,
) -> Result<String, ThreadGitError> {
    if choices.is_empty() || choices.len() > snapshot.files.len() {
        return Err(invalid("Select at least one reviewed file or hunk."));
    }
    checked(
        &snapshot.repo,
        &["read-tree", &snapshot.head],
        Call::read().indexed(index),
    )?;
    let mut seen = BTreeSet::new();
    let mut patch = String::new();
    for choice in choices {
        if !seen.insert(&choice.path) {
            return Err(invalid("A file was selected more than once."));
        }
        let file = snapshot
            .files
            .iter()
            .find(|file| file.view.path == choice.path)
            .ok_or_else(|| invalid("A selected file is no longer in this review."))?;
        if let Some(why) = &file.view.blocked {
            return Err(invalid(why));
        }
        if let Some(ids) = &choice.hunks {
            if ids.is_empty() {
                continue;
            }
            let unique: BTreeSet<_> = ids.iter().copied().collect();
            if unique.len() != ids.len() || ids.iter().any(|id| *id as usize >= file.hunks.len()) {
                return Err(invalid("A selected hunk is no longer in this review."));
            }
            patch.push_str(&file.header);
            for id in unique {
                patch.push_str(&file.hunks[id as usize]);
            }
        } else {
            patch.push_str(&file.patch);
        }
    }
    if patch.is_empty() {
        return Err(invalid("Select at least one reviewed change."));
    }
    let output = git_with_input(
        &snapshot.repo,
        &["apply", "--cached", "--binary", "--whitespace=nowarn", "-"],
        Call::long().indexed(index),
        Some(patch.into_bytes()),
    )?;
    if !output.succeeded() {
        return Err(failed(
            "apply the selected hunks to a private index",
            &output,
        ));
    }
    checked(&snapshot.repo, &["write-tree"], Call::read().indexed(index))
}

fn unchanged(snapshot: &Snapshot) -> Result<bool, ThreadGitError> {
    Ok(
        head_commit(&snapshot.repo, Call::read())?.as_ref() == Some(&snapshot.head)
            && checked(
                &snapshot.repo,
                &["symbolic-ref", "-q", "HEAD"],
                Call::read(),
            )? == snapshot.branch
            && read_index(&snapshot.index_path)? == snapshot.index_bytes
            && working_tree(&snapshot.repo)? == snapshot.work,
    )
}

fn run_hook(
    snapshot: &Snapshot,
    index: &Path,
    name: &str,
    args: &[&str],
) -> Result<(), ThreadGitError> {
    let mut command = vec!["hook", "run", "--ignore-missing", name, "--"];
    command.extend_from_slice(args);
    let output = git(&snapshot.repo, &command, Call::checkout().indexed(index))?;
    if output.succeeded() {
        Ok(())
    } else {
        Err(failed("run the repository's commit hook", &output))
    }
}

fn reviewed_hook_message(
    snapshot: &Snapshot,
    selected_index: &Path,
    selected_tree: &str,
    message_file: &Path,
    message: &str,
) -> Result<Option<Vec<u8>>, ThreadGitError> {
    fs::write(message_file, message).map_err(io_error)?;
    let message_path = message_file
        .to_str()
        .ok_or_else(|| invalid("The commit message path is not UTF-8."))?;
    run_hook(snapshot, selected_index, "pre-commit", &[])?;
    run_hook(
        snapshot,
        selected_index,
        "prepare-commit-msg",
        &[message_path, "message"],
    )?;
    run_hook(snapshot, selected_index, "commit-msg", &[message_path])?;
    if checked(
        &snapshot.repo,
        &["write-tree"],
        Call::read().indexed(selected_index),
    )? != selected_tree
        || !unchanged(snapshot)?
    {
        return Ok(None);
    }
    if fs::metadata(message_file).map_err(io_error)?.len() > 64 * 1024 {
        return Err(invalid("The commit hook made the message too large."));
    }
    let message = fs::read(message_file).map_err(io_error)?;
    if message.iter().all(u8::is_ascii_whitespace) {
        return Err(invalid("The commit message is empty after running hooks."));
    }
    Ok(Some(message))
}

fn merge_staged_index(
    snapshot: &Snapshot,
    selected_tree: &str,
    merged: &Path,
) -> Result<bool, ThreadGitError> {
    let output = git(
        &snapshot.repo,
        &[
            "read-tree",
            "-i",
            "-m",
            &snapshot.head,
            selected_tree,
            &snapshot.index_tree,
        ],
        Call::long().indexed(merged),
    )?;
    if !output.succeeded()
        || !git(
            &snapshot.repo,
            &["write-tree"],
            Call::read().indexed(merged),
        )?
        .succeeded()
    {
        return Ok(false);
    }
    Ok(true)
}

fn commit_object(
    snapshot: &Snapshot,
    selected_index: &Path,
    selected_tree: &str,
    message: Vec<u8>,
) -> Result<String, ThreadGitError> {
    let signed = git(
        &snapshot.repo,
        &["config", "--bool", "--get", "commit.gpgsign"],
        Call::read(),
    )?;
    let mut args = vec!["commit-tree", selected_tree, "-p", &snapshot.head];
    if signed.line() == "true" {
        args.push("-S");
    }
    let object = git_with_input(
        &snapshot.repo,
        &args,
        Call::checkout().indexed(selected_index),
        Some(message),
    )?;
    if !object.succeeded() {
        return Err(failed("create the selected commit", &object));
    }
    Ok(object.line())
}

/// Commits exactly a reviewed selection once. Stale reviews, conflicting staged
/// work, and hooks that modify the selection are refused before moving HEAD.
///
/// # Errors
/// All returned errors precede a branch change. After that boundary uncertainty
/// is returned as `ShipEffect::Unknown`, never as retryable failure.
pub fn selected(
    cwd: &Path,
    revision: &str,
    message: &str,
    choices: &[CommitChoice],
) -> Result<ShipEffect<String>, ThreadGitError> {
    if message.trim().is_empty() || message.len() > 64 * 1024 {
        return Err(invalid("A commit needs a message (at most 64 KiB)."));
    }
    let snapshot = snapshot(cwd, None)?;
    if snapshot.revision != revision {
        return Ok(ShipEffect::Refused("The reviewed content, index or branch changed. Review the changes again before committing.".into()));
    }
    let scratch = tempfile::tempdir().map_err(io_error)?;
    let selected_index = scratch.path().join("selected");
    let selected_tree = apply_selection(&snapshot, choices, &selected_index)?;
    let merged = scratch.path().join("merged");
    if !merge_staged_index(&snapshot, &selected_tree, &merged)? {
        return Ok(ShipEffect::Refused("The selection overlaps differently staged work. Reconcile that staging in a terminal, then review again.".into()));
    }
    let lock_path = snapshot.index_path.with_file_name("index.lock");
    let mut lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| {
            invalid(
                "Another Git operation holds the index. Wait for it to finish and review again.",
            )
        })?;
    let mut lock = IndexLock {
        path: lock_path,
        identity: lock_file.metadata().map_err(io_error)?,
        preserve: false,
    };
    if !unchanged(&snapshot)? {
        return Ok(ShipEffect::Refused(
            "The checkout changed during review. Review again before committing.".into(),
        ));
    }
    let message_file = scratch.path().join("COMMIT_EDITMSG");
    let Some(message) = reviewed_hook_message(
        &snapshot,
        &selected_index,
        &selected_tree,
        &message_file,
        message,
    )?
    else {
        return Ok(ShipEffect::Refused(
            "A commit hook changed the reviewed content. Review those changes before committing."
                .into(),
        ));
    };
    // Prepare and sync the final index before the only branch-writing effect.
    // Hooks may write the same inode or replace the path. Only rewrite our own
    // file; truncate it so a hook's longer payload cannot survive at the end.
    if !lock.owns_path() {
        return Ok(ShipEffect::Refused(
            "A commit hook replaced the index lock. Inspect that lock before committing again."
                .into(),
        ));
    }
    let merged_bytes = read_index(&merged)?;
    lock_file.set_len(0).map_err(io_error)?;
    lock_file.write_all(&merged_bytes).map_err(io_error)?;
    lock_file.sync_all().map_err(io_error)?;
    let commit = commit_object(&snapshot, &selected_index, &selected_tree, message)?;
    if !unchanged(&snapshot)? {
        return Ok(ShipEffect::Refused(
            "The checkout changed while preparing the commit. Review again before committing."
                .into(),
        ));
    }
    if !lock.contains(&merged_bytes) {
        return Ok(ShipEffect::Refused(
            "The prepared index lock changed. Inspect it before committing again.".into(),
        ));
    }
    Ok(install_commit(
        &snapshot,
        &selected_index,
        &mut lock,
        &merged_bytes,
        commit,
    ))
}

// The return type deliberately has no retry-safe error variant after the CAS.
fn install_commit(
    snapshot: &Snapshot,
    selected_index: &Path,
    lock: &mut IndexLock,
    merged_bytes: &[u8],
    commit: String,
) -> ShipEffect<String> {
    // From this boundary onward even an unsuccessful command can be ambiguous:
    // a reference hook can run after the ref moved, fail, or change it again.
    // Preserve our prepared index and never return a retry-safe error/refusal.
    lock.preserve = true;
    let _update = git(
        &snapshot.repo,
        &[
            "update-ref",
            "-m",
            "commit: Interface selected changes",
            &snapshot.branch,
            &commit,
            &snapshot.head,
        ],
        Call::checkout(),
    );
    let observed = git(
        &snapshot.repo,
        &["rev-parse", "--verify", &snapshot.branch],
        Call::read(),
    );
    if !observed
        .as_ref()
        .is_ok_and(|output| output.succeeded() && output.line() == commit)
    {
        return ShipEffect::Unknown(format!(
            "The branch could not be confirmed after committing. {}",
            lock.recovery_message(merged_bytes)
        ));
    }
    // The index lock fences ordinary checkout, but a hook can bypass it with
    // symbolic-ref or direct writes. Do not install this branch's index after
    // HEAD switches, or overwrite index changes made outside the lock protocol.
    let branch = git(
        &snapshot.repo,
        &["symbolic-ref", "-q", "HEAD"],
        Call::read(),
    );
    let head = head_commit(&snapshot.repo, Call::read());
    if !branch
        .as_ref()
        .is_ok_and(|output| output.succeeded() && output.line() == snapshot.branch)
        || !head
            .as_ref()
            .is_ok_and(|head| head.as_ref() == Some(&commit))
        || !read_index(&snapshot.index_path).is_ok_and(|bytes| bytes == snapshot.index_bytes)
        || !lock.contains(merged_bytes)
    {
        return ShipEffect::Unknown(format!(
            "Commit {commit} was created, but the checkout or index lock changed before installation. {}",
            lock.recovery_message(merged_bytes)
        ));
    }
    // Check the owned inode immediately before rename; never install a lock
    // replaced by a hook or another Git process.
    if !lock.owns_path() || fs::rename(&lock.path, &snapshot.index_path).is_err() {
        return ShipEffect::Unknown(format!(
            "Commit {commit} was created, but the index could not be installed. {}",
            lock.recovery_message(merged_bytes)
        ));
    }
    if snapshot
        .index_path
        .parent()
        .and_then(|parent| fs::File::open(parent).ok())
        .is_none_or(|directory| directory.sync_all().is_err())
    {
        return ShipEffect::Unknown(format!(
            "Commit {commit} was created, but syncing the index could not be confirmed. Inspect Git status before another commit."
        ));
    }
    let _ = run_hook(snapshot, selected_index, "post-commit", &[]);
    ShipEffect::Done(commit)
}

#[cfg(test)]
#[path = "thread_commit_tests.rs"]
mod tests;
