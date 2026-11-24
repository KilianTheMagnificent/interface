use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use interface_contracts::ipc::{
    RepositoryReview, RepositoryReviewError, RepositoryReviewErrorCode, RepositoryReviewFile,
    RepositoryReviewFileStatus, RepositoryReviewOmission, RepositoryReviewState,
};
use interface_workspace::{is_likely_binary_path, is_search_excluded_path};

use crate::process::{cap_utf8_owned, run_hardened_git};
use crate::safety::{
    DirectorySnapshot, SafetyError, assert_directory, canonical_directory, read_verified_file,
};

pub const REVIEW_GIT_UNAVAILABLE: i32 = -1;
const REVIEW_PATH_CHANGED: i32 = -2;
/// Git started but did not finish within [`REVIEW_GIT_TIMEOUT`]. Review says
/// so, and names iCloud as the likely cause, instead of calling git missing.
pub const REVIEW_GIT_TIMED_OUT: i32 = -3;
/// How long one review git command may run.
pub const REVIEW_GIT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RepositoryReviewLimits {
    pub max_files: usize,
    pub max_status_bytes: usize,
    pub max_patch_files: usize,
    pub max_patch_bytes_per_file: usize,
    pub max_total_patch_bytes: usize,
    pub max_untracked_file_bytes: usize,
    pub max_metadata_bytes: usize,
}

pub const REPOSITORY_REVIEW_LIMITS: RepositoryReviewLimits = RepositoryReviewLimits {
    max_files: 200,
    max_status_bytes: 1024 * 1024,
    max_patch_files: 80,
    max_patch_bytes_per_file: 128 * 1024,
    max_total_patch_bytes: 1024 * 1024,
    max_untracked_file_bytes: 256 * 1024,
    max_metadata_bytes: 16 * 1024,
};

impl Default for RepositoryReviewLimits {
    fn default() -> Self {
        REPOSITORY_REVIEW_LIMITS
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReviewGitResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

pub trait ReviewGitRunner: Send + Sync {
    fn run(&self, args: &[String], cwd: &Path, max_output_bytes: usize) -> ReviewGitResult;

    /// Whether independent read-only commands may run at the same time. A
    /// runner that answers in call order (the scripted test fakes) keeps the
    /// default: one command at a time, in the order the review issues them.
    fn overlaps_reads(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemReviewGitRunner;

impl ReviewGitRunner for SystemReviewGitRunner {
    fn overlaps_reads(&self) -> bool {
        true
    }

    /// Runs nothing the repository's own git settings name (filter drivers
    /// above all: `status` and `diff` re-read files through them).
    fn run(&self, args: &[String], cwd: &Path, max_output_bytes: usize) -> ReviewGitResult {
        let output = run_hardened_git(args, cwd, REVIEW_GIT_TIMEOUT, max_output_bytes);
        let code = if output.unavailable {
            REVIEW_GIT_UNAVAILABLE
        } else if output.timed_out {
            REVIEW_GIT_TIMED_OUT
        } else if output.output_truncated {
            0
        } else {
            output.exit_code.unwrap_or(REVIEW_GIT_UNAVAILABLE)
        };
        ReviewGitResult {
            code,
            stdout: output.stdout,
            stderr: output.stderr,
            truncated: output.output_truncated,
        }
    }
}

pub trait RepositoryReviewHooks: Send + Sync {
    fn after_untracked_parent_validated(&self, _parent: &Path, _file: &Path) {}
    /// Runs on an untracked file once it is validated, right before the
    /// bounded open that reads it.
    fn before_untracked_open(&self, _file: &Path) {}
}

#[derive(Default)]
pub struct RepositoryReviewOptions<'a> {
    pub runner: Option<&'a dyn ReviewGitRunner>,
    pub limits: RepositoryReviewLimits,
    pub hooks: Option<&'a dyn RepositoryReviewHooks>,
    /// When true, skip git numstat/diff for tracked files. Untracked files still
    /// go through `read_untracked` so symlink/not-regular omissions stay intact.
    /// Default is false so `RepositoryReviewOptions::default()` keeps the Changes
    /// view's patch pipeline.
    pub skip_patches: bool,
}

const GIT_PREFIX: &[&str] = &[
    "--no-pager",
    "--no-optional-locks",
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.quotepath=false",
    "-c",
    "status.relativePaths=true",
    "-c",
    "status.renames=true",
    "-c",
    "diff.renames=true",
    "-c",
    "core.fsmonitor=false",
];

#[derive(Clone, Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
struct StatusEntry {
    path: String,
    previous_path: Option<String>,
    staged: bool,
    unstaged: bool,
    untracked: bool,
    conflict: bool,
    index_code: char,
    worktree_code: char,
    unsafe_path: bool,
}

#[derive(Clone, Debug, Default)]
struct ParsedStatus {
    files: Vec<StatusEntry>,
    file_limit_hit: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Numstat {
    additions: u64,
    deletions: u64,
    binary: bool,
}

enum LayerNumstat {
    Rows(HashMap<String, Numstat>),
    Fallback,
}

#[derive(Clone, Debug, Default)]
struct UntrackedRead {
    patch: String,
    additions: Option<u64>,
    binary: bool,
    truncated: bool,
    omitted: Option<RepositoryReviewOmission>,
    /// The read stopped at its deadline (the file is still downloading from
    /// iCloud); the review reads no more untracked files.
    downloading: bool,
}

fn resolved_display(path: &Path) -> String {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    resolved.to_string_lossy().into_owned()
}

fn review_error(code: RepositoryReviewErrorCode, message: &str) -> RepositoryReviewError {
    RepositoryReviewError {
        code,
        message: message.to_owned(),
    }
}

/// What Review says when git started but did not answer in time. Threads
/// says the same of its git calls: an iCloud Drive project whose files are
/// still downloading is the usual reason, not a missing git.
fn git_timed_out_message() -> String {
    format!(
        "Git did not finish reading this repository within {} seconds. If this project is in iCloud Drive, files Git needs may still be downloading; try again once they have.",
        REVIEW_GIT_TIMEOUT.as_secs()
    )
}

fn empty_review(root: String, code: RepositoryReviewErrorCode, message: &str) -> RepositoryReview {
    RepositoryReview {
        root,
        repository: false,
        branch: None,
        head: None,
        metadata_complete: None,
        clean: None,
        files: Vec::new(),
        truncated: code == RepositoryReviewErrorCode::OutputLimit,
        error: Some(review_error(code, message)),
    }
}

fn safe_relative(raw: &str) -> Option<String> {
    if raw.is_empty()
        || raw.contains('\0')
        || raw.contains(['\r', '\n'])
        || Path::new(raw).is_absolute()
    {
        return None;
    }
    if raw.contains('\\') {
        return None;
    }
    let normalized = raw.to_owned();
    if normalized
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return None;
    }
    Some(normalized)
}

fn parse_repository_status(stdout: &str, max_files: usize) -> ParsedStatus {
    let records = stdout.split('\0').collect::<Vec<_>>();
    let mut indices = HashMap::<String, usize>::new();
    let mut parsed = ParsedStatus::default();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        if record.is_empty() || record.starts_with("## ") {
            index += 1;
            continue;
        }
        let bytes = record.as_bytes();
        if bytes.len() < 4 || bytes[2] != b' ' {
            index += 1;
            continue;
        }
        let xy = &record[..2];
        let raw_path = &record[3..];
        let renamed = xy.contains('R') || xy.contains('C');
        let raw_previous = if renamed {
            index += 1;
            records.get(index).copied()
        } else {
            None
        };
        let relative = safe_relative(raw_path);
        let previous = raw_previous.and_then(safe_relative);
        let key = relative
            .clone()
            .unwrap_or_else(|| format!("\0unsafe-{index}"));

        let entry_index = if let Some(existing) = indices.get(&key).copied() {
            existing
        } else {
            if parsed.files.len() >= max_files {
                parsed.file_limit_hit = true;
                index += 1;
                continue;
            }
            let entry = StatusEntry {
                path: relative.clone().unwrap_or_else(|| raw_path.to_owned()),
                index_code: xy.chars().next().unwrap_or(' '),
                worktree_code: xy.chars().nth(1).unwrap_or(' '),
                unsafe_path: relative.is_none(),
                ..StatusEntry::default()
            };
            parsed.files.push(entry);
            let new_index = parsed.files.len() - 1;
            indices.insert(key, new_index);
            new_index
        };
        let entry = &mut parsed.files[entry_index];
        if raw_previous.is_some() {
            if previous.is_none() {
                entry.unsafe_path = true;
            } else {
                entry.previous_path = previous;
            }
        }
        if xy == "??" {
            entry.untracked = true;
        } else if matches!(xy, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU") || xy.contains('U') {
            entry.conflict = true;
        } else {
            let current_index = xy.chars().next().unwrap_or(' ');
            let current_worktree = xy.chars().nth(1).unwrap_or(' ');
            entry.staged = entry.staged || !matches!(current_index, ' ' | '!');
            entry.unstaged = entry.unstaged || !matches!(current_worktree, ' ' | '!');
        }
        index += 1;
    }
    parsed
}

fn states(file: &StatusEntry) -> Vec<RepositoryReviewState> {
    let mut result = Vec::with_capacity(2);
    if file.conflict {
        result.push(RepositoryReviewState::Conflict);
    }
    if file.staged {
        result.push(RepositoryReviewState::Staged);
    }
    if file.unstaged {
        result.push(RepositoryReviewState::Unstaged);
    }
    if file.untracked {
        result.push(RepositoryReviewState::Untracked);
    }
    result
}

fn file_status(file: &StatusEntry) -> RepositoryReviewFileStatus {
    if file.untracked {
        return RepositoryReviewFileStatus::Untracked;
    }
    if file.conflict {
        return RepositoryReviewFileStatus::Conflicted;
    }
    match if file.index_code == ' ' {
        file.worktree_code
    } else {
        file.index_code
    } {
        'A' => RepositoryReviewFileStatus::Added,
        'M' => RepositoryReviewFileStatus::Modified,
        'D' => RepositoryReviewFileStatus::Deleted,
        'R' => RepositoryReviewFileStatus::Renamed,
        'C' => RepositoryReviewFileStatus::Copied,
        'T' => RepositoryReviewFileStatus::TypeChanged,
        _ => RepositoryReviewFileStatus::Unknown,
    }
}

/// Walks `git diff --numstat -z` output: one `additions\tdeletions\tpath`
/// record per file, where a rename or copy has an empty path followed by its
/// two path records. Those two are always consumed here, so a path is never
/// read as a record. Short records and unparseable counts are skipped; `-`
/// counts yield a binary entry.
fn numstat_records(stdout: &str) -> impl Iterator<Item = (Numstat, &str)> {
    let mut records = stdout.split('\0');
    std::iter::from_fn(move || {
        loop {
            let mut fields = records.next()?.splitn(3, '\t');
            let (Some(additions), Some(deletions), Some(path)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if path.is_empty() {
                records.next();
                records.next();
            }
            if additions == "-" || deletions == "-" {
                return Some((
                    Numstat {
                        binary: true,
                        ..Numstat::default()
                    },
                    path,
                ));
            }
            if let (Ok(additions), Ok(deletions)) = (additions.parse(), deletions.parse()) {
                let counts = Numstat {
                    additions,
                    deletions,
                    binary: false,
                };
                return Some((counts, path));
            }
        }
    })
}

/// One file's total across its records (a rename's single record included);
/// binary as soon as any record is.
fn parse_numstat(stdout: &str) -> Option<Numstat> {
    let mut total: Option<Numstat> = None;
    for (stats, _) in numstat_records(stdout) {
        if stats.binary {
            return Some(stats);
        }
        let total = total.get_or_insert_with(Numstat::default);
        total.additions = total.additions.saturating_add(stats.additions);
        total.deletions = total.deletions.saturating_add(stats.deletions);
    }
    total
}

/// Per-path counts for a `--no-renames` batch. A rename record cannot appear
/// there; if git ever emits one anyway, it is dropped, leaving both entries
/// with no counts rather than crediting either with the pair's.
fn parse_numstat_rows(stdout: &str) -> HashMap<String, Numstat> {
    numstat_records(stdout)
        .filter(|(_, path)| !path.is_empty())
        .map(|(stats, path)| (path.to_owned(), stats))
        .collect()
}

fn load_layer_numstat(
    root: &DirectorySnapshot,
    runner: &dyn ReviewGitRunner,
    staged: bool,
    specs: &[String],
    limits: &RepositoryReviewLimits,
) -> std::result::Result<LayerNumstat, ()> {
    if specs.is_empty() {
        return Ok(LayerNumstat::Rows(HashMap::new()));
    }
    let batch_limit = limits
        .max_metadata_bytes
        .saturating_mul(limits.max_patch_files.max(1));
    let numstat = run_git(
        root,
        runner,
        numstat_args(staged, false, specs),
        batch_limit,
    );
    if numstat.code == REVIEW_PATH_CHANGED {
        return Err(());
    }
    if numstat.truncated || numstat.code != 0 {
        return Ok(LayerNumstat::Fallback);
    }
    Ok(LayerNumstat::Rows(parse_numstat_rows(&numstat.stdout)))
}

/// `renames` is off for a batch: several unrelated status entries share one
/// invocation, and rename detection across them would pair a delete with an
/// add and report one row for two files. A per-file call keeps detection on,
/// because a renamed entry passes both of its own paths and needs the pairing.
fn numstat_args(staged: bool, renames: bool, specs: &[String]) -> Vec<String> {
    let mut args = vec!["diff".to_owned()];
    if staged {
        args.push("--cached".to_owned());
    }
    args.extend(command(&[
        "--numstat",
        "-z",
        "--no-ext-diff",
        "--no-textconv",
    ]));
    if !renames {
        args.push("--no-renames".to_owned());
    }
    args.push("--".to_owned());
    args.extend(specs.iter().cloned());
    args
}

/// A path whose `diff --git a/<path> b/<path>` header git prints verbatim with
/// `core.quotepath=false`: no control byte, DEL, quote or backslash (which git
/// C-quotes), and no ` b/` that would make the header read ambiguously.
fn batchable_diff_path(path: &str) -> bool {
    !path.contains(" b/")
        && !path
            .bytes()
            .any(|byte| byte < 0x20 || matches!(byte, 0x7f | b'"' | b'\\'))
}

/// One layer's patches from a single `git diff` over every batchable file,
/// keyed by status index. A file is present only when its complete output
/// was received and can stand in for its own per-file call; every other file
/// (and every file when the batch fails, writes to stderr, or prints a
/// header it did not ask for) falls back to today's per-file diff.
///
/// # Errors
///
/// `Err` when the root changed around the call, exactly as `run_git` reports.
fn load_layer_diffs(
    root: &DirectorySnapshot,
    runner: &dyn ReviewGitRunner,
    staged: bool,
    files: &[StatusEntry],
    batch: &[usize],
    limits: &RepositoryReviewLimits,
) -> std::result::Result<HashMap<usize, String>, ()> {
    // One file gains nothing from a batch; it keeps the exact per-file call.
    if batch.len() < 2 {
        return Ok(HashMap::new());
    }
    let mut args = vec!["diff".to_owned()];
    if staged {
        args.push("--cached".to_owned());
    }
    // `--no-renames` keeps unrelated entries from pairing (a single-path call
    // cannot pair either). `--submodule=short` keeps a `diff.submodule=log`
    // config from printing header-less text that would land in a neighbour's
    // section; gitlink sections themselves are never taken from the batch.
    args.extend(command(&[
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--unified=3",
        "--no-renames",
        "--submodule=short",
        "--",
    ]));
    let mut headers = HashMap::with_capacity(batch.len());
    for &index in batch {
        let path = &files[index].path;
        args.push(format!(":(literal){path}"));
        headers.insert(format!("diff --git a/{path} b/{path}"), index);
    }
    let output = run_git(
        root,
        runner,
        args,
        limits.max_patch_bytes_per_file.saturating_mul(batch.len()),
    );
    if output.code == REVIEW_PATH_CHANGED {
        return Err(());
    }
    if output.code != 0 || !output.stderr.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(split_layer_diff(&output.stdout, output.truncated, &headers).unwrap_or_default())
}

/// Splits batched `git diff` output into each file's complete sections.
/// `None` means the output cannot be attributed and nothing may be used.
fn split_layer_diff(
    stdout: &str,
    truncated: bool,
    headers: &HashMap<String, usize>,
) -> Option<HashMap<usize, String>> {
    if !stdout.is_empty() && !stdout.starts_with("diff --git ") {
        return None;
    }
    // A section runs from its `diff --git` line to the next one. Patch body
    // lines always carry a ` `, `+`, `-`, `\` or `@@` prefix, so no content can
    // start a new section.
    let mut starts = vec![0];
    starts.extend(stdout.match_indices("\ndiff --git ").map(|(at, _)| at + 1));
    let mut sections = starts
        .iter()
        .zip(starts.iter().skip(1).chain(std::iter::once(&stdout.len())))
        .map(|(&start, &end)| &stdout[start..end])
        .filter(|section| !section.is_empty())
        .collect::<Vec<_>>();
    if truncated {
        // The last section was cut, and may not even hold its whole header.
        sections.pop();
    }
    // A typechange prints two consecutive sections for one path.
    let mut groups: Vec<(usize, String)> = Vec::new();
    let mut seen = HashSet::new();
    for section in sections {
        let header = section.split_once('\n').map_or(section, |(line, _)| line);
        let &index = headers.get(header)?;
        match groups.last_mut() {
            Some((last, output)) if *last == index => output.push_str(section),
            _ => {
                if !seen.insert(index) {
                    return None;
                }
                groups.push((index, section.to_owned()));
            }
        }
    }
    if truncated {
        // The file whose sections came last may continue past the cut.
        groups.pop();
    }
    Some(
        groups
            .into_iter()
            // A replacement character may stand for bytes the per-file capture
            // would cut differently; a gitlink may be printed in another format.
            .filter(|(_, output)| !output.contains('\u{FFFD}') && !is_gitlink_diff(output))
            .collect(),
    )
}

fn is_gitlink_diff(output: &str) -> bool {
    output.lines().any(|line| {
        line.starts_with("index ") && line.ends_with(" 160000") || line.ends_with(" mode 160000")
    })
}

/// What the per-file call would have returned for one fully received batch
/// output: the same capped byte prefix and truncation flag the bounded runner
/// produces, from bytes that are valid UTF-8 as received.
fn received_diff(output: &str, maximum: usize) -> ReviewGitResult {
    let maximum = maximum.max(1);
    if output.len() > maximum {
        ReviewGitResult {
            stdout: String::from_utf8_lossy(&output.as_bytes()[..maximum]).into_owned(),
            truncated: true,
            ..ReviewGitResult::default()
        }
    } else {
        ReviewGitResult {
            stdout: output.to_owned(),
            ..ReviewGitResult::default()
        }
    }
}

/// How one status entry is handled, decided once before any git call.
enum FileClass {
    Unsafe,
    Credential { binary: bool },
    Binary,
    Patchable { specs: Vec<String> },
}

fn classify_file(status: &StatusEntry) -> FileClass {
    if status.unsafe_path {
        return FileClass::Unsafe;
    }
    let binary = is_likely_binary_path(&status.path);
    if is_search_excluded_path(&status.path, false)
        || status
            .previous_path
            .as_deref()
            .is_some_and(|path| is_search_excluded_path(path, false))
    {
        return FileClass::Credential { binary };
    }
    if binary {
        return FileClass::Binary;
    }
    FileClass::Patchable {
        specs: literal_pathspecs(status),
    }
}

fn literal_pathspecs(file: &StatusEntry) -> Vec<String> {
    std::iter::once(&file.path)
        .chain(file.previous_path.iter())
        .map(|path| format!(":(literal){path}"))
        .collect()
}

fn clean_line(value: &str) -> Option<String> {
    let line = value.lines().next().unwrap_or_default().trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn display_patch_path(path: &str) -> String {
    if path.contains('\t') {
        serde_json::to_string(path).unwrap_or_else(|_| path.to_owned())
    } else {
        path.to_owned()
    }
}

fn untracked_patch(path: &str, text: &str, executable: bool) -> String {
    let shown = display_patch_path(path);
    let mut lines = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect::<Vec<_>>()
    };
    if text.ends_with('\n') {
        lines.pop();
    }
    let mode = if executable { "100755" } else { "100644" };
    let mut output = format!(
        "diff --git a/{shown} b/{shown}\nnew file mode {mode}\n--- /dev/null\n+++ b/{shown}\n"
    );
    if !lines.is_empty() {
        let _ = writeln!(output, "@@ -0,0 +1,{} @@", lines.len());
        for line in &lines {
            output.push('+');
            output.push_str(line);
            output.push('\n');
        }
        if !text.ends_with('\n') {
            output.push_str("\\ No newline at end of file\n");
        }
    }
    output
}

fn read_untracked(
    root: &DirectorySnapshot,
    file: &StatusEntry,
    patch_limit: usize,
    file_limit: usize,
    hooks: Option<&dyn RepositoryReviewHooks>,
) -> UntrackedRead {
    let callback = |parent: &Path, target: &Path| {
        if let Some(hooks) = hooks {
            hooks.after_untracked_parent_validated(parent, target);
        }
    };
    let before_open = |target: &Path| {
        if let Some(hooks) = hooks {
            hooks.before_untracked_open(target);
        }
    };
    let opened = match read_verified_file(
        root,
        Path::new(&file.path),
        file_limit,
        true,
        Some(&callback),
        Some(&before_open),
    ) {
        Ok(opened) => opened,
        Err(error) => {
            let (omitted, truncated) = match error {
                SafetyError::Symlink => (RepositoryReviewOmission::Symlink, false),
                // A read past its deadline is unreadable for now, like any
                // other; omitted paths are never named to the agent.
                SafetyError::NotRegular | SafetyError::Unreadable | SafetyError::Downloading => {
                    (RepositoryReviewOmission::NotRegular, false)
                }
                SafetyError::OutputLimit => (RepositoryReviewOmission::OutputLimit, true),
                SafetyError::HardLink | SafetyError::PathChanged => {
                    (RepositoryReviewOmission::UnsafePath, false)
                }
            };
            return UntrackedRead {
                omitted: Some(omitted),
                truncated,
                downloading: error == SafetyError::Downloading,
                ..UntrackedRead::default()
            };
        }
    };
    if opened.content.contains(&0) {
        return UntrackedRead {
            binary: true,
            omitted: Some(RepositoryReviewOmission::Binary),
            ..UntrackedRead::default()
        };
    }
    let Ok(text) = std::str::from_utf8(&opened.content) else {
        return UntrackedRead {
            binary: true,
            omitted: Some(RepositoryReviewOmission::Binary),
            ..UntrackedRead::default()
        };
    };
    let full = untracked_patch(&file.path, text, opened.executable);
    let (patch, truncated) = cap_utf8_owned(full, patch_limit);
    let additions = if text.is_empty() {
        0
    } else {
        u64::try_from(text.split('\n').count() - usize::from(text.ends_with('\n')))
            .unwrap_or(u64::MAX)
    };
    UntrackedRead {
        patch,
        additions: Some(additions),
        binary: false,
        truncated,
        omitted: truncated.then_some(RepositoryReviewOmission::OutputLimit),
        downloading: false,
    }
}

fn append_patch(mut current: String, next: &str, limit: usize) -> (String, bool) {
    if next.is_empty() {
        return cap_utf8_owned(current, limit);
    }
    if !current.is_empty() {
        current.push('\n');
    }
    current.push_str(next);
    cap_utf8_owned(current, limit)
}

fn run_git(
    root: &DirectorySnapshot,
    runner: &dyn ReviewGitRunner,
    args: Vec<String>,
    maximum: usize,
) -> ReviewGitResult {
    let prefixed = GIT_PREFIX
        .iter()
        .map(ToString::to_string)
        .chain(args)
        .collect::<Vec<_>>();
    if assert_directory(root).is_err() {
        return ReviewGitResult {
            code: REVIEW_PATH_CHANGED,
            ..ReviewGitResult::default()
        };
    }
    let result = runner.run(&prefixed, &root.path, maximum);
    if assert_directory(root).is_err() {
        return ReviewGitResult {
            code: REVIEW_PATH_CHANGED,
            ..ReviewGitResult::default()
        };
    }
    result
}

fn command(parts: &[&str]) -> Vec<String> {
    parts.iter().map(ToString::to_string).collect()
}

#[must_use]
pub fn read_repository_review(root: &Path) -> RepositoryReview {
    read_repository_review_with(root, &RepositoryReviewOptions::default())
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn read_repository_review_with(
    root_path: &Path,
    options: &RepositoryReviewOptions<'_>,
) -> RepositoryReview {
    let Ok(root) = canonical_directory(root_path) else {
        return empty_review(
            resolved_display(root_path),
            RepositoryReviewErrorCode::GitError,
            "Interface could not safely inspect that open folder.",
        );
    };
    let canonical = root.path.to_string_lossy().into_owned();
    let limits = options.limits;
    let system_runner = SystemReviewGitRunner;
    let runner = options.runner.unwrap_or(&system_runner);
    let unsafe_root = || {
        empty_review(
            canonical.clone(),
            RepositoryReviewErrorCode::GitError,
            "The open folder changed while Interface was reviewing it. No review data was returned.",
        )
    };

    let inside_tree_command = || {
        run_git(
            &root,
            runner,
            command(&["rev-parse", "--is-inside-work-tree"]),
            limits.max_metadata_bytes,
        )
    };
    let branch_command = || {
        run_git(
            &root,
            runner,
            command(&["symbolic-ref", "--quiet", "--short", "HEAD"]),
            limits.max_metadata_bytes,
        )
    };
    let head_command = || {
        run_git(
            &root,
            runner,
            command(&["rev-parse", "--verify", "HEAD"]),
            limits.max_metadata_bytes,
        )
    };
    let status_command = || {
        run_git(
            &root,
            runner,
            command(&[
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
                "--",
                ".",
            ]),
            limits.max_status_bytes,
        )
    };
    // The four reads are independent: a runner that can overlap them pays
    // for the slowest (status), not their sum. Their answers are still judged
    // in the order below, so a folder outside a repository reports exactly
    // as before; it only cost three extra overlapped reads.
    let (inside_tree, prefetched) = if runner.overlaps_reads() {
        std::thread::scope(|scope| {
            let branch = scope.spawn(branch_command);
            let head = scope.spawn(head_command);
            let status = scope.spawn(status_command);
            let inside_tree = inside_tree_command();
            let joined = |handle: std::thread::ScopedJoinHandle<'_, ReviewGitResult>| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            };
            (
                inside_tree,
                Some((joined(branch), joined(head), joined(status))),
            )
        })
    } else {
        (inside_tree_command(), None)
    };
    if inside_tree.code == REVIEW_PATH_CHANGED {
        return unsafe_root();
    }
    if inside_tree.code == REVIEW_GIT_UNAVAILABLE {
        return empty_review(
            canonical,
            RepositoryReviewErrorCode::GitUnavailable,
            "Git is not available on this machine.",
        );
    }
    if inside_tree.code == REVIEW_GIT_TIMED_OUT {
        return empty_review(
            canonical,
            RepositoryReviewErrorCode::GitError,
            &git_timed_out_message(),
        );
    }
    if inside_tree.truncated {
        return empty_review(
            canonical,
            RepositoryReviewErrorCode::OutputLimit,
            "Git repository metadata exceeded the review limit.",
        );
    }
    if inside_tree.code != 0 || clean_line(&inside_tree.stdout).as_deref() != Some("true") {
        return empty_review(
            canonical,
            RepositoryReviewErrorCode::NotRepository,
            "This folder is not inside a Git repository.",
        );
    }

    let (branch_result, head_result, status_result) =
        prefetched.unwrap_or_else(|| (branch_command(), head_command(), status_command()));
    if [branch_result.code, head_result.code, status_result.code].contains(&REVIEW_PATH_CHANGED) {
        return unsafe_root();
    }
    let branch_legitimate = matches!(branch_result.code, 0 | 1);
    let head_legitimate =
        head_result.code == 0 || head_result.code == 128 && branch_result.code == 0;
    let metadata_incomplete =
        !branch_legitimate || !head_legitimate || branch_result.truncated || head_result.truncated;
    let branch = (branch_result.code == 0 && !branch_result.truncated)
        .then(|| clean_line(&branch_result.stdout))
        .flatten();
    let head = (head_result.code == 0 && !head_result.truncated)
        .then(|| clean_line(&head_result.stdout))
        .flatten();
    let status_failure = if status_result.code == REVIEW_GIT_UNAVAILABLE {
        Some((
            RepositoryReviewErrorCode::GitUnavailable,
            "Git stopped before status could be read.".to_owned(),
        ))
    } else if status_result.code == REVIEW_GIT_TIMED_OUT {
        Some((RepositoryReviewErrorCode::GitError, git_timed_out_message()))
    } else if status_result.code != 0 {
        Some((
            RepositoryReviewErrorCode::GitError,
            "Git could not read this repository's status.".to_owned(),
        ))
    } else {
        None
    };
    if let Some((code, message)) = status_failure {
        return RepositoryReview {
            root: canonical,
            repository: true,
            branch,
            head,
            metadata_complete: Some(!metadata_incomplete),
            clean: None,
            files: Vec::new(),
            truncated: metadata_incomplete,
            error: Some(review_error(code, &message)),
        };
    }

    let complete_status = if status_result.truncated && !status_result.stdout.ends_with('\0') {
        status_result
            .stdout
            .rfind('\0')
            .map_or("", |position| &status_result.stdout[..=position])
    } else {
        &status_result.stdout
    };
    let parsed = parse_repository_status(complete_status, limits.max_files);
    let mut truncated = status_result.truncated || parsed.file_limit_hit || metadata_incomplete;
    let mut total_patch_bytes = 0_usize;
    let mut patch_files = 0_usize;
    let mut untracked_downloading = false;
    let mut git_read_error = false;
    let mut files = Vec::with_capacity(parsed.files.len());

    let classes = parsed.files.iter().map(classify_file).collect::<Vec<_>>();

    let (staged_numstat, unstaged_numstat, staged_diffs, unstaged_diffs) = if options.skip_patches {
        (
            LayerNumstat::Rows(HashMap::new()),
            LayerNumstat::Rows(HashMap::new()),
            HashMap::new(),
            HashMap::new(),
        )
    } else {
        let mut staged_specs = Vec::new();
        let mut unstaged_specs = Vec::new();
        let mut staged_batch = Vec::new();
        let mut unstaged_batch = Vec::new();
        let mut budget = limits.max_patch_files;
        for (index, (status, class)) in parsed.files.iter().zip(&classes).enumerate() {
            let FileClass::Patchable { specs } = class else {
                continue;
            };
            if budget == 0 {
                break;
            }
            budget -= 1;
            // Untracked files are read from disk, and a rename's two pathspecs would let git pair
            // it with an unrelated path in the batch: both keep their own numstat call.
            if status.untracked || status.previous_path.is_some() {
                continue;
            }
            if status.staged {
                staged_specs.extend(specs.iter().cloned());
            }
            if status.unstaged || status.conflict {
                unstaged_specs.extend(specs.iter().cloned());
            }
            // Conflicts print combined diffs, and paths git would C-quote need a
            // header this code does not rebuild: both keep their per-file diff.
            if status.conflict || !batchable_diff_path(&status.path) {
                continue;
            }
            if status.staged {
                staged_batch.push(index);
            }
            if status.unstaged {
                unstaged_batch.push(index);
            }
        }
        let Ok(staged) = load_layer_numstat(&root, runner, true, &staged_specs, &limits) else {
            return unsafe_root();
        };
        let Ok(unstaged) = load_layer_numstat(&root, runner, false, &unstaged_specs, &limits)
        else {
            return unsafe_root();
        };
        // numstat already proved some files binary; their patch is never read.
        let without_binary = |mut batch: Vec<usize>, numstat: &LayerNumstat| {
            if let LayerNumstat::Rows(rows) = numstat {
                batch.retain(|&index| {
                    !rows
                        .get(&parsed.files[index].path)
                        .is_some_and(|stats| stats.binary)
                });
            }
            batch
        };
        let staged_batch = without_binary(staged_batch, &staged);
        let Ok(staged_diffs) =
            load_layer_diffs(&root, runner, true, &parsed.files, &staged_batch, &limits)
        else {
            return unsafe_root();
        };
        let unstaged_batch = without_binary(unstaged_batch, &unstaged);
        let Ok(unstaged_diffs) = load_layer_diffs(
            &root,
            runner,
            false,
            &parsed.files,
            &unstaged_batch,
            &limits,
        ) else {
            return unsafe_root();
        };
        (staged, unstaged, staged_diffs, unstaged_diffs)
    };

    for (index, (status, class)) in parsed.files.iter().zip(classes).enumerate() {
        let (omitted, known_binary) = match &class {
            FileClass::Unsafe => (Some(RepositoryReviewOmission::UnsafePath), false),
            FileClass::Credential { binary } => {
                (Some(RepositoryReviewOmission::Credential), *binary)
            }
            FileClass::Binary => (Some(RepositoryReviewOmission::Binary), true),
            FileClass::Patchable { .. } => (None, false),
        };
        let mut item = RepositoryReviewFile {
            path: status.path.clone(),
            previous_path: status.previous_path.clone(),
            states: states(status),
            status: file_status(status),
            additions: None,
            deletions: None,
            binary: known_binary,
            patch: String::new(),
            patch_truncated: false,
            omitted,
        };
        let FileClass::Patchable { specs } = class else {
            files.push(item);
            continue;
        };
        if patch_files >= limits.max_patch_files
            || total_patch_bytes >= limits.max_total_patch_bytes
        {
            item.omitted = Some(RepositoryReviewOmission::OutputLimit);
            item.patch_truncated = true;
            truncated = true;
            files.push(item);
            continue;
        }
        patch_files += 1;
        let available = limits
            .max_patch_bytes_per_file
            .min(limits.max_total_patch_bytes - total_patch_bytes);

        if status.untracked && untracked_downloading {
            // One untracked file was still downloading from iCloud; the rest
            // of an evicted project would each wait out the same deadline.
            item.omitted = Some(RepositoryReviewOmission::NotRegular);
        } else if status.untracked {
            let result = read_untracked(
                &root,
                status,
                available,
                limits.max_untracked_file_bytes,
                options.hooks,
            );
            untracked_downloading |= result.downloading;
            if assert_directory(&root).is_err() {
                return unsafe_root();
            }
            item.patch = result.patch;
            item.patch_truncated = result.truncated;
            item.additions = result.additions;
            item.deletions = result.additions.map(|_| 0);
            item.binary = result.binary;
            item.omitted = result.omitted;
        } else if !options.skip_patches {
            let mut counts_valid = !status.conflict;
            let mut additions = 0_u64;
            let mut deletions = 0_u64;
            for staged in [true, false] {
                let active = if staged {
                    status.staged
                } else {
                    status.unstaged || status.conflict
                };
                if !active {
                    continue;
                }
                let layer = if staged {
                    &staged_numstat
                } else {
                    &unstaged_numstat
                };
                let stats = match layer {
                    LayerNumstat::Rows(rows) if status.previous_path.is_none() => {
                        rows.get(&status.path).copied()
                    }
                    _ => {
                        let numstat = run_git(
                            &root,
                            runner,
                            numstat_args(staged, true, &specs),
                            limits.max_metadata_bytes,
                        );
                        if numstat.code == REVIEW_PATH_CHANGED {
                            return unsafe_root();
                        }
                        if numstat.code != 0 {
                            git_read_error = true;
                        }
                        if numstat.truncated {
                            truncated = true;
                        }
                        if numstat.code == 0 && !numstat.truncated {
                            parse_numstat(&numstat.stdout)
                        } else {
                            None
                        }
                    }
                };
                match stats {
                    None => counts_valid = false,
                    Some(stats) if stats.binary => {
                        item.binary = true;
                        counts_valid = false;
                    }
                    Some(stats) => {
                        additions = additions.saturating_add(stats.additions);
                        deletions = deletions.saturating_add(stats.deletions);
                    }
                }
                if item.binary {
                    continue;
                }

                let remaining = available.saturating_sub(item.patch.len());
                if remaining == 0 {
                    item.patch_truncated = true;
                    item.omitted = Some(RepositoryReviewOmission::OutputLimit);
                    truncated = true;
                    continue;
                }
                let batched = if staged {
                    &staged_diffs
                } else {
                    &unstaged_diffs
                };
                let diff = if let Some(output) = batched.get(&index) {
                    received_diff(output, remaining)
                } else {
                    let mut diff_args = vec!["diff".to_owned()];
                    if staged {
                        diff_args.push("--cached".to_owned());
                    }
                    diff_args.extend(command(&[
                        "--no-ext-diff",
                        "--no-textconv",
                        "--no-color",
                        "--unified=3",
                        "--",
                    ]));
                    diff_args.extend(specs.iter().cloned());
                    run_git(&root, runner, diff_args, remaining)
                };
                if diff.code == REVIEW_PATH_CHANGED {
                    return unsafe_root();
                }
                if diff.code != 0 {
                    item.omitted = Some(RepositoryReviewOmission::GitError);
                    counts_valid = false;
                    git_read_error = true;
                    continue;
                }
                let layer_name = if staged { "staged" } else { "unstaged" };
                let chunk = if diff.stdout.is_empty() {
                    String::new()
                } else {
                    format!("Interface layer: {layer_name}\n{}", diff.stdout)
                };
                let (combined, append_truncated) =
                    append_patch(std::mem::take(&mut item.patch), &chunk, available);
                item.patch = combined;
                item.patch_truncated = item.patch_truncated || diff.truncated || append_truncated;
                if item.patch_truncated {
                    item.omitted = Some(RepositoryReviewOmission::OutputLimit);
                    truncated = true;
                }
            }
            if item.binary {
                item.omitted = Some(RepositoryReviewOmission::Binary);
            }
            if counts_valid && !item.binary {
                item.additions = Some(additions);
                item.deletions = Some(deletions);
            }
        }
        total_patch_bytes = total_patch_bytes.saturating_add(item.patch.len());
        if item.patch_truncated {
            truncated = true;
        }
        files.push(item);
    }

    if assert_directory(&root).is_err() {
        return unsafe_root();
    }
    let error = if metadata_incomplete {
        Some(review_error(
            RepositoryReviewErrorCode::GitError,
            "Git could not confirm branch or commit metadata; the review snapshot is partial.",
        ))
    } else if truncated {
        Some(review_error(
            RepositoryReviewErrorCode::OutputLimit,
            "Some changed paths or patch data exceeded the bounded Review limits.",
        ))
    } else if git_read_error {
        Some(review_error(
            RepositoryReviewErrorCode::GitError,
            "Git could not return complete review data for every changed path.",
        ))
    } else {
        None
    };
    RepositoryReview {
        root: canonical,
        repository: true,
        branch,
        head,
        metadata_complete: Some(!metadata_incomplete),
        clean: if status_result.truncated {
            None
        } else {
            Some(files.is_empty())
        },
        files,
        truncated,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RepositoryReviewFileStatus, RepositoryReviewState, parse_numstat, parse_repository_status,
    };

    #[test]
    fn parses_porcelain_rename_and_state_order() {
        let parsed = parse_repository_status("RM new name.txt\0old name.txt\0?? new.txt\0", 10);
        assert_eq!(parsed.files.len(), 2);
        assert_eq!(parsed.files[0].path, "new name.txt");
        assert_eq!(
            parsed.files[0].previous_path.as_deref(),
            Some("old name.txt")
        );
        assert_eq!(
            super::states(&parsed.files[0]),
            vec![
                RepositoryReviewState::Staged,
                RepositoryReviewState::Unstaged
            ]
        );
        assert_eq!(
            super::file_status(&parsed.files[0]),
            RepositoryReviewFileStatus::Renamed
        );
    }

    #[test]
    fn parses_binary_and_rename_numstat() {
        assert!(parse_numstat("-\t-\timage\0").expect("stat").binary);
        let stat = parse_numstat("2\t1\t\0old\0new\0").expect("rename stat");
        assert_eq!(stat.additions, 2);
        assert_eq!(stat.deletions, 1);
        let rows = super::parse_numstat_rows(concat!("-\t-\timage\0", "2\t1\tfile.txt\0"));
        assert!(rows["image"].binary);
        assert_eq!(rows["file.txt"].additions, 2);
        assert_eq!(rows["file.txt"].deletions, 1);
        let renamed = super::parse_numstat_rows(concat!("2\t1\t\0old\0new\0", "3\t0\tafter.txt\0"));
        assert!(!renamed.contains_key("new"));
        assert!(!renamed.contains_key("old"));
        assert_eq!(renamed["after.txt"].additions, 3);
    }

    #[test]
    fn both_numstat_parsers_walk_records_the_same_way() {
        let total = |stdout| {
            parse_numstat(stdout).map(|stat| (stat.additions, stat.deletions, stat.binary))
        };
        let rows = |stdout| {
            let mut rows = super::parse_numstat_rows(stdout)
                .into_iter()
                .map(|(path, stat)| (path, stat.additions, stat.deletions, stat.binary))
                .collect::<Vec<_>>();
            rows.sort();
            rows
        };
        assert_eq!(total(""), None);
        assert!(rows("").is_empty());
        // A rename consumes its two path records even when one looks like a row.
        let renamed = concat!("2\t1\t\0", "9\t9\told\0", "new\0", "3\t0\tafter.txt\0");
        assert_eq!(total(renamed), Some((5, 1, false)));
        assert_eq!(rows(renamed), [("after.txt".to_owned(), 3, 0, false)]);
        // A binary rename is binary for the total and no row for the batch.
        let binary_rename = concat!("-\t-\t\0", "old\0", "new\0", "1\t1\tf\0");
        assert_eq!(total(binary_rename), Some((0, 0, true)));
        assert_eq!(rows(binary_rename), [("f".to_owned(), 1, 1, false)]);
        // Short and unparseable records are skipped one at a time.
        let malformed = concat!("garbage\0", "x\t1\tg\0", "3\t4\tf\0", "\0");
        assert_eq!(total(malformed), Some((3, 4, false)));
        assert_eq!(rows(malformed), [("f".to_owned(), 3, 4, false)]);
        assert_eq!(total("x\t1\tg\0"), None);
        // Even an unparseable rename record consumes its paths.
        let bad_rename = concat!("x\t1\t\0", "5\t5\told\0", "new\0");
        assert_eq!(total(bad_rename), None);
        assert!(rows(bad_rename).is_empty());
        // A later row for the same path replaces the earlier one.
        assert_eq!(
            rows(concat!("1\t1\tf\0", "-\t-\tf\0")),
            [("f".to_owned(), 0, 0, true)]
        );
    }

    #[test]
    fn creates_the_exact_untracked_patch_shape() {
        assert_eq!(
            super::untracked_patch("new file.txt", "one\nlast", false),
            "diff --git a/new file.txt b/new file.txt\n\
             new file mode 100644\n\
             --- /dev/null\n\
             +++ b/new file.txt\n\
             @@ -0,0 +1,2 @@\n\
             +one\n\
             +last\n\
             \\ No newline at end of file\n"
        );
    }
}
