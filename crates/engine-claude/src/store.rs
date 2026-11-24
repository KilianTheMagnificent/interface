use std::{
    collections::{HashMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use interface_common::time::{IsoFraction, unix_seconds_iso};
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, EngineId, SessionSummary},
    ipc::{HistoryPage, unavailable_history},
    prompt_images::{inline_prompt_image_url, replayed_turn_images},
};
use interface_engine::{ForkBoundary, adapter::FORK_BOUNDARY_NOT_FOUND};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::mapper::ClaudeEventMapper;

mod cache_file;
mod history_cache;
mod listing_cache;
use listing_cache::CachedSummary;

pub const INTERFACE_SESSION_TAG: &str = "interface";
const MAX_TITLE_CHARS: usize = 80;
// This probe never reads transcript bytes, but a fantastical sparse or corrupt
// inode is not trustworthy evidence that a resumable provider session exists.
// One TiB is deliberately far above a plausible interactive transcript while
// still giving the metadata-only existence check a finite safety boundary.
const MAX_SESSION_PROBE_BYTES: u64 = 1 << 40;
const COMPACTION_PREAMBLE: &str = "This session is being continued from a previous conversation";
const ENGINE_BLOCKS: [(&str, &str); 7] = [
    ("<command-name>", "</command-name>"),
    ("<command-message>", "</command-message>"),
    ("<command-args>", "</command-args>"),
    ("<local-command-stdout>", "</local-command-stdout>"),
    ("<local-command-stderr>", "</local-command-stderr>"),
    ("<task-notification>", "</task-notification>"),
    ("<system-reminder>", "</system-reminder>"),
];
/// What Claude Code writes as a user message when the owner stops a turn.
const INTERRUPT_MARKERS: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];
/// The echo of an interactive `!` bash command, written as a user message.
const BASH_MODE_BLOCKS: [(&str, &str); 3] = [
    ("<bash-input>", "</bash-input>"),
    ("<bash-stdout>", "</bash-stdout>"),
    ("<bash-stderr>", "</bash-stderr>"),
];

#[derive(Debug)]
pub struct StoreListing {
    pub sessions: Vec<SessionSummary>,
    pub complete: bool,
}

/// Per-transcript display summaries. Unchanged files reuse their summary;
/// appends verify the old prefix and parse only new records. Missing paths are evicted.
#[derive(Debug, Default)]
pub struct ListingCache {
    entries: HashMap<PathBuf, CachedSummary>,
}

#[must_use]
pub fn default_projects_root() -> PathBuf {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".claude"))
        })
        .unwrap_or_else(|| PathBuf::from(".claude"));
    config.join("projects")
}

#[must_use]
pub fn project_dir_name(cwd: &str) -> String {
    cwd.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Check whether Claude's store contains one exact provider session without
/// reading or parsing any transcript content.
///
/// A clean absence is distinct from an unsafe or unreadable store. Callers can
/// use `false` to offer Claude's own recovery picker, while every I/O or path
/// safety failure remains an error instead of being guessed away.
///
/// # Errors
///
/// Returns an error for an invalid provider UUID, an unsafe store shape, or a
/// metadata/read-directory failure that prevents proving absence.
pub fn session_exists(root: &Path, session_id: &str) -> std::io::Result<bool> {
    if !provider_session_id(session_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Claude session id is invalid",
        ));
    }
    let Some(root_metadata) = session_probe_directory(root, true)? else {
        return Ok(false);
    };

    let file = format!("{session_id}.jsonl");
    for project in fs::read_dir(root)? {
        let project = project?;
        if project.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let project_path = project.path();
        let project_metadata = fs::symlink_metadata(&project_path)?;
        if project_metadata.file_type().is_symlink() {
            if sibling_project_alias(root, &project_path).is_some() {
                continue;
            }
            return Err(unsafe_session_probe("Claude project path was a symlink"));
        }
        if !project_metadata.is_dir() {
            continue;
        }
        validate_session_probe_directory(&project_metadata)?;
        if session_probe_file(&project_path.join(&file))? {
            assert_session_probe_directory(&project_path, &project_metadata)?;
            assert_session_probe_directory(root, &root_metadata)?;
            return Ok(true);
        }
        assert_session_probe_directory(&project_path, &project_metadata)?;
    }
    assert_session_probe_directory(root, &root_metadata)?;
    Ok(false)
}

/// A link from one project name to another project directory directly inside
/// the same store: what a project folder that moved leaves behind so Claude
/// finds its old transcripts under the new path. Nothing is read through it;
/// the real directory is scanned under its own name, so skipping the alias
/// adds no transcript. A link leading anywhere else stays an unsafe store.
fn sibling_project_alias(root: &Path, link: &Path) -> Option<PathBuf> {
    let root = fs::canonicalize(root).ok()?;
    let target = fs::canonicalize(link).ok()?;
    (target.parent() == Some(root.as_path())
        && fs::symlink_metadata(&target)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink()))
    .then_some(target)
}

/// Recover the durable Claude session id encoded by one exact transcript path.
///
/// The claimed transcript must be a direct child of one direct project
/// directory below `projects_root`, with the exact shape
/// `<projects>/<project>/<canonical-v4>.jsonl`. The transcript is inspected as
/// metadata only; its bytes are never read. Directory and file identity are
/// rechecked around the descriptor open so a path replacement cannot be used
/// to bless an unrelated session. A moved project's alias may resolve to a
/// real sibling directory in this same store; the alias identity and target
/// are rechecked too. Store-root and transcript-file links remain refused.
///
/// # Errors
///
/// Returns `InvalidInput` when the claimed path is outside the configured
/// projects root, has the wrong nesting, or does not carry a canonical UUID-v4
/// filename. A missing transcript returns `NotFound`; unsafe or unstable store
/// metadata returns an error.
pub fn session_id_from_transcript_path(
    projects_root: &Path,
    transcript_path: &Path,
) -> std::io::Result<String> {
    let root_metadata = session_probe_directory(projects_root, false)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Claude projects root did not exist",
        )
    })?;
    let relative = transcript_path.strip_prefix(projects_root).map_err(|_| {
        invalid_transcript_path("Claude transcript path was outside the projects root")
    })?;
    let mut components = relative.components();
    let (Some(std::path::Component::Normal(project)), Some(std::path::Component::Normal(file))) =
        (components.next(), components.next())
    else {
        return Err(invalid_transcript_path(
            "Claude transcript path had invalid project nesting",
        ));
    };
    if components.next().is_some() || project.to_string_lossy().starts_with('.') {
        return Err(invalid_transcript_path(
            "Claude transcript path had invalid project nesting",
        ));
    }
    let Some(file) = file.to_str() else {
        return Err(invalid_transcript_path(
            "Claude transcript filename was not valid UTF-8",
        ));
    };
    let Some(session_id) = file.strip_suffix(".jsonl") else {
        return Err(invalid_transcript_path(
            "Claude transcript filename was invalid",
        ));
    };
    let parsed = Uuid::parse_str(session_id)
        .map_err(|_| invalid_transcript_path("Claude transcript session id was invalid"))?;
    let canonical = parsed.hyphenated().to_string();
    if parsed.get_version_num() != 4
        || parsed.get_variant() != uuid::Variant::RFC4122
        || canonical != session_id
    {
        return Err(invalid_transcript_path(
            "Claude transcript session id was not a canonical UUID-v4",
        ));
    }

    let named_project = projects_root.join(project);
    let named_metadata = fs::symlink_metadata(&named_project)?;
    let alias = named_metadata.file_type().is_symlink();
    let project_path = if alias {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if named_metadata.uid() != session_probe_effective_uid()? {
                return Err(unsafe_session_probe(
                    "Claude project alias had an unsafe owner",
                ));
            }
        }
        sibling_project_alias(projects_root, &named_project).ok_or_else(|| {
            unsafe_session_probe("Claude project alias did not name a sibling project")
        })?
    } else {
        named_project.clone()
    };
    let project_metadata = session_probe_directory(&project_path, false)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Claude project directory did not exist",
        )
    })?;
    if !session_probe_file(&project_path.join(file))? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Claude transcript did not exist",
        ));
    }
    assert_session_probe_directory(&project_path, &project_metadata)?;
    if alias {
        assert_session_probe_alias(
            projects_root,
            &named_project,
            &named_metadata,
            &project_path,
        )?;
    }
    assert_session_probe_directory(projects_root, &root_metadata)?;
    Ok(canonical)
}

fn assert_session_probe_alias(
    root: &Path,
    alias: &Path,
    expected: &fs::Metadata,
    target: &Path,
) -> std::io::Result<()> {
    let observed = fs::symlink_metadata(alias)?;
    if !observed.file_type().is_symlink()
        || !same_session_probe_file(expected, &observed)
        || sibling_project_alias(root, alias).as_deref() != Some(target)
    {
        return Err(unsafe_session_probe(
            "Claude project alias changed during the session probe",
        ));
    }
    Ok(())
}

fn invalid_transcript_path(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}

fn unsafe_session_probe(message: &'static str) -> std::io::Error {
    std::io::Error::other(message)
}

fn session_probe_directory(
    path: &Path,
    missing_is_absence: bool,
) -> std::io::Result<Option<fs::Metadata>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if missing_is_absence && error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    validate_session_probe_directory(&metadata)?;
    Ok(Some(metadata))
}

fn assert_session_probe_directory(path: &Path, expected: &fs::Metadata) -> std::io::Result<()> {
    let Some(observed) = session_probe_directory(path, false)? else {
        unreachable!("a required session probe directory cannot be absent")
    };
    if !same_session_probe_directory(expected, &observed) {
        return Err(unsafe_session_probe(
            "Claude project directory changed during the session probe",
        ));
    }
    Ok(())
}

fn validate_session_probe_directory(metadata: &fs::Metadata) -> std::io::Result<()> {
    if !metadata.file_type().is_dir() {
        return Err(unsafe_session_probe(
            "Claude project path was not a safe directory",
        ));
    }
    #[cfg(unix)]
    validate_session_probe_unix_metadata(metadata, false, session_probe_effective_uid()?)?;
    Ok(())
}

fn session_probe_file(path: &Path) -> std::io::Result<bool> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    validate_session_probe_file_metadata(&before)?;

    // The descriptor is opened only to prove that the checked name resolves to
    // the same safe inode. No transcript bytes are ever read. On Unix,
    // O_NOFOLLOW closes the final-component symlink race and O_NONBLOCK keeps a
    // replace-with-FIFO/device race from hanging the host.
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let opened_file = options.open(path)?;
    let opened = opened_file.metadata()?;
    validate_session_probe_file_metadata(&opened)?;
    if !same_session_probe_file(&before, &opened) {
        return Err(unsafe_session_probe(
            "Claude transcript changed while its session was probed",
        ));
    }

    let named = fs::symlink_metadata(path)?;
    validate_session_probe_file_metadata(&named)?;
    if !same_session_probe_file(&opened, &named) {
        return Err(unsafe_session_probe(
            "Claude transcript path changed while its session was probed",
        ));
    }
    Ok(true)
}

fn validate_session_probe_file_metadata(metadata: &fs::Metadata) -> std::io::Result<()> {
    if !metadata.file_type().is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_SESSION_PROBE_BYTES
    {
        return Err(unsafe_session_probe(
            "Claude transcript was not a safe regular file",
        ));
    }
    #[cfg(unix)]
    validate_session_probe_unix_metadata(metadata, true, session_probe_effective_uid()?)?;
    Ok(())
}

#[cfg(unix)]
fn validate_session_probe_unix_metadata(
    metadata: &fs::Metadata,
    require_single_link: bool,
    effective_uid: u32,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if metadata.uid() != effective_uid
        || metadata.mode() & 0o022 != 0
        || (require_single_link && metadata.nlink() != 1)
    {
        return Err(unsafe_session_probe(
            "Claude session metadata had unsafe Unix ownership or permissions",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn session_probe_effective_uid() -> std::io::Result<u32> {
    use interface_common::unix::{EffectiveUidError, effective_uid};

    effective_uid().map_err(|error| match error {
        EffectiveUidError::Spawn(error) => error,
        EffectiveUidError::Failed => {
            unsafe_session_probe("Interface could not determine its effective Unix user")
        }
        EffectiveUidError::NotText(_) | EffectiveUidError::NotUid(_) => {
            unsafe_session_probe("Interface received an invalid effective Unix user")
        }
    })
}

#[cfg(unix)]
fn same_session_probe_directory(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_session_probe_directory(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.modified().ok() == right.modified().ok()
}

#[cfg(unix)]
fn same_session_probe_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    interface_common::unix::same_stable_metadata(left, right) && left.gid() == right.gid()
}

#[cfg(not(unix))]
fn same_session_probe_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

pub fn list_sessions(
    root: &Path,
    engine: &EngineId,
    cache: &mut ListingCache,
) -> std::io::Result<StoreListing> {
    let readers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(LISTING_READERS);
    list_sessions_with(root, engine, cache, readers)
}

/// The most threads a listing reads transcripts on. A cold listing reads every
/// transcript (gigabytes on a long-used machine) to find each title.
const LISTING_READERS: usize = 4;

fn list_sessions_with(
    root: &Path,
    engine: &EngineId,
    cache: &mut ListingCache,
    readers: usize,
) -> std::io::Result<StoreListing> {
    let projects = match fs::read_dir(root) {
        Ok(projects) => projects,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cache.entries.clear();
            return Ok(StoreListing {
                sessions: Vec::new(),
                complete: true,
            });
        }
        Err(error) => return Err(error),
    };
    let mut previous = std::mem::take(&mut cache.entries);
    let mut complete = true;
    // Every transcript in walk order, with the entry the last listing kept.
    let mut transcripts = Vec::<Transcript>::new();
    for project in projects {
        let Ok(project) = project else {
            complete = false;
            continue;
        };
        let Ok(kind) = project.file_type() else {
            complete = false;
            continue;
        };
        if !kind.is_dir() || project.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(entries) = fs::read_dir(project.path()) else {
            complete = false;
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|value| value.to_str()) else {
                complete = false;
                continue;
            };
            if !provider_session_id(session_id) {
                continue;
            }
            let session_id = session_id.to_owned();
            let kept = previous.remove(&path);
            transcripts.push(Transcript {
                path,
                session_id,
                kept,
            });
        }
    }
    let summaries = summarize_all(transcripts, engine, readers);
    let mut sessions = HashMap::<String, SessionSummary>::new();
    let mut fresh = HashMap::<PathBuf, CachedSummary>::new();
    // Merged in walk order, so a session id in two projects resolves exactly
    // as a one-by-one listing would.
    for (path, session_id, summary, keep) in summaries {
        if let Some(keep) = keep {
            fresh.insert(path, keep);
        }
        match summary {
            Ok(summary) => {
                let replace = sessions
                    .get(&session_id)
                    .is_none_or(|current| summary.last_activity > current.last_activity);
                if replace {
                    sessions.insert(session_id, summary);
                }
            }
            Err(_) => complete = false,
        }
    }
    cache.entries = fresh;
    let mut sessions = sessions.into_values().collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        right
            .last_activity
            .cmp(&left.last_activity)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(StoreListing { sessions, complete })
}

/// One transcript a listing summarizes, with the cache entry the last
/// listing kept for it.
struct Transcript {
    path: PathBuf,
    session_id: String,
    kept: Option<CachedSummary>,
}

type Summarized = (
    PathBuf,
    String,
    std::io::Result<SessionSummary>,
    Option<CachedSummary>,
);

/// Summarizes every transcript, in input order. Up to `readers` threads take
/// transcripts as they free up, so one huge file does not hold the rest.
fn summarize_all(
    transcripts: Vec<Transcript>,
    engine: &EngineId,
    readers: usize,
) -> Vec<Summarized> {
    let mut results = Vec::with_capacity(transcripts.len());
    let mut pending = Vec::new();
    for (index, transcript) in transcripts.into_iter().enumerate() {
        match safe_file_metadata(&transcript.path) {
            Ok(metadata)
                if transcript
                    .kept
                    .as_ref()
                    .is_some_and(|old| old.is_current(&metadata)) =>
            {
                let (summary, keep) = summarize_cached_with_metadata(
                    &transcript.path,
                    engine,
                    &transcript.session_id,
                    transcript.kept,
                    Some(metadata),
                );
                results.push(Some((
                    transcript.path,
                    transcript.session_id,
                    summary,
                    keep,
                )));
            }
            Ok(metadata) => {
                results.push(None);
                pending.push((index, transcript, metadata));
            }
            Err(error) => {
                results.push(Some((
                    transcript.path,
                    transcript.session_id,
                    Err(error),
                    None,
                )));
            }
        }
    }
    let summarize = |(index, transcript, metadata): (usize, Transcript, fs::Metadata)| {
        let (summary, keep) = summarize_cached_with_metadata(
            &transcript.path,
            engine,
            &transcript.session_id,
            transcript.kept,
            Some(metadata),
        );
        (
            index,
            (transcript.path, transcript.session_id, summary, keep),
        )
    };
    let readers = readers.min(pending.len());
    if readers < 2 {
        for (index, summarized) in pending.into_iter().map(summarize) {
            results[index] = Some(summarized);
        }
        return results
            .into_iter()
            .map(|entry| entry.expect("every transcript slot is populated"))
            .collect();
    }
    let queue = pending
        .into_iter()
        .map(|item| std::sync::Mutex::new(Some(item)))
        .collect::<Vec<_>>();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::thread::scope(|scope| {
        let workers = (0..readers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(slot) = queue.get(index) else {
                            break done;
                        };
                        if let Some(item) = interface_common::sync::lock(slot).take() {
                            done.push(summarize(item));
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect::<Vec<_>>()
    });
    for (index, summarized) in done {
        results[index] = Some(summarized);
    }
    results
        .into_iter()
        .map(|entry| entry.expect("every transcript slot is populated"))
        .collect()
}

/// Summarizes one transcript from the entry the last listing kept, returning
/// the entry to keep now. The cache is presentation-only; it grants no
/// provider or filesystem authority.
fn summarize_cached_with_metadata(
    path: &Path,
    engine: &EngineId,
    session_id: &str,
    mut kept: Option<CachedSummary>,
    mut initial_metadata: Option<fs::Metadata>,
) -> (std::io::Result<SessionSummary>, Option<CachedSummary>) {
    let mut attempt = 1;
    loop {
        let metadata = match initial_metadata.take() {
            Some(metadata) => metadata,
            None => match safe_file_metadata(path) {
                Ok(metadata) => metadata,
                Err(error) => return (Err(error), None),
            },
        };
        // An unchanged transcript carries its entry forward without a copy. A
        // changed one is read from a copy, so a read that loses a race with
        // Claude appending keeps the entry the next attempt resumes from.
        let old = if kept.as_ref().is_some_and(|old| old.is_current(&metadata)) {
            kept.take()
        } else {
            kept.clone()
        };
        match listing_cache::read(path, &metadata, engine, session_id, old) {
            Ok(cached) => {
                let summary = cached.summary.clone();
                return (Ok(summary), Some(cached));
            }
            Err(error) if cache_file::changed_during_read(&error) => {
                if attempt < cache_file::READ_ATTEMPTS {
                    attempt += 1;
                    continue;
                }
                // Still moving: the transcript demonstrably exists, so the row
                // the last listing kept stands in for it (seconds stale at
                // most) and the next listing resumes from that entry instead
                // of reparsing from byte 0. Only a transcript never yet read
                // leaves the listing incomplete.
                return match kept {
                    Some(kept) => (Ok(kept.summary.clone()), Some(kept)),
                    None => (Err(error), None),
                };
            }
            Err(error) => return (Err(error), None),
        }
    }
}

fn listing_line_may_matter(line: &str, cwd_missing: bool, first_prompt_missing: bool) -> bool {
    line.contains("\"summary\"")
        || line.contains("custom-title")
        || line.contains("\"tag\"")
        || (cwd_missing && line.contains("\"cwd\""))
        || (first_prompt_missing && line.contains("\"user\""))
}

pub fn history(root: &Path, session_id: &str, cwd: &str, limit: usize) -> HistoryPage {
    match read_history_page(root, session_id, cwd, limit) {
        Ok(Some((records, older_messages))) => HistoryPage {
            availability: None,
            reason: None,
            events: history_events(records.iter()),
            messages: Some(records.len() as u64),
            older_messages: older_messages as u64,
        },
        Ok(None) => unavailable_history("Claude's local store does not contain this transcript."),
        Err(_) => unavailable_history("Interface could not safely read this Claude transcript."),
    }
}

fn history_events<'a>(records: impl IntoIterator<Item = &'a Value>) -> Vec<EngineEvent> {
    let mut mapper = ClaudeEventMapper::default();
    let mut events = Vec::new();
    for record in records.into_iter().filter_map(Value::as_object) {
        let record_type = nonempty(record.get("type"));
        let agent_key = nonempty(record.get("parent_tool_use_id")).map(ToOwned::to_owned);
        if record_type == Some("assistant") {
            for text in text_blocks(record.get("message")) {
                events.push(event(EngineEventKind::MessageDelta {
                    text,
                    agent_key: agent_key.clone(),
                }));
            }
        } else if record_type == Some("user") {
            let text = user_turn_text(record.get("message"));
            let images = user_turn_images(record.get("message"));
            if text.is_some() || !images.is_empty() {
                events.push(event(EngineEventKind::UserMessage {
                    text: text.unwrap_or_default(),
                    agent_key: agent_key.clone(),
                    images: (!images.is_empty()).then_some(images),
                    image_references: None,
                }));
            }
        }
        events.extend(mapper.map_object(record));
        if record_type == Some("assistant") {
            events.push(event(EngineEventKind::ThinkingSignal {
                active: false,
                agent_key,
            }));
        }
    }
    events
}

fn event(event: EngineEventKind) -> EngineEvent {
    EngineEvent { at: None, event }
}

fn user_turn_text(message: Option<&Value>) -> Option<String> {
    if has_tool_result(message) {
        return None;
    }
    let text = text_blocks(message).join("\n\n");
    let trimmed = text.trim();
    if trimmed.starts_with(COMPACTION_PREAMBLE) {
        return None;
    }
    let mut cleaned = trimmed.to_owned();
    for (open, close) in ENGINE_BLOCKS {
        if cleaned.contains(open) {
            cleaned = strip_balanced_blocks(&cleaned, open, close);
        }
    }
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

fn strip_balanced_blocks(input: &str, open: &str, close: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find(open) {
        output.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(close) else {
            output.push_str(&rest[start..]);
            return output;
        };
        rest = &after_open[end + close.len()..];
    }
    output.push_str(rest);
    output
}

fn user_turn_images(
    message: Option<&Value>,
) -> Vec<interface_contracts::prompt_images::PromptTurnImage> {
    if has_tool_result(message) {
        return Vec::new();
    }
    let mut urls = Vec::new();
    for block in message
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
    {
        if nonempty(block.get("type")) != Some("image") {
            continue;
        }
        let Some(source) = block.get("source").and_then(Value::as_object) else {
            continue;
        };
        if nonempty(source.get("type")) != Some("base64") {
            continue;
        }
        if let (Some(media_type), Some(data)) = (
            nonempty(source.get("media_type")),
            nonempty(source.get("data")),
        ) {
            if let Some(url) = inline_prompt_image_url(media_type, data) {
                urls.push(url);
            }
        }
    }
    replayed_turn_images(&urls)
}

fn has_tool_result(message: Option<&Value>) -> bool {
    message
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks
                .iter()
                .filter_map(Value::as_object)
                .any(|block| nonempty(block.get("type")) == Some("tool_result"))
        })
}

fn text_blocks(message: Option<&Value>) -> Vec<String> {
    let Some(message) = message else {
        return Vec::new();
    };
    if let Some(text) = message.as_str().filter(|text| !text.is_empty()) {
        return vec![text.to_owned()];
    }
    let Some(message) = message.as_object() else {
        return Vec::new();
    };
    if let Some(text) = nonempty(message.get("content")) {
        return vec![text.to_owned()];
    }
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|block| nonempty(block.get("type")) == Some("text"))
        .filter_map(|block| nonempty(block.get("text")).map(ToOwned::to_owned))
        .collect()
}

pub fn rename(root: &Path, session_id: &str, title: &str, cwd: &str) -> std::io::Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Claude session title must not be empty",
        ));
    }
    let record = json!({"type":"custom-title", "customTitle":title, "sessionId":session_id});
    in_project(root, session_id, cwd, |path| {
        append_provider_metadata(path, &record)
    })
}

pub fn tag(root: &Path, session_id: &str, adopt: bool, cwd: &str) -> std::io::Result<()> {
    let record = json!({
        "type":"tag",
        // The Agent SDK writes an empty string when tagSession(..., null)
        // clears a tag. Keep that provider-owned wire shape byte-for-byte.
        "tag": if adopt { INTERFACE_SESSION_TAG } else { "" },
        "sessionId":session_id
    });
    in_project(root, session_id, cwd, |path| {
        append_provider_metadata(path, &record)
    })
}

fn append_provider_metadata(path: &Path, value: &Value) -> std::io::Result<()> {
    let before = safe_file_metadata(path)?;
    if before.len() == 0 {
        return Err(session_not_found());
    }
    let mut encoded = Vec::new();
    serde_json::to_writer(&mut encoded, value)?;
    encoded.push(b'\n');

    // This is the one write shape the Agent SDK itself uses: O_APPEND against
    // the provider's live transcript. Replacing the file transactionally would
    // be more dangerous here because Claude can still have the inode open and
    // append a new turn while the best-effort launch tag is being written.
    let mut file = OpenOptions::new().append(true).open(path)?;
    let opened = file.metadata()?;
    if !same_file_snapshot(&before, &opened) {
        return Err(std::io::Error::other(
            "Claude transcript changed while session metadata was prepared",
        ));
    }
    file.write_all(&encoded)?;
    file.sync_data()
}

pub fn delete(root: &Path, session_id: &str, cwd: &str) -> std::io::Result<()> {
    in_project(root, session_id, cwd, delete_transaction)
}

pub fn fork(root: &Path, session_id: &str, cwd: &str) -> std::io::Result<String> {
    in_project(root, session_id, cwd, |source| {
        let records = read_jsonl(source)?;
        fork_transaction(source, session_id, &records, records.len())
    })
}

/// Forks keeping the transcript through the last answer to the owner prompt
/// `boundary` names: the copy ends on the last assistant message before the
/// next prompt, the entry Claude's own `--resume-session-at` would cut at.
/// Prompts are the ones [`history`] shows, less the user messages Claude Code
/// writes itself (see [`is_written_by_claude`]), walked from the newest so
/// prompts the thread host never recorded cannot shift the cut. `Ok(Err(_))`
/// explains why no such cut exists; nothing was written then.
///
/// # Errors
///
/// Fails when the transcript cannot be read or the copy cannot be written.
pub fn fork_at(
    root: &Path,
    session_id: &str,
    cwd: &str,
    boundary: &ForkBoundary,
) -> std::io::Result<Result<String, String>> {
    in_project(root, session_id, cwd, |source| {
        let records = read_jsonl(source)?;
        match fork_cut(&records, boundary) {
            Ok(cut) => fork_transaction(source, session_id, &records, cut + 1).map(Ok),
            Err(reason) => Ok(Err(reason)),
        }
    })
}

/// The index of the record a fork through `boundary`'s prompt ends on.
fn fork_cut(records: &[Value], boundary: &ForkBoundary) -> Result<usize, String> {
    let prompts = records
        .iter()
        .enumerate()
        .filter(|(_, record)| is_history_message(record) && is_history_prompt(record))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let found =
        boundary.find_newest_first(prompts.iter().enumerate().rev().map(|(position, &index)| {
            let text = user_turn_text(records[index].get("message")).unwrap_or_default();
            (position, text)
        }));
    let Some(position) = found else {
        return Err(FORK_BOUNDARY_NOT_FOUND.into());
    };
    let next = prompts.get(position + 1).copied().unwrap_or(records.len());
    (prompts[position] + 1..next)
        .rev()
        .find(|&index| {
            let record = &records[index];
            is_history_message(record)
                && nonempty(record.get("type")) == Some("assistant")
                && nonempty(record.get("parent_tool_use_id")).is_none()
        })
        .ok_or_else(|| "That turn has no answer yet, so there is nothing to fork after.".into())
}

/// An owner prompt: a main-thread user message the history page shows (text
/// or an image left once tool results and engine blocks are removed) that
/// Claude Code did not write itself.
fn is_history_prompt(record: &Value) -> bool {
    let Some(record) = record.as_object() else {
        return false;
    };
    if nonempty(record.get("type")) != Some("user")
        || nonempty(record.get("parent_tool_use_id")).is_some()
    {
        return false;
    }
    let message = record.get("message");
    let images = !user_turn_images(message).is_empty();
    match user_turn_text(message) {
        Some(text) => images || !is_written_by_claude(&text),
        None => images,
    }
}

/// A user message's text that Claude Code wrote, not the owner: the marker a
/// stop leaves (`[Request interrupted by user]`, and its `for tool use`
/// form) and the echo of an interactive `!` bash command. The history page
/// still shows them; a fork never counts them as prompts.
fn is_written_by_claude(text: &str) -> bool {
    if INTERRUPT_MARKERS.contains(&text) {
        return true;
    }
    let mut rest = text.to_owned();
    let mut echoed = false;
    for (open, close) in BASH_MODE_BLOCKS {
        if rest.contains(open) {
            rest = strip_balanced_blocks(&rest, open, close);
            echoed = true;
        }
    }
    echoed && rest.trim().is_empty()
}

/// Writes a fork of the first `kept` records (metadata comes from them all).
fn fork_transaction(
    source: &Path,
    session_id: &str,
    records: &[Value],
    kept: usize,
) -> std::io::Result<String> {
    let forked_id = Uuid::new_v4().to_string();
    let output = fork_records_through(records, kept, session_id, &forked_id)?;
    let destination = source.with_file_name(format!("{forked_id}.jsonl"));
    let mut encoded = Vec::new();
    for value in output {
        serde_json::to_writer(&mut encoded, &value)?;
        encoded.push(b'\n');
    }
    let parent = parent_directory(source)?;
    let (temporary, mut writer) = private_temporary_file(parent, "fork")?;
    if let Err(error) = writer.write_all(&encoded).and_then(|()| writer.sync_all()) {
        drop(writer);
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    drop(writer);
    if let Err(error) = fs::hard_link(&temporary, &destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = sync_directory(parent) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let _ = fs::remove_file(&temporary);
    Ok(forked_id)
}

#[cfg(test)]
fn fork_records(
    records: &[Value],
    session_id: &str,
    forked_id: &str,
) -> std::io::Result<Vec<Value>> {
    fork_records_through(records, records.len(), session_id, forked_id)
}

/// The fork of the transcript entries among the first `kept` records; title,
/// content replacements and relocation come from every record.
fn fork_records_through(
    records: &[Value],
    kept: usize,
    session_id: &str,
    forked_id: &str,
) -> std::io::Result<Vec<Value>> {
    let transcript = records[..kept.min(records.len())]
        .iter()
        .filter_map(Value::as_object)
        .filter(|record| {
            record.get("isSidechain").and_then(Value::as_bool) != Some(true)
                && matches!(
                    nonempty(record.get("type")),
                    Some("user" | "assistant" | "attachment" | "system" | "progress")
                )
                && nonempty(record.get("uuid")).is_some()
        })
        .collect::<Vec<_>>();
    let mut uuid_map = HashMap::new();
    let mut by_uuid = HashMap::new();
    for record in &transcript {
        let uuid = nonempty(record.get("uuid")).expect("filtered transcript uuid");
        uuid_map.insert(uuid.to_owned(), Uuid::new_v4().to_string());
        by_uuid.insert(uuid.to_owned(), *record);
    }
    let visible = transcript
        .into_iter()
        .filter(|record| nonempty(record.get("type")) != Some("progress"))
        .collect::<Vec<_>>();
    if visible.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Claude session has no messages to fork",
        ));
    }

    let timestamp = system_time_iso(SystemTime::now());
    let mut output = Vec::new();
    if records
        .iter()
        .filter_map(Value::as_object)
        .any(|record| nonempty(record.get("type")) == Some("history-suppression"))
    {
        output.push(json!({
            "type":"history-suppression", "sessionId":forked_id,
            "cause":"fork_inherit", "ts":timestamp
        }));
    }
    for (index, record) in visible.iter().enumerate() {
        let old_uuid = nonempty(record.get("uuid")).expect("filtered visible uuid");
        let mut forked = (*record).clone();
        forked.insert("uuid".into(), Value::String(uuid_map[old_uuid].clone()));
        forked.insert(
            "parentUuid".into(),
            remap_fork_parent(record, &by_uuid, &uuid_map),
        );
        match record.get("logicalParentUuid") {
            Some(Value::String(uuid)) => {
                let replacement = uuid_map
                    .get(uuid)
                    .cloned()
                    .map_or(Value::Null, Value::String);
                forked.insert("logicalParentUuid".into(), replacement);
            }
            Some(Value::Null) => {
                forked.insert("logicalParentUuid".into(), Value::Null);
            }
            _ => {
                forked.remove("logicalParentUuid");
            }
        }
        forked.insert("sessionId".into(), Value::String(forked_id.to_owned()));
        if index + 1 == visible.len() {
            forked.insert("timestamp".into(), Value::String(timestamp.clone()));
        }
        if nonempty(record.get("type")) == Some("system")
            && nonempty(record.get("subtype")) == Some("model_refusal_fallback")
        {
            forked.insert("neutralizedByFork".into(), Value::Bool(true));
        }
        forked.insert("isSidechain".into(), Value::Bool(false));
        for field in [
            "teamName",
            "agentName",
            "sessionKind",
            "slug",
            "sourceToolAssistantUUID",
        ] {
            forked.remove(field);
        }
        forked.insert(
            "forkedFrom".into(),
            json!({"sessionId":session_id,"messageUuid":old_uuid}),
        );
        output.push(Value::Object(forked));
    }

    append_fork_metadata(records, session_id, forked_id, &timestamp, &mut output);
    let title = format!("{} (fork)", fork_source_title(records));
    output.push(json!({
        "type":"custom-title", "customTitle":title, "sessionId":forked_id,
        "uuid":Uuid::new_v4().to_string(), "timestamp":timestamp
    }));
    Ok(output)
}

fn append_fork_metadata(
    records: &[Value],
    session_id: &str,
    forked_id: &str,
    timestamp: &str,
    output: &mut Vec<Value>,
) {
    let replacements = records
        .iter()
        .filter_map(Value::as_object)
        .filter(|record| {
            nonempty(record.get("type")) == Some("content-replacement")
                && nonempty(record.get("sessionId")) == Some(session_id)
        })
        .filter_map(|record| record.get("replacements").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if !replacements.is_empty() {
        output.push(json!({
            "type":"content-replacement", "sessionId":forked_id,
            "replacements":replacements, "uuid":Uuid::new_v4().to_string(),
            "timestamp":timestamp
        }));
    }
    if let Some(relocated_cwd) = records
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|record| {
            (nonempty(record.get("type")) == Some("relocated"))
                .then(|| nonempty(record.get("relocatedCwd")))
                .flatten()
        })
        .next_back()
    {
        output.push(json!({
            "type":"relocated", "sessionId":forked_id, "relocatedCwd":relocated_cwd
        }));
    }
}

fn remap_fork_parent(
    record: &Map<String, Value>,
    by_uuid: &HashMap<String, &Map<String, Value>>,
    uuid_map: &HashMap<String, String>,
) -> Value {
    let mut parent = nonempty(record.get("parentUuid"));
    // An acyclic chain visits each record at most once, so this bound only
    // stops a parentUuid cycle (which a hand-edited transcript can hold).
    for _ in 0..by_uuid.len() {
        let Some(parent_uuid) = parent else {
            break;
        };
        let Some(parent_record) = by_uuid.get(parent_uuid) else {
            break;
        };
        if nonempty(parent_record.get("type")) != Some("progress") {
            return uuid_map
                .get(parent_uuid)
                .cloned()
                .map_or(Value::Null, Value::String);
        }
        parent = nonempty(parent_record.get("parentUuid"));
    }
    Value::Null
}

fn fork_source_title(records: &[Value]) -> String {
    let mut custom_title = None;
    let mut ai_title = None;
    let mut first_prompt = None;
    for record in records.iter().filter_map(Value::as_object) {
        match nonempty(record.get("type")) {
            Some("custom-title") => {
                custom_title = nonempty(record.get("customTitle")).map(ToOwned::to_owned);
            }
            Some("user") if first_prompt.is_none() => {
                first_prompt = user_turn_text(record.get("message"));
            }
            _ => {}
        }
        if let Some(title) = nonempty(record.get("aiTitle")) {
            ai_title = Some(title.to_owned());
        }
    }
    custom_title
        .or(ai_title)
        .or(first_prompt)
        .unwrap_or_else(|| "Forked session".into())
}

fn create_private_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn delete_transaction(path: &Path) -> std::io::Result<()> {
    let before = safe_file_metadata(path)?;
    if before.len() == 0 {
        return Err(session_not_found());
    }
    let parent = parent_directory(path)?;
    let subagents = path.with_extension("");
    let subagent_metadata = match fs::symlink_metadata(&subagents) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(std::io::Error::other(
                    "Claude subagent path was not a safe directory",
                ));
            }
            Some(metadata)
        }
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    let current = safe_file_metadata(path)?;
    if !same_file_snapshot(&before, &current) {
        return Err(std::io::Error::other(
            "Claude transcript changed while deletion was prepared",
        ));
    }
    if let Some(before_subagents) = subagent_metadata.as_ref() {
        let current_subagents = fs::symlink_metadata(&subagents)?;
        if current_subagents.file_type().is_symlink()
            || !current_subagents.is_dir()
            || !same_file_snapshot(before_subagents, &current_subagents)
        {
            return Err(std::io::Error::other(
                "Claude subagent directory changed while deletion was prepared",
            ));
        }
    }

    let transaction = private_temporary_directory(parent, "delete")?;
    let saved_transcript = transaction.join("transcript.jsonl");
    let saved_subagents = transaction.join("subagents");
    if let Err(error) = fs::rename(path, &saved_transcript) {
        let _ = fs::remove_dir(&transaction);
        return Err(error);
    }
    let mut moved_subagents = false;
    if subagent_metadata.is_some() {
        if let Err(error) = fs::rename(&subagents, &saved_subagents) {
            return rollback_delete(
                error,
                path,
                &saved_transcript,
                &subagents,
                &saved_subagents,
                false,
                &transaction,
            );
        }
        moved_subagents = true;
    }
    if let Err(error) = sync_directory(parent) {
        return rollback_delete(
            error,
            path,
            &saved_transcript,
            &subagents,
            &saved_subagents,
            moved_subagents,
            &transaction,
        );
    }

    // The provider-visible commit point is the atomic transcript rename. A
    // failed cleanup can leave only a hidden, complete recovery copy; it must
    // never turn a confirmed deletion into a blind retry of the destructive
    // operation.
    let _ = fs::remove_dir_all(&transaction);
    let _ = sync_directory(parent);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rollback_delete(
    original_error: std::io::Error,
    transcript: &Path,
    saved_transcript: &Path,
    subagents: &Path,
    saved_subagents: &Path,
    moved_subagents: bool,
    transaction: &Path,
) -> std::io::Result<()> {
    let mut rollback_error = None;
    if moved_subagents {
        if let Err(error) = restore_without_overwrite(saved_subagents, subagents) {
            rollback_error = Some(error);
        }
    }
    if let Err(error) = restore_without_overwrite(saved_transcript, transcript) {
        rollback_error.get_or_insert(error);
    }
    if rollback_error.is_none() {
        let _ = fs::remove_dir(transaction);
        return Err(original_error);
    }
    Err(std::io::Error::other(format!(
        "Claude deletion failed and rollback could not restore the provider store: {}",
        rollback_error.expect("checked rollback failure")
    )))
}

fn restore_without_overwrite(source: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::rename(source, destination)
        }
        Err(error) => Err(error),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "Claude provider path was replaced during rollback",
        )),
    }
}

struct SessionFileCandidates<'a> {
    scoped: Option<PathBuf>,
    root: &'a Path,
    file: &'a str,
}

impl<'a> SessionFileCandidates<'a> {
    fn new(root: &'a Path, cwd: &str, file: &'a str) -> Self {
        Self {
            scoped: (!cwd.is_empty()).then(|| root.join(project_dir_name(cwd)).join(file)),
            root,
            file,
        }
    }

    fn scoped(&self) -> Option<&Path> {
        self.scoped.as_deref()
    }

    fn fallbacks(&self) -> std::io::Result<impl Iterator<Item = PathBuf> + '_> {
        let scoped = self.scoped.as_ref();
        Ok(unscoped_candidates(self.root, self.file)?
            .into_iter()
            .filter(move |path| scoped != Some(path)))
    }
}

fn in_project<T>(
    root: &Path,
    session_id: &str,
    cwd: &str,
    mut operation: impl FnMut(&Path) -> std::io::Result<T>,
) -> std::io::Result<T> {
    if !provider_session_id(session_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Claude session id is invalid",
        ));
    }
    let file = format!("{session_id}.jsonl");
    let candidates = SessionFileCandidates::new(root, cwd, &file);
    // A mutation stops at the first error other than NotFound: it is never
    // retried against a second candidate file.
    let mut attempt = |path: &Path| match candidate_metadata(path) {
        Ok(Some(_)) => match operation(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            result => Some(result),
        },
        Ok(None) => None,
        Err(error) => Some(Err(error)),
    };
    if let Some(result) = candidates.scoped().and_then(&mut attempt) {
        return result;
    }
    for path in candidates.fallbacks()? {
        if let Some(result) = attempt(&path) {
            return result;
        }
    }
    Err(session_not_found())
}

fn read_history_page(
    root: &Path,
    session_id: &str,
    cwd: &str,
    limit: usize,
) -> std::io::Result<Option<(Vec<Value>, usize)>> {
    if !usable_session_id(session_id) {
        return Ok(None);
    }
    let file = format!("{session_id}.jsonl");
    let candidates = SessionFileCandidates::new(root, cwd, &file);
    // A read remembers the last error and keeps trying other candidates.
    let mut last_error = None;
    let mut attempt = |path: &Path| {
        candidate_metadata(path)
            .and_then(|found| found.map(|_| history_cache::read(path, limit)).transpose())
            .unwrap_or_else(|error| {
                last_error = Some(error);
                None
            })
    };
    if let Some(page) = candidates.scoped().and_then(&mut attempt) {
        return Ok(Some(page));
    }
    for path in candidates.fallbacks()? {
        if let Some(page) = attempt(&path) {
            return Ok(Some(page));
        }
    }
    last_error.map_or(Ok(None), Err)
}

/// The caller has already run `candidate_metadata`, which is the symlink and
/// regular-file guard, so this does not stat the path a second time.
fn fold_history_jsonl(path: &Path, limit: usize) -> std::io::Result<(Vec<Value>, usize)> {
    let mut kept = VecDeque::new();
    let mut matching = 0_usize;
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if !is_history_message(&value) {
            continue;
        }
        matching += 1;
        if limit != 0 && kept.len() == limit {
            kept.pop_front();
        }
        kept.push_back(value);
    }
    let older_messages = matching.saturating_sub(kept.len());
    Ok((Vec::from(kept), older_messages))
}

fn is_history_message(record: &Value) -> bool {
    let Some(record) = record.as_object() else {
        return false;
    };
    record.get("isSidechain").and_then(Value::as_bool) != Some(true)
        && record.get("isMeta").and_then(Value::as_bool) != Some(true)
        && nonempty(record.get("teamName")).is_none()
        && matches!(nonempty(record.get("type")), Some("user" | "assistant"))
}

fn unscoped_candidates(root: &Path, file: &str) -> std::io::Result<Vec<PathBuf>> {
    let projects = match fs::read_dir(root) {
        Ok(projects) => projects,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut candidates = Vec::new();
    for project in projects {
        let Ok(project) = project else {
            continue;
        };
        let Ok(kind) = project.file_type() else {
            continue;
        };
        if !kind.is_dir() || project.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        candidates.push(project.path().join(file));
    }
    candidates.sort();
    Ok(candidates)
}

fn candidate_metadata(path: &Path) -> std::io::Result<Option<fs::Metadata>> {
    match safe_file_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn private_temporary_file(parent: &Path, purpose: &str) -> std::io::Result<(PathBuf, File)> {
    let path = parent.join(format!(".interface-{purpose}-{}.tmp", Uuid::new_v4()));
    let file = create_private_new(&path)?;
    Ok((path, file))
}

fn private_temporary_directory(parent: &Path, purpose: &str) -> std::io::Result<PathBuf> {
    let path = parent.join(format!(".interface-{purpose}-{}.tmp", Uuid::new_v4()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&path)?;
    Ok(path)
}

fn parent_directory(path: &Path) -> std::io::Result<&Path> {
    path.parent()
        .ok_or_else(|| std::io::Error::other("Claude transcript has no project directory"))
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(unix)]
fn same_file_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(not(unix))]
fn same_file_snapshot(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

fn session_not_found() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotFound, "Claude session was not found")
}

fn safe_file_metadata(path: &Path) -> std::io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        Err(std::io::Error::other(
            "Claude transcript was not a safe regular file",
        ))
    } else {
        Ok(metadata)
    }
}

fn read_jsonl(path: &Path) -> std::io::Result<Vec<Value>> {
    safe_file_metadata(path)?;
    let mut records = Vec::new();
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        if let Ok(value) = serde_json::from_str(line.trim()) {
            records.push(value);
        }
    }
    Ok(records)
}

fn usable_session_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control)
}

fn provider_session_id(value: &str) -> bool {
    usable_session_id(value) && Uuid::parse_str(value).is_ok()
}

fn nonempty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn clip(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn epoch_iso() -> String {
    "1970-01-01T00:00:00Z".into()
}

fn system_time_iso(value: SystemTime) -> String {
    let seconds = value.duration_since(UNIX_EPOCH).map_or(0, |duration| {
        i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
    });
    unix_timestamp(seconds)
}

/// The Claude store's stored form carries no sub-second field.
fn unix_timestamp(seconds: i64) -> String {
    unix_seconds_iso(seconds, IsoFraction::None)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
