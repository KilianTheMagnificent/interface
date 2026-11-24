//! Threads surface v1 state: the leased single writer of `threads.v5.json`
//! and every rule the renderer must not decide — sections, woke and unread,
//! order keys, seeding, run stamps, auto-settle and restore isolation.
//!
//! Store version 2 (protocol 36) keeps each turn's end, the files its own
//! edits named and whether others shared its checkout. Store version 3
//! (protocol 46, review B4) adds the `handoff` lineage of a thread that
//! continues another on a different provider, and the lineage a prepared
//! workspace hands to the thread bound to it. Store version 4 (protocol 57,
//! team tools) adds the `child` lineage of a thread another thread's agent
//! started and the `teamTools` setting. Store version 5 (protocol 59, switch
//! provider) adds the chain of provider sessions a thread whose provider was
//! switched runs as (`chain`, `segmentOf`; `threads/chain.rs`). Each version
//! lives in its own file: when `threads.v5.json` does not exist,
//! `threads.v4.json` (else `threads.v3.json`, `threads.v2.json`, then
//! `threads.v1.json`) is read once, never changed, and carried over with the
//! newer fields absent (unknown, or the default), so a build from before
//! keeps reading the file it wrote.
//!
//! A thread is a session key (`engine#sessionId`) with an overlay record; the
//! session registry stays the source of sessions and is never written here.
//! Records keep what the wire view leaves out: full checkpoint ref names,
//! prompt previews and the workspaces prepared for launches not yet bound.
//! Everything except [`ThreadStore`]'s open and commit is a pure function
//! over [`ThreadsDocument`]. Design:
//! `docs/superpowers/specs/2026-10-06-threads-design.md`.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use interface_contracts::threads::{
    LineageRelation, OutsideAgents, PullRequestState, RunOutcome, SettledReason, ThreadLineage,
    ThreadPullRequest, ThreadRun, ThreadSection, ThreadSettings, ThreadWorkspace, TitleSource,
    TurnState, WorkspaceKept, WorkspaceMode,
};
use interface_storage::{JsonLimits, PrivateDirectory, StorageError, read_json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{SettingsError, leased::LeasedDocument};

mod chain;
mod view;

pub use chain::{
    ChainLink, ChainPart, HANDOFF_PREFIXES, THREADS_SWITCH_PLAIN_CODE, THREADS_SWITCH_SAME_CODE,
    is_handoff_preview,
};

/// The store's file in the private data root, beside `harness.v1.json`.
pub const THREADS_FILE_NAME: &str = "threads.v5.json";
/// [`ThreadsDocument::version`].
pub const THREADS_DOCUMENT_VERSION: u8 = 5;
/// Beside [`THREADS_FILE_NAME`]: generated titles were turned on once for
/// this store ([`ThreadStore::turn_generated_titles_on_once`]), so that is
/// never done again. Outside the document, whose accepted schema denies
/// unknown fields and which older builds still read.
pub const GENERATED_TITLES_ON_MARKER: &str = "threads.generated-titles-on";
/// Beside [`THREADS_FILE_NAME`]: the owner's "Pane tools without asking"
/// switch (protocol 71). It holds [`PANE_TOOLS_ON`] while the switch is on;
/// absent, unreadable or anything else means off, so a pane tool that types
/// into or starts another pane asks. Outside the document, whose accepted
/// schema denies unknown fields and which older builds still read.
pub const PANE_TOOLS_MARKER: &str = "threads.pane-tools-without-asking";
/// [`PANE_TOOLS_MARKER`]'s contents while the switch is on.
pub const PANE_TOOLS_ON: &[u8] = b"on\n";
/// [`PANE_TOOLS_MARKER`]'s contents once the owner turned the switch off.
pub const PANE_TOOLS_OFF: &[u8] = b"off\n";
/// The version 4 store, read once when [`THREADS_FILE_NAME`] does not exist
/// yet and never written again.
pub const THREADS_V4_FILE_NAME: &str = "threads.v4.json";
/// The version 4 file's [`ThreadsDocument::version`].
pub const THREADS_V4_VERSION: u8 = 4;
/// The version 3 store, read once when [`THREADS_FILE_NAME`] does not exist
/// yet and never written again.
pub const THREADS_V3_FILE_NAME: &str = "threads.v3.json";
/// The version 3 file's [`ThreadsDocument::version`].
pub const THREADS_V3_VERSION: u8 = 3;
/// The version 2 store, read once when [`THREADS_FILE_NAME`] does not exist
/// yet and never written again.
pub const THREADS_V2_FILE_NAME: &str = "threads.v2.json";
/// The version 2 file's [`ThreadsDocument::version`].
pub const THREADS_V2_VERSION: u8 = 2;
/// The version 1 store, read once when neither [`THREADS_FILE_NAME`] nor
/// [`THREADS_V2_FILE_NAME`] exists yet, and never written again.
pub const THREADS_LEGACY_FILE_NAME: &str = "threads.v1.json";
/// The legacy file's [`ThreadsDocument::version`].
pub const THREADS_LEGACY_VERSION: u8 = 1;
pub const MAX_RECORDS: usize = 4096;
/// Turns kept per record; the newest are kept.
pub const MAX_TURNS: usize = 200;
/// Workspaces prepared for launches that have not been bound yet.
pub const MAX_PREPARED: usize = 64;
/// A turn keeps this many characters of its normalized prompt.
pub const PREVIEW_CHARS: usize = 200;
/// A turn keeps at most this many of the files its edits named (the run
/// tap's own bound); past it, which files it touched is unknown.
pub const MAX_TURN_PATHS: usize = 1_000;
/// ... and at most this many bytes of them.
pub const MAX_TURN_PATH_BYTES: usize = 64 * 1024;
pub const DAY_MS: u64 = 86_400_000;
/// A prepared workspace never bound for this long is discarded at host start.
pub const PREPARED_TTL_MS: u64 = 3_600_000;
/// A record whose session has been missing from the registry this long is
/// dropped at the next reconcile.
pub const MISSING_TTL_MS: u64 = DAY_MS;
/// Auto-settle runs this often (and at host start).
pub const AUTO_SETTLE_INTERVAL_MS: u64 = 600_000;
/// One thread's pull request is looked up at most this often.
pub const PULL_REQUEST_INTERVAL_MS: u64 = 600_000;
pub const MAX_AUTO_SETTLE_DAYS: u32 = 3650;

pub const THREADS_UNKNOWN_CODE: &str = "threads-unknown";
pub const THREADS_INVALID_CODE: &str = "threads-invalid";
pub const THREADS_FULL_CODE: &str = "threads-full";
pub const THREADS_UNKNOWN_WORKSPACE_CODE: &str = "threads-unknown-workspace";
pub const THREADS_PREPARED_FULL_CODE: &str = "threads-prepared-full";
pub const THREADS_NOT_OWNED_CODE: &str = "threads-workspace-not-owned";
pub const THREADS_WORKTREE_MISSING_CODE: &str = "threads-worktree-missing";
pub const THREADS_NO_CHECKPOINT_CODE: &str = "threads-no-checkpoint";
pub const THREADS_BUSY_CODE: &str = "threads-busy";
pub const THREADS_SHARED_CODE: &str = "threads-workspace-shared";
pub const THREADS_UNKNOWN_TURN_CODE: &str = "threads-unknown-turn";
pub const THREADS_TURN_FILES_UNKNOWN_CODE: &str = "threads-turn-files-unknown";
pub const THREADS_NOT_SHIPPABLE_CODE: &str = "threads-not-shippable";
/// A commit message's first line keeps this many characters.
pub const COMMIT_SUBJECT_CHARS: usize = 72;
/// A commit message lists this many of the thread's newest prompts.
pub const COMMIT_PROMPTS: usize = 10;

const LOCK_NAME: &str = ".threads.lock";
const UNCERTAIN: &str =
    "Thread state needs recovery after an uncertain write. Restart Interface to retry.";
const BYTES: usize = 4 * 1024 * 1024;
/// A write refused at the byte cap is retried once after room is made down
/// to this size (see `ThreadsDocument::make_room`).
const SOFT_BYTES: usize = BYTES / 4 * 3;
/// Making room cuts each record to this many newest turns before it evicts
/// anything.
const TRIM_FLOOR: usize = 20;
const MAX_TIME: u64 = 8_640_000_000_000_000;
const MAX_KEY: usize = 512;
const MAX_PATH: usize = 4096;
const MAX_REF: usize = 512;
const MAX_TOKEN: usize = 64;
const MAX_URL: usize = 2048;
/// An order key longer than this spreads its whole section out again.
const ORDER_KEY_SPREAD_AT: usize = 24;
const ORDER_KEY_MAX: usize = 64;
/// Base 62 in ASCII order, so keys compare as plain strings.
const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The whole `threads.v5.json` document (or the `threads.v4.json`,
/// `threads.v3.json`, `threads.v2.json` or `threads.v1.json` it was carried
/// over from).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadsDocument {
    pub version: u8,
    /// Increments once per committed change. A fresh store (none on disk,
    /// or one quarantined at open) starts at the time it opened in
    /// milliseconds, so a renderer holding a view from before still takes
    /// the new one as newer.
    pub revision: u64,
    /// The first registry reconcile ran; its settle-stale rules never run again.
    pub seeded: bool,
    pub settings: ThreadSettings,
    pub records: Vec<ThreadRecord>,
    pub prepared: Vec<PreparedRecord>,
}

impl Default for ThreadsDocument {
    fn default() -> Self {
        Self {
            version: THREADS_DOCUMENT_VERSION,
            revision: 0,
            seeded: false,
            settings: ThreadSettings::default(),
            records: Vec::new(),
            prepared: Vec::new(),
        }
    }
}

/// One thread's overlay on its session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadRecord {
    pub key: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_order: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_order: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_reason: Option<SettledReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsettled_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_at: Option<u64>,
    pub auto_settle_disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_visited_at: Option<u64>,
    pub marked_unread: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ThreadRun>,
    /// Stored with `missing` absent; the view derives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<ThreadWorkspace>,
    /// The repository root behind `workspace`; absent outside git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<ThreadLineage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<ThreadPullRequest>,
    /// The last pull request lookup, whether or not it found one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_checked_at: Option<u64>,
    /// When a lookup first saw the pull request merged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_seen_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<TitleSource>,
    /// The owned worktree was released because the session was archived,
    /// deleted or forgotten. A worktree that stayed shows in the view as
    /// `workspace.kept`; unarchiving clears this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<WorkspaceRelease>,
    /// Interface opened this thread's pull request (or found it open while
    /// opening one): it is tracked by its number even while
    /// `trackPullRequests` is off. Store v2, added before release.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pull_request_tracked: bool,
    /// When reconcile first found the session missing from a complete
    /// registry (or off the Threads surface); cleared when it comes back.
    /// The record is dropped once it has been missing for
    /// [`MISSING_TTL_MS`], and only a missing record is ever evicted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_since: Option<u64>,
    /// Store v5: this record is one provider session of a thread whose
    /// provider was switched (its chain's first key and its activation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainLink>,
    /// Store v5: the head this earlier provider session belongs to. A
    /// record with `segmentOf` is never listed, auto-settled or seeded again;
    /// it keeps its session, turns and journal for a switch back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_of: Option<String>,
    pub turns: Vec<TurnRecord>,
}

impl ThreadRecord {
    /// A record with nothing but its identity: active, unordered, unread rules off.
    #[must_use]
    pub fn new(key: impl Into<String>, created_at: u64) -> Self {
        Self {
            key: key.into(),
            created_at,
            pinned_at: None,
            pin_order: None,
            active_order: None,
            settled_at: None,
            settled_reason: None,
            unsettled_at: None,
            snoozed_until: None,
            snoozed_at: None,
            auto_settle_disabled: false,
            last_visited_at: None,
            marked_unread: false,
            last_run: None,
            workspace: None,
            git_root: None,
            lineage: None,
            pull_request: None,
            pull_request_checked_at: None,
            merged_seen_at: None,
            title_source: None,
            release: None,
            pull_request_tracked: false,
            missing_since: None,
            chain: None,
            segment_of: None,
            turns: Vec::new(),
        }
    }

    /// From terminals while settled `external` (a Code pane or CLI session
    /// never brought into Threads), else settled, else snoozed while
    /// `snoozedUntil` is ahead, else pinned, else active.
    #[must_use]
    pub fn section(&self, now: u64) -> ThreadSection {
        if self.settled_at.is_some() {
            if self.settled_reason == Some(SettledReason::External) {
                ThreadSection::FromTerminals
            } else {
                ThreadSection::Settled
            }
        } else if self.snoozed_until.is_some_and(|until| until > now) {
            ThreadSection::Snoozed
        } else if self.pinned_at.is_some() {
            ThreadSection::Pinned
        } else {
            ThreadSection::Active
        }
    }

    /// The snooze ran out and the owner has not looked since.
    #[must_use]
    pub fn woke(&self, now: u64) -> bool {
        self.snoozed_until.is_some_and(|until| {
            until <= now && self.last_visited_at.is_none_or(|visited| visited < until)
        })
    }

    /// A run started and has not ended.
    #[must_use]
    pub fn running(&self) -> bool {
        self.last_run
            .as_ref()
            .is_some_and(|run| run.ended_at.is_none())
    }

    /// Marked unread, or the last run completed or failed after the last
    /// visit. A running thread is never unread; an interrupted run never makes
    /// one unread.
    #[must_use]
    pub fn unread(&self) -> bool {
        if self.running() {
            return false;
        }
        self.marked_unread
            || self.last_run.as_ref().is_some_and(|run| {
                matches!(
                    run.outcome,
                    Some(RunOutcome::Completed | RunOutcome::Failed)
                ) && run
                    .ended_at
                    .is_some_and(|ended| self.last_visited_at.is_none_or(|visited| ended > visited))
            })
    }

    /// The folder this thread works in: its worktree, else its project folder.
    #[must_use]
    pub fn workspace_dir(&self) -> Option<&str> {
        workspace_dir(self.workspace.as_ref()?)
    }

    /// The worktree this thread created and still owns.
    #[must_use]
    pub fn owned_worktree(&self) -> Option<&str> {
        owned_worktree(self)
    }

    /// Clears a settle (stamping `unsettledAt`) and any snooze.
    fn wake_up(&mut self, now: u64) -> bool {
        let was_settled = self.settled_at.take().is_some();
        if was_settled {
            self.settled_reason = None;
            self.unsettled_at = Some(now);
        }
        self.snoozed_until = None;
        self.snoozed_at = None;
        was_settled
    }

    fn settle(&mut self, reason: SettledReason, now: u64) {
        self.settled_at = Some(now);
        self.settled_reason = Some(reason);
        self.pinned_at = None;
        self.pin_order = None;
        self.snoozed_until = None;
        self.snoozed_at = None;
    }

    fn next_ordinal(&self) -> u32 {
        self.turns
            .last()
            .map_or(0, |turn| turn.ordinal.saturating_add(1))
    }

    /// The newest moment anything happened to the thread.
    fn last_activity(&self) -> u64 {
        [
            Some(self.created_at),
            self.unsettled_at,
            self.settled_at,
            self.last_run.as_ref().map(|run| run.started_at),
            self.last_run.as_ref().and_then(|run| run.ended_at),
            self.turns.last().map(|turn| turn.sent_at),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(0)
    }
}

/// A thread's suggested commit message: its title as the first line (at
/// most [`COMMIT_SUBJECT_CHARS`] characters; the first prompt's first line
/// when it has none), then its prompts' first lines as a list.
#[must_use]
pub fn commit_message(title: &str, prompts: &[String]) -> String {
    let first_line = |text: &str| {
        text.lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned()
    };
    let subject = Some(first_line(title))
        .filter(|line| !line.is_empty())
        .or_else(|| {
            prompts
                .iter()
                .map(|prompt| first_line(prompt))
                .find(|line| !line.is_empty())
        })
        .unwrap_or_else(|| "Changes from Interface".to_owned());
    let subject = clipped(&subject, COMMIT_SUBJECT_CHARS);
    let mut items = Vec::<String>::new();
    for prompt in prompts {
        let line = clipped(&first_line(prompt), 100);
        if !line.is_empty() && items.last() != Some(&line) {
            items.push(line);
        }
    }
    if items.is_empty() || (items.len() == 1 && items[0] == subject) {
        return subject;
    }
    let list = items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{subject}\n\n{list}")
}

/// `text`'s first `max` characters, with "…" when it was longer.
fn clipped(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut kept = text.chars().take(max.saturating_sub(1)).collect::<String>();
    kept.push('…');
    kept
}

/// The folder a workspace works in: its worktree, else its project folder.
#[must_use]
pub fn workspace_dir(workspace: &ThreadWorkspace) -> Option<&str> {
    match workspace.mode {
        WorkspaceMode::Worktree => workspace.worktree_path.as_deref(),
        WorkspaceMode::Local => {
            Some(workspace.project_root.as_str()).filter(|root| !root.is_empty())
        }
        WorkspaceMode::None => None,
    }
}

/// One prompt sent through Interface and the run it started.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TurnRecord {
    pub ordinal: u32,
    pub sent_at: u64,
    pub prompt_digest: String,
    pub state: TurnState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
    /// The first [`PREVIEW_CHARS`] characters of the normalized prompt.
    pub preview: String,
    /// The prompt was longer than its preview.
    pub preview_truncated: bool,
    /// When its run ended (store v2). Absent while it runs, for a turn from
    /// a v1 store, and for one closed without an end the run tap saw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// The files its run's own edits named, relative to the repository root,
    /// sorted (store v2). Absent when unknown: a turn from a v1 store, one
    /// outside git, a run whose start the tap never saw, or one past
    /// [`MAX_TURN_PATHS`] / [`MAX_TURN_PATH_BYTES`]. Changes made by shell
    /// commands are never in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub touched: Option<Vec<String>>,
    /// Someone else worked in its checkout when it ended (store v2), so its
    /// counts and its diff cover only `touched`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub shared: bool,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's `skip_serializing_if` passes a reference.
const fn is_false(value: &bool) -> bool {
    !*value
}

impl TurnRecord {
    /// A running turn for `prompt`, sent at `sent_at`.
    #[must_use]
    pub fn new(ordinal: u32, prompt: &str, sent_at: u64, start_ref: Option<String>) -> Self {
        let normalized = normalized_prompt(prompt);
        let preview = normalized.chars().take(PREVIEW_CHARS).collect::<String>();
        let preview_truncated = preview.len() < normalized.len();
        Self {
            ordinal,
            sent_at,
            prompt_digest: prompt_digest(prompt),
            state: TurnState::Running,
            start_ref,
            end_ref: None,
            files_changed: None,
            added: None,
            removed: None,
            preview,
            preview_truncated,
            ended_at: None,
            touched: None,
            shared: false,
        }
    }

    /// The files this turn's diff covers: `None` for its whole checkout,
    /// else the files its own edits named, when others shared the checkout.
    ///
    /// # Errors
    /// Refuses a turn in a shared checkout whose files are unknown or none:
    /// its whole diff would show someone else's changes as its own.
    pub fn diff_paths(&self) -> Result<Option<&[String]>, ThreadRefusal> {
        if !self.shared {
            return Ok(None);
        }
        match self.touched.as_deref() {
            Some(paths) if !paths.is_empty() => Ok(Some(paths)),
            _ => Err(ThreadRefusal::new(
                THREADS_TURN_FILES_UNKNOWN_CODE,
                "Others work in this checkout too, and Interface doesn't know which files this turn changed, so it can't show this turn's own changes.",
            )),
        }
    }

    /// The whole prompt, when the preview holds all of it.
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        (!self.preview_truncated).then_some(self.preview.as_str())
    }
}

/// A workspace prepared for a launch that has not been bound to a session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedRecord {
    pub token: String,
    pub cwd: String,
    pub workspace: ThreadWorkspace,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_root: Option<String>,
    /// This prepare created the worktree and its branch.
    pub created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_ref: Option<String>,
    pub created_at: u64,
    /// Store v3: the lineage the thread bound to this workspace takes (a
    /// Continue with's `handoff` from its parent). Its `at` is set at bind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<ThreadLineage>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceRelease {
    pub at: u64,
    pub outcome: ReleaseOutcome,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReleaseOutcome {
    Removed,
    /// The worktree had changes, so it stayed.
    KeptDirty,
    /// The worktree held ignored files (dependencies, build output, local
    /// settings), so it stayed for the owner to remove.
    KeptIgnored,
    Missing,
    /// A live session or another thread still used it.
    InUse,
    Failed,
}

impl ReleaseOutcome {
    /// Why the worktree stayed, for the view; `None` when it went (or the
    /// release failed, which says nothing about the worktree).
    #[must_use]
    pub const fn kept(self) -> Option<WorkspaceKept> {
        match self {
            Self::KeptDirty => Some(WorkspaceKept::Dirty),
            Self::KeptIgnored => Some(WorkspaceKept::Ignored),
            Self::InUse => Some(WorkspaceKept::InUse),
            Self::Removed | Self::Missing | Self::Failed => None,
        }
    }
}

/// A refused thread command: nothing changed. `code` is a stable `threads-…` string.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct ThreadRefusal {
    pub code: &'static str,
    pub message: String,
}

impl ThreadRefusal {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The refusal for a key that has no thread record.
    #[must_use]
    pub fn unknown() -> Self {
        Self::new(
            THREADS_UNKNOWN_CODE,
            "That thread is no longer in Interface. Refresh the list and try again.",
        )
    }

    #[must_use]
    pub fn invalid(message: &str) -> Self {
        Self::new(THREADS_INVALID_CODE, message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ThreadStoreError {
    #[error(transparent)]
    Refused(#[from] ThreadRefusal),
    #[error(transparent)]
    Storage(#[from] SettingsError),
}

// --- Names and digests -----------------------------------------------------

/// `"\r\n"` becomes `"\n"`, then the prompt is trimmed.
#[must_use]
pub fn normalized_prompt(prompt: &str) -> String {
    prompt.replace("\r\n", "\n").trim().to_owned()
}

/// The first 16 hex of sha256 of the normalized prompt.
#[must_use]
pub fn prompt_digest(prompt: &str) -> String {
    sha256_prefix(&normalized_prompt(prompt))
}

fn sha256_prefix(text: &str) -> String {
    use std::fmt::Write as _;
    Sha256::digest(text.as_bytes())[..8]
        .iter()
        .fold(String::with_capacity(16), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// `refs/interface/threads/<h>`, `<h>` being the first 16 hex of sha256(key).
#[must_use]
pub fn thread_ref_namespace(key: &str) -> String {
    format!("refs/interface/threads/{}", sha256_prefix(key))
}

/// `refs/interface/threads/<h>/<ordinal>/start` (or `/end`).
#[must_use]
pub fn turn_ref(key: &str, ordinal: u32, end: bool) -> String {
    let side = if end { "end" } else { "start" };
    format!("{}/{ordinal}/{side}", thread_ref_namespace(key))
}

/// Where `PrepareWorkspace` keeps a baseline before the thread has a key.
#[must_use]
pub fn pending_baseline_ref(token: &str) -> String {
    format!("refs/interface/threads/pending/{token}")
}

/// `refs/interface/threads/<h>/recovery/<ms>`, captured before a restore.
#[must_use]
pub fn recovery_ref(key: &str, at: u64) -> String {
    format!("{}/recovery/{at}", thread_ref_namespace(key))
}

// --- Order keys ------------------------------------------------------------

/// A base-62 fractional-index key strictly between `lower` and `upper`
/// (`None` is an open end). Keys never end in `0`, so there is always room
/// before one. An inverted pair is treated as having no upper bound.
#[must_use]
pub fn order_key_between(lower: Option<&str>, upper: Option<&str>) -> String {
    let lower = lower.unwrap_or("");
    let upper = upper.filter(|upper| !upper.is_empty() && lower < *upper);
    midpoint(lower.as_bytes(), upper.map(str::as_bytes))
        .into_iter()
        .map(char::from)
        .collect()
}

fn digit_value(byte: u8) -> usize {
    DIGITS.iter().position(|digit| *digit == byte).unwrap_or(0)
}

fn midpoint(lower: &[u8], upper: Option<&[u8]>) -> Vec<u8> {
    if let Some(upper) = upper {
        let mut common = 0;
        while common < upper.len()
            && lower.get(common).copied().unwrap_or(DIGITS[0]) == upper[common]
        {
            common += 1;
        }
        if common > 0 {
            let mut key = upper[..common].to_vec();
            let rest = upper.get(common..).filter(|rest| !rest.is_empty());
            key.extend(midpoint(lower.get(common..).unwrap_or_default(), rest));
            return key;
        }
    }
    let low = lower.first().map_or(0, |byte| digit_value(*byte));
    let high = upper.map_or(DIGITS.len(), |upper| digit_value(upper[0]));
    if high.saturating_sub(low) > 1 {
        vec![DIGITS[(low + high).div_ceil(2)]]
    } else if let Some(upper) = upper.filter(|upper| upper.len() > 1) {
        vec![upper[0]]
    } else {
        let mut key = vec![DIGITS[low]];
        key.extend(midpoint(lower.get(1..).unwrap_or_default(), None));
        key
    }
}

/// `count` evenly spaced keys in ascending order, as short as `count` allows.
fn spread_keys(count: usize) -> Vec<String> {
    let slots = u64::try_from(count).unwrap_or(u64::MAX).saturating_add(1);
    let mut width = 1;
    let mut span = 62_u64;
    while span <= slots {
        width += 1;
        span = span.saturating_mul(62);
    }
    let step = span / slots;
    (1..=slots.saturating_sub(1))
        .map(|slot| {
            let mut value = step * slot;
            let mut digits = vec![DIGITS[0]; width];
            for digit in digits.iter_mut().rev() {
                *digit = DIGITS[usize::try_from(value % 62).unwrap_or(0)];
                value /= 62;
            }
            while digits.last() == Some(&DIGITS[0]) {
                digits.pop();
            }
            digits.into_iter().map(char::from).collect()
        })
        .collect()
}

fn order_key_valid(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= ORDER_KEY_MAX
        && key.bytes().all(|byte| DIGITS.contains(&byte))
        && !key.ends_with('0')
}

/// Which of a record's order keys a section uses.
#[derive(Clone, Copy)]
enum OrderField {
    Pin,
    Active,
}

impl OrderField {
    fn get(self, record: &ThreadRecord) -> Option<&str> {
        match self {
            Self::Pin => record.pin_order.as_deref(),
            Self::Active => record.active_order.as_deref(),
        }
    }

    fn set(self, record: &mut ThreadRecord, key: String) {
        match self {
            Self::Pin => record.pin_order = Some(key),
            Self::Active => record.active_order = Some(key),
        }
    }

    const fn section(self) -> ThreadSection {
        match self {
            Self::Pin => ThreadSection::Pinned,
            Self::Active => ThreadSection::Active,
        }
    }
}

fn keyed_first(left: Option<&str>, right: Option<&str>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

// --- Seeding inputs and plans ----------------------------------------------

/// Where a session came from, as the registry says.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeedOrigin {
    /// Started in Interface.
    App,
    /// Found in provider history (a terminal CLI run, another machine).
    Store,
}

/// One registry session as reconcile sees it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSession {
    pub key: String,
    pub origin: SeedOrigin,
    pub first_seen: Option<u64>,
    pub last_activity: Option<u64>,
    /// Working or waiting on the owner.
    pub busy: bool,
    pub archived: bool,
    /// A Threads-surface session: not an agent's, not an Away or
    /// orchestration worker.
    pub listed: bool,
}

/// What a reconcile changed that the host acts on.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Reconciled {
    /// Records whose session has been missing for [`MISSING_TTL_MS`]. Their
    /// worktrees stay: only an archive, delete or forget releases one.
    pub dropped: Vec<ThreadRecord>,
    /// Archived threads whose owned worktree has not been released yet.
    pub to_release: Vec<String>,
}

/// One registry session's working folder, for isolation checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionUse {
    pub key: String,
    pub cwd: String,
    /// The runtime holds a live attachment for it.
    pub live: bool,
    pub archived: bool,
    /// Its registry title, which a refusal names it by.
    pub title: String,
}

/// What else uses a worktree, as [`ThreadsDocument::shared_with`] finds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorktreeUse {
    /// A session with a live provider attachment works inside or around it.
    Live(String),
    /// An unarchived thread's folder overlaps it.
    Thread(String),
}

impl WorktreeUse {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Live(key) | Self::Thread(key) => key,
        }
    }
}

/// An allowed restore: where, to which checkpoint, and the recovery ref to
/// capture first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlan {
    pub dir: String,
    pub start_ref: String,
    pub recovery_ref: String,
}

/// An allowed worktree release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleasePlan {
    pub git_root: String,
    pub worktree_path: String,
}

/// A git thread's turn whose run ended: its end checkpoint (when it has a
/// start one) and the files its edits named are still to record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinishedTurn {
    pub key: String,
    pub ordinal: u32,
    pub dir: String,
    /// `None`: the turn has no start checkpoint, so no end is captured.
    pub start_ref: Option<String>,
    pub end_ref: String,
}

/// What a finished turn's end recorded, for [`ThreadsDocument::record_turn_end`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TurnEnd {
    /// The end checkpoint, when one was captured.
    pub end_ref: Option<String>,
    /// `(files, added, removed)`, when they were measured.
    pub counts: Option<(u32, u32, u32)>,
    /// The files its edits named, relative to the repository root; `None`
    /// unknown.
    pub touched: Option<Vec<String>>,
    /// Others worked in its checkout ([`TurnMeasure`] other than `Checkout`).
    pub shared: bool,
}

/// What a finished turn's counts measure, as [`ThreadsDocument::turn_measure`]
/// decides it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnMeasure {
    /// Every change in its checkout: nobody else works there, so they are
    /// all the turn's own, shell commands' edits included.
    Checkout,
    /// Only these files, as the turn's own edits named them (absolute, or
    /// relative to its folder): others work in the checkout too.
    Files(Vec<String>),
    /// Nothing: others work in the checkout and which files the turn's own
    /// edits touched is unknown, or none were named. The counts stay unknown
    /// rather than take someone else's lines.
    Unknown,
}

/// A pull request lookup to make.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestCheck {
    pub key: String,
    pub project_root: String,
    pub by: PullRequestLookupBy,
}

/// How a thread's pull request is looked up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PullRequestLookupBy {
    /// A worktree thread's branch: its newest pull request in any state.
    Branch(String),
    /// The pull request the thread already has (one Interface opened from a
    /// Local thread, say): looked up by its number.
    Number(u64),
}

/// What shipping a thread works on, from its record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShipScope {
    /// The folder the thread works in.
    pub dir: String,
    /// A worktree thread: its worktree is its own, so Commit stages every
    /// change in it. A Local thread shares the owner's checkout.
    pub worktree: bool,
    /// A Local thread's files: every repository-relative file its turns'
    /// edits named. `None` for a worktree thread.
    pub touched: Option<BTreeSet<String>>,
    /// Files explicitly named by finished turns, including worktree threads.
    /// Owning a worktree does not establish who edited every file in it.
    pub attributed: BTreeSet<String>,
    /// A Local thread has finished turns whose files are unknown.
    pub touched_unknown: bool,
    /// The pull request the thread has, if it has one.
    pub pull_request: Option<ThreadPullRequest>,
    /// The normalized prompts of its newest turns, oldest first, at most
    /// [`COMMIT_PROMPTS`] (previews: the first [`PREVIEW_CHARS`] characters).
    pub prompts: Vec<String>,
}

/// What `gh pr list` found for a branch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FoundPullRequest {
    pub url: String,
    pub number: u64,
    pub state: PullRequestState,
}

/// A launch to bind: its session, prepared workspace and launch prompt.
#[derive(Clone, Debug, Default)]
pub struct ThreadBind {
    pub key: String,
    /// From `PrepareWorkspace`; absent for a plain chat.
    pub token: Option<String>,
    pub prompt: String,
    pub title_source: Option<TitleSource>,
    /// Turn 0's start ref when the baseline moved under the thread's namespace.
    pub start_ref: Option<String>,
}

/// A fork (or an edit) to record.
#[derive(Clone, Debug)]
pub struct ThreadFork {
    pub parent: String,
    pub child: String,
    pub relation: LineageRelation,
    pub kept_turns: u32,
}

/// The recorded turn a fork keeps last, by identity: its prompt digest and
/// how many later recorded turns carry the same digest. The provider adapter
/// finds that prompt from the newest end of its own history, so prompts the
/// host never recorded cannot shift the cut.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkPoint {
    pub prompt_digest: String,
    pub later_same_digest: u32,
}

/// The record state a send changed, to put back if it was never delivered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BegunTurn {
    pub ordinal: u32,
    pub previous_run: Option<ThreadRun>,
}

// --- The document's transitions ---------------------------------------------

impl ThreadsDocument {
    /// Drops `key`'s record, if it has one (protocol 67: a side question's
    /// side thread is never a thread, even when a reconcile seeded it before
    /// it was hidden). Answers whether it had one.
    pub fn drop_record(&mut self, key: &str) -> bool {
        let before = self.records.len();
        self.records.retain(|record| record.key != key);
        before != self.records.len()
    }

    #[must_use]
    pub fn record(&self, key: &str) -> Option<&ThreadRecord> {
        self.records.iter().find(|record| record.key == key)
    }

    /// Whether `key`'s launch is still waiting for its bind: the session
    /// has no thread record yet, or only the one a reconcile seeded (no
    /// workspace, no turn), and a prepared workspace for its folder `cwd`
    /// has not been bound. A new thread is launched, then bound with the
    /// prepared workspace's token; a session nothing prepared a workspace
    /// for (a plain chat, an agent's) never awaits one.
    #[must_use]
    pub fn awaits_bind(&self, key: &str, cwd: &str) -> bool {
        let unbound = self
            .record(key)
            .is_none_or(|record| record.workspace.is_none() && record.turns.is_empty());
        if !unbound || cwd.is_empty() {
            return false;
        }
        // The provider reports the folder it runs in by its real path, while
        // the workspace keeps the path as picked: a project reached through a
        // link (on macOS `/var` and `/tmp` are links) is the same folder.
        let real = |path: &Path| std::fs::canonicalize(path).ok();
        let cwd_path = Path::new(cwd);
        let cwd = interface_common::paths::lexically_normalized(cwd_path);
        let mut cwd_real = None;
        self.prepared.iter().any(|prepared| {
            let prepared = Path::new(&prepared.cwd);
            interface_common::paths::lexically_normalized(prepared) == cwd
                || real(prepared).is_some_and(|prepared| {
                    cwd_real.get_or_insert_with(|| real(cwd_path)).as_ref() == Some(&prepared)
                })
        })
    }

    fn index(&self, key: &str) -> Result<usize, ThreadRefusal> {
        self.records
            .iter()
            .position(|record| record.key == key)
            .ok_or_else(ThreadRefusal::unknown)
    }

    #[must_use]
    pub fn prepared(&self, token: &str) -> Option<&PreparedRecord> {
        self.prepared
            .iter()
            .find(|prepared| prepared.token == token)
    }

    /// Gives `index` a key before every other record of the field's section.
    fn place_first(&mut self, index: usize, field: OrderField, now: u64) {
        // Inserting at the front needs only the first key. Sorting and
        // allocating the whole section per new session made bulk seeding
        // quadratic-logarithmic. Sort only when the keys need spreading.
        let first = self
            .records
            .iter()
            .enumerate()
            .filter(|(other, record)| {
                *other != index && record.listed() && record.section(now) == field.section()
            })
            .map(|(other, _)| other)
            .min_by(|&left, &right| self.compare_records(left, right, field.section()))
            .and_then(|other| field.get(&self.records[other]));
        let key = order_key_between(None, first);
        let spread = key.len() > ORDER_KEY_SPREAD_AT;
        field.set(&mut self.records[index], key);
        if spread {
            let mut order = vec![index];
            order.extend(
                self.ordered(field.section(), now)
                    .into_iter()
                    .filter(|&other| other != index),
            );
            self.spread(field, &order);
        }
    }

    fn spread(&mut self, field: OrderField, order: &[usize]) {
        for (&index, key) in order.iter().zip(spread_keys(order.len())) {
            field.set(&mut self.records[index], key);
        }
    }

    /// # Errors
    /// Refuses an unknown thread.
    pub fn pin(&mut self, key: &str, now: u64) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        let record = &mut self.records[index];
        if record.pinned_at.is_some() && record.settled_at.is_none() {
            return Ok(());
        }
        // Pinning un-settles; a snooze stays.
        if record.settled_at.take().is_some() {
            record.settled_reason = None;
            record.unsettled_at = Some(now);
        }
        record.pinned_at = Some(now);
        self.place_first(index, OrderField::Pin, now);
        Ok(())
    }

    /// # Errors
    /// Refuses an unknown thread.
    pub fn unpin(&mut self, key: &str, now: u64) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        let record = &mut self.records[index];
        if record.pinned_at.take().is_none() {
            return Ok(());
        }
        record.pin_order = None;
        self.place_first(index, OrderField::Active, now);
        Ok(())
    }

    /// Moves `key` into `section` (pinning, unpinning, waking or un-settling
    /// it as needed) and places it before `before`, or last when `before` is
    /// absent or not in that section.
    ///
    /// # Errors
    /// Refuses an unknown thread and any section but pinned and active.
    pub fn reorder(
        &mut self,
        key: &str,
        section: ThreadSection,
        before: Option<&str>,
        now: u64,
    ) -> Result<(), ThreadRefusal> {
        let field = match section {
            ThreadSection::Pinned => OrderField::Pin,
            ThreadSection::Active => OrderField::Active,
            ThreadSection::Snoozed | ThreadSection::Settled | ThreadSection::FromTerminals => {
                return Err(ThreadRefusal::invalid(
                    "Only pinned and active threads can be reordered.",
                ));
            }
        };
        let index = self.index(key)?;
        let record = &mut self.records[index];
        record.wake_up(now);
        match field {
            OrderField::Pin => {
                record.pinned_at.get_or_insert(now);
            }
            OrderField::Active => {
                record.pinned_at = None;
                record.pin_order = None;
            }
        }
        let others = self
            .ordered(section, now)
            .into_iter()
            .filter(|&other| other != index)
            .collect::<Vec<_>>();
        let position = before
            .and_then(|before| {
                others
                    .iter()
                    .position(|&other| self.records[other].key == before)
            })
            .unwrap_or(others.len());
        if others
            .iter()
            .any(|&other| field.get(&self.records[other]).is_none())
        {
            self.spread(field, &others);
        }
        let lower = position
            .checked_sub(1)
            .and_then(|previous| field.get(&self.records[others[previous]]))
            .map(str::to_owned);
        let upper = others
            .get(position)
            .and_then(|&next| field.get(&self.records[next]))
            .map(str::to_owned);
        let ordered_key = order_key_between(lower.as_deref(), upper.as_deref());
        let spread = ordered_key.len() > ORDER_KEY_SPREAD_AT;
        field.set(&mut self.records[index], ordered_key);
        if spread {
            let mut order = others;
            order.insert(position, index);
            self.spread(field, &order);
        }
        Ok(())
    }

    /// Settles `key`, clearing its pin and snooze. Settling a settled thread
    /// keeps its first settle.
    ///
    /// # Errors
    /// Refuses an unknown thread.
    pub fn settle(
        &mut self,
        key: &str,
        reason: SettledReason,
        now: u64,
    ) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        let record = &mut self.records[index];
        if record.settled_at.is_none() {
            record.settle(reason, now);
        }
        Ok(())
    }

    /// Un-settles `key` to the top of Active.
    ///
    /// # Errors
    /// Refuses an unknown thread.
    pub fn unsettle(&mut self, key: &str, now: u64) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        if self.records[index].settled_at.is_none() {
            return Ok(());
        }
        self.records[index].wake_up(now);
        self.place_first(index, OrderField::Active, now);
        Ok(())
    }

    /// Snoozes `key` until `until`, keeping its pin and order. A settled
    /// thread un-settles.
    ///
    /// # Errors
    /// Refuses an unknown thread and a time that is not in the future.
    pub fn snooze(&mut self, key: &str, until: u64, now: u64) -> Result<(), ThreadRefusal> {
        if until <= now || until > MAX_TIME {
            return Err(ThreadRefusal::invalid(
                "Pick a time in the future to snooze until.",
            ));
        }
        let index = self.index(key)?;
        let record = &mut self.records[index];
        if record.settled_at.take().is_some() {
            record.settled_reason = None;
            record.unsettled_at = Some(now);
        }
        record.snoozed_until = Some(until);
        record.snoozed_at = Some(now);
        Ok(())
    }

    /// # Errors
    /// Refuses an unknown thread.
    pub fn unsnooze(&mut self, key: &str) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        let record = &mut self.records[index];
        record.snoozed_until = None;
        record.snoozed_at = None;
        Ok(())
    }

    /// The owner looked at `key`: read, and awake if it woke.
    ///
    /// # Errors
    /// Refuses an unknown thread.
    pub fn visit(&mut self, key: &str, now: u64) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        let record = &mut self.records[index];
        record.last_visited_at = Some(now);
        record.marked_unread = false;
        Ok(())
    }

    /// # Errors
    /// Refuses an unknown thread.
    pub fn mark_unread(&mut self, key: &str) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        self.records[index].marked_unread = true;
        Ok(())
    }

    /// # Errors
    /// Refuses an unknown thread.
    pub fn set_auto_settle(&mut self, key: &str, enabled: bool) -> Result<(), ThreadRefusal> {
        let index = self.index(key)?;
        self.records[index].auto_settle_disabled = !enabled;
        Ok(())
    }

    /// The outside agents level (protocol 65) is never kept here: the team
    /// tools ledger keeps it, so a Threads store an older build reads never
    /// carries a field it does not know. Pane tools without asking (protocol
    /// 71) is not either: [`PANE_TOOLS_MARKER`] beside the store keeps it.
    ///
    /// # Errors
    /// Refuses more than [`MAX_AUTO_SETTLE_DAYS`].
    pub fn set_settings(&mut self, mut settings: ThreadSettings) -> Result<(), ThreadRefusal> {
        if settings.auto_settle_days > MAX_AUTO_SETTLE_DAYS {
            return Err(ThreadRefusal::invalid(
                "Auto-settle accepts at most 3650 days (0 turns it off).",
            ));
        }
        settings.outside_agents = OutsideAgents::Off;
        settings.pane_tools_without_asking = false;
        self.settings = settings;
        Ok(())
    }

    /// # Errors
    /// Refuses a reused token and a full prepared list.
    pub fn add_prepared(&mut self, prepared: PreparedRecord) -> Result<(), ThreadRefusal> {
        if self.prepared.len() >= MAX_PREPARED {
            return Err(prepared_full());
        }
        if self.prepared(&prepared.token).is_some() {
            return Err(ThreadRefusal::invalid(
                "That workspace token is already in use.",
            ));
        }
        self.prepared.push(prepared);
        Ok(())
    }

    /// Whether another workspace may be prepared now.
    ///
    /// # Errors
    /// Refuses when [`MAX_PREPARED`] launches are still unbound.
    pub fn ensure_prepared_room(&self) -> Result<(), ThreadRefusal> {
        if self.prepared.len() >= MAX_PREPARED {
            return Err(prepared_full());
        }
        Ok(())
    }

    pub fn take_prepared(&mut self, token: &str) -> Option<PreparedRecord> {
        let index = self
            .prepared
            .iter()
            .position(|prepared| prepared.token == token)?;
        Some(self.prepared.remove(index))
    }

    /// The tokens of every prepared workspace older than [`PREPARED_TTL_MS`].
    #[must_use]
    pub fn stale_prepared(&self, now: u64) -> Vec<String> {
        self.prepared
            .iter()
            .filter(|prepared| prepared.created_at.saturating_add(PREPARED_TTL_MS) <= now)
            .map(|prepared| prepared.token.clone())
            .collect()
    }

    /// Adds a record for `key` (created at `created_at`) if it has none,
    /// making room by dropping the oldest settled record. Answers its index.
    fn ensure_record(&mut self, key: &str, created_at: u64) -> Result<usize, ThreadRefusal> {
        if let Some(index) = self.records.iter().position(|record| record.key == key) {
            return Ok(index);
        }
        if self.records.len() >= MAX_RECORDS && !self.evict_oldest_missing() {
            return Err(ThreadRefusal::new(
                THREADS_FULL_CODE,
                "Interface keeps at most 4096 threads. Delete some you no longer need, then try again.",
            ));
        }
        self.records.push(ThreadRecord::new(key, created_at));
        Ok(self.records.len() - 1)
    }

    /// Drops the record whose session has been missing longest. A record
    /// whose session is still listed is never evicted: it would seed again
    /// as a new, active thread. Answers whether one went.
    fn evict_oldest_missing(&mut self) -> bool {
        self.evict_oldest_missing_freeing().is_some()
    }

    /// [`Self::evict_oldest_missing`], answering about how many bytes went.
    fn evict_oldest_missing_freeing(&mut self) -> Option<usize> {
        let (_, index) = self
            .records
            .iter()
            .enumerate()
            .filter_map(|(index, record)| record.missing_since.map(|at| (at, index)))
            .min()?;
        Some(encoded_len(&self.records.remove(index)))
    }

    /// Binds a launch: the thread goes to the top of Active with its
    /// workspace, title source and turn 0 (the launch prompt, on the prepared
    /// baseline). A run already over finishes turn 0 and answers it for its
    /// end checkpoint. Binding again keeps the existing turns.
    ///
    /// # Errors
    /// Refuses an unknown token and a full store.
    pub fn bind(
        &mut self,
        bind: ThreadBind,
        now: u64,
    ) -> Result<Option<FinishedTurn>, ThreadRefusal> {
        let prepared = match bind.token.as_deref() {
            Some(token) => Some(self.take_prepared(token).ok_or_else(|| {
                ThreadRefusal::new(
                    THREADS_UNKNOWN_WORKSPACE_CODE,
                    "That prepared workspace was discarded or expired. Start the thread again.",
                )
            })?),
            None => None,
        };
        let index = self.ensure_record(&bind.key, now)?;
        let owner = prepared
            .as_ref()
            .and_then(|prepared| self.settled_owner(&bind.key, &prepared.workspace));
        if let Some(owner) = owner
            && let Some(workspace) = self.records[owner].workspace.as_mut()
        {
            workspace.owned = false;
        }
        let record = &mut self.records[index];
        record.wake_up(now);
        record.pinned_at = None;
        record.pin_order = None;
        record.title_source = Some(bind.title_source.unwrap_or(TitleSource::Prompt));
        let baseline = if let Some(mut prepared) = prepared {
            prepared.workspace.owned |= owner.is_some();
            if let Some(lineage) = prepared.lineage {
                record.lineage = Some(ThreadLineage { at: now, ..lineage });
            }
            record.workspace = Some(prepared.workspace);
            record.git_root = prepared.git_root;
            bind.start_ref.or(prepared.baseline_ref)
        } else {
            record.workspace = Some(ThreadWorkspace {
                mode: WorkspaceMode::None,
                project_root: String::new(),
                worktree_path: None,
                branch: None,
                base_ref: None,
                owned: false,
                missing: None,
                kept: None,
            });
            record.git_root = None;
            None
        };
        if record.turns.is_empty() {
            record
                .turns
                .push(TurnRecord::new(0, &bind.prompt, now, baseline));
        }
        let run = record
            .last_run
            .get_or_insert(ThreadRun {
                started_at: now,
                ended_at: None,
                outcome: None,
            })
            .clone();
        self.place_first(index, OrderField::Active, now);
        Ok(run
            .outcome
            .zip(run.ended_at)
            .and_then(|(outcome, ended)| self.finish_running_turn(index, outcome, ended)))
    }

    /// Binds a session created with no prompt (an Away loop's work
    /// session, or a thread team tools started): the thread goes to the top
    /// of Active with the prepared workspace, its lineage (store v4: a
    /// started thread's `child`) and `title_source`, and no turn or run; its
    /// first send records turn 0 on a checkpoint of its own. Answers the
    /// repository root and the prepared baseline ref, which no turn uses,
    /// for the caller to delete.
    ///
    /// # Errors
    /// Refuses an unknown token and a full store.
    pub fn bind_empty(
        &mut self,
        key: &str,
        token: &str,
        title_source: TitleSource,
        now: u64,
    ) -> Result<Option<(String, String)>, ThreadRefusal> {
        let prepared = self.take_prepared(token).ok_or_else(|| {
            ThreadRefusal::new(
                THREADS_UNKNOWN_WORKSPACE_CODE,
                "That prepared workspace was discarded or expired. Start the thread again.",
            )
        })?;
        let index = self.ensure_record(key, now)?;
        let record = &mut self.records[index];
        record.wake_up(now);
        record.pinned_at = None;
        record.pin_order = None;
        record.title_source = Some(title_source);
        let baseline = prepared.git_root.clone().zip(prepared.baseline_ref);
        if let Some(lineage) = prepared.lineage {
            record.lineage = Some(ThreadLineage { at: now, ..lineage });
        }
        record.workspace = Some(prepared.workspace);
        record.git_root = prepared.git_root;
        self.place_first(index, OrderField::Active, now);
        Ok(baseline)
    }

    /// The settled thread (other than `key`) that owns the existing worktree
    /// `workspace` names. A thread started in it takes the ownership over:
    /// that is how "Edit from here" on the first prompt hands the worktree
    /// on, after restoring it and settling the original.
    fn settled_owner(&self, key: &str, workspace: &ThreadWorkspace) -> Option<usize> {
        let path = Path::new(workspace.worktree_path.as_deref()?);
        if workspace.mode != WorkspaceMode::Worktree || workspace.owned {
            return None;
        }
        self.records.iter().position(|record| {
            record.key != key
                && record.settled_at.is_some()
                && owned_worktree(record).is_some_and(|owned| Path::new(owned) == path)
        })
    }

    /// Records a fork (or an edit): the child goes to the top of Active with
    /// its lineage, the parent's workspace (shared, so not owned) and the
    /// parent's first `kept_turns` turns. An edit moves worktree ownership to
    /// the child and settles the parent as edited.
    ///
    /// # Errors
    /// Refuses an unknown parent and a full store.
    pub fn bind_fork(&mut self, fork: &ThreadFork, now: u64) -> Result<(), ThreadRefusal> {
        let parent = self.records[self.index(&fork.parent)?].clone();
        let index = self.ensure_record(&fork.child, now)?;
        let edit = fork.relation == LineageRelation::Edit;
        let record = &mut self.records[index];
        record.wake_up(now);
        record.pinned_at = None;
        record.pin_order = None;
        record.last_run = None;
        record.lineage = Some(ThreadLineage {
            parent: fork.parent.clone(),
            relation: fork.relation,
            kept_turns: fork.kept_turns,
            at: now,
        });
        record.workspace = parent.workspace.clone().map(|mut workspace| {
            workspace.owned = edit && workspace.owned;
            workspace
        });
        record.git_root.clone_from(&parent.git_root);
        record.turns = parent
            .turns
            .iter()
            .filter(|turn| turn.ordinal < fork.kept_turns)
            .cloned()
            .collect();
        self.place_first(index, OrderField::Active, now);
        if edit {
            let parent = self.index(&fork.parent)?;
            let parent = &mut self.records[parent];
            if let Some(workspace) = parent.workspace.as_mut() {
                workspace.owned = false;
            }
            parent.settle(SettledReason::Edited, now);
        }
        Ok(())
    }

    /// Where a fork keeping the first `keep_turns` of `key`'s recorded turns
    /// cuts: after turn `keep_turns - 1` (`ForkAt.keepTurns` and
    /// `EditFrom.ordinal` count the thread's recorded turns, not the
    /// provider's prompts).
    ///
    /// # Errors
    /// Refuses an unknown thread, keeping no turn, and a turn the record no
    /// longer holds.
    pub fn fork_point(&self, key: &str, keep_turns: u32) -> Result<ForkPoint, ThreadRefusal> {
        let record = &self.records[self.index(key)?];
        let kept = keep_turns
            .checked_sub(1)
            .and_then(|ordinal| record.turns.iter().find(|turn| turn.ordinal == ordinal))
            .ok_or_else(|| {
                ThreadRefusal::new(
                    THREADS_UNKNOWN_TURN_CODE,
                    "Interface no longer has a record of that turn, so it cannot fork there.",
                )
            })?;
        let later = record
            .turns
            .iter()
            .filter(|turn| turn.ordinal > kept.ordinal && turn.prompt_digest == kept.prompt_digest)
            .count();
        Ok(ForkPoint {
            prompt_digest: kept.prompt_digest.clone(),
            later_same_digest: u32::try_from(later).unwrap_or(u32::MAX),
        })
    }

    /// A send is going out: appends a running turn, starts the run, and
    /// brings a settled or snoozed thread back to Active. A turn still
    /// running from an earlier send is closed as done.
    ///
    /// # Errors
    /// Refuses an unknown thread.
    pub fn begin_turn(
        &mut self,
        key: &str,
        prompt: &str,
        start_ref: Option<String>,
        now: u64,
    ) -> Result<BegunTurn, ThreadRefusal> {
        let index = self.index(key)?;
        let record = &mut self.records[index];
        let ordinal = record.next_ordinal();
        for turn in &mut record.turns {
            if turn.state == TurnState::Running {
                turn.state = TurnState::Done;
            }
        }
        record
            .turns
            .push(TurnRecord::new(ordinal, prompt, now, start_ref));
        let previous_run = record.last_run.replace(ThreadRun {
            started_at: now,
            ended_at: None,
            outcome: None,
        });
        if record.wake_up(now) {
            self.place_first(index, OrderField::Active, now);
        }
        Ok(BegunTurn {
            ordinal,
            previous_run,
        })
    }

    /// Takes back a turn whose send was never delivered.
    pub fn cancel_turn(&mut self, key: &str, begun: &BegunTurn, started_at: u64) {
        let Ok(index) = self.index(key) else {
            return;
        };
        let record = &mut self.records[index];
        if record
            .turns
            .last()
            .is_some_and(|turn| turn.ordinal == begun.ordinal)
        {
            record.turns.pop();
        }
        // The send's run, even if the tap already saw it end (a provider's
        // own error and idle can reach the tap before the refusal does).
        if record
            .last_run
            .as_ref()
            .is_some_and(|run| run.started_at == started_at)
        {
            record.last_run.clone_from(&begun.previous_run);
        }
    }

    /// A run started outside a send this host recorded (a launch prompt, a
    /// steer). Answers whether anything changed.
    pub fn run_started(&mut self, key: &str, at: u64) -> bool {
        let Ok(index) = self.index(key) else {
            return false;
        };
        let record = &mut self.records[index];
        if record.running() {
            return false;
        }
        record.last_run = Some(ThreadRun {
            started_at: at,
            ended_at: None,
            outcome: None,
        });
        true
    }

    /// The running run ended: stamps it and finishes the running turn. Answers
    /// the turn when its end checkpoint is still to capture.
    pub fn run_ended(&mut self, key: &str, at: u64, outcome: RunOutcome) -> Option<FinishedTurn> {
        let index = self.index(key).ok()?;
        let record = &mut self.records[index];
        let run = record
            .last_run
            .as_mut()
            .filter(|run| run.ended_at.is_none())?;
        let ended = at.max(run.started_at);
        run.ended_at = Some(ended);
        run.outcome = Some(outcome);
        self.finish_running_turn(index, outcome, ended)
    }

    /// Ends the running turn at `at`. Answers it for its end checkpoint and
    /// files when the thread works in git.
    fn finish_running_turn(
        &mut self,
        index: usize,
        outcome: RunOutcome,
        at: u64,
    ) -> Option<FinishedTurn> {
        let record = &mut self.records[index];
        let dir = record.workspace_dir().map(str::to_owned);
        let git = record.git_root.is_some();
        let key = record.key.clone();
        let turn = record
            .turns
            .iter_mut()
            .rev()
            .find(|turn| turn.state == TurnState::Running)?;
        turn.state = match outcome {
            RunOutcome::Completed => TurnState::Done,
            RunOutcome::Failed => TurnState::Failed,
            RunOutcome::Interrupted => TurnState::Interrupted,
        };
        turn.ended_at = Some(at);
        if !git {
            return None;
        }
        Some(FinishedTurn {
            end_ref: turn_ref(&key, turn.ordinal, true),
            ordinal: turn.ordinal,
            start_ref: turn.start_ref.clone(),
            key,
            dir: dir?,
        })
    }

    /// Stores what a finished turn's end recorded: its end checkpoint, its
    /// numstat counts, the files its edits named (kept only within
    /// [`MAX_TURN_PATHS`] and [`MAX_TURN_PATH_BYTES`], else unknown) and
    /// whether others shared its checkout.
    pub fn record_turn_end(&mut self, key: &str, ordinal: u32, end: TurnEnd) {
        let Ok(index) = self.index(key) else {
            return;
        };
        let Some(turn) = self.records[index]
            .turns
            .iter_mut()
            .find(|turn| turn.ordinal == ordinal)
        else {
            return;
        };
        if end.end_ref.is_some() {
            turn.end_ref = end.end_ref;
        }
        if let Some((files, added, removed)) = end.counts {
            turn.files_changed = Some(files);
            turn.added = Some(added);
            turn.removed = Some(removed);
        }
        turn.touched = end.touched.and_then(touched_within_bounds);
        turn.shared = end.shared;
    }

    /// Whether `key` asks for a generated title as its first prompt goes
    /// out (design "Titles"): generated titles are on, its title is still
    /// its launch prompt's seed, and its one turn, the first, is running.
    /// Any thread is named so, a chat with no project included: the title
    /// call reads only the excerpt, in a neutral folder.
    #[must_use]
    pub fn wants_title_at_first_prompt(&self, key: &str) -> bool {
        self.settings.generated_titles
            && self.record(key).is_some_and(|record| {
                named_by_its_seed(record)
                    && matches!(
                        record.turns.as_slice(),
                        [turn] if turn.ordinal == 0 && turn.state == TurnState::Running
                    )
            })
    }

    /// Whether `key` gets a generated title after a run: generated titles
    /// are on, its title is still its launch prompt's seed, and its last run
    /// completed the first of its turns to complete. The host asks then only
    /// when the attempt at the first prompt failed (or never ran: Claude
    /// came later, or the host restarted), with the first answer too.
    #[must_use]
    pub fn wants_generated_title(&self, key: &str) -> bool {
        self.settings.generated_titles
            && self.record(key).is_some_and(|record| {
                named_by_its_seed(record)
                    && record
                        .last_run
                        .as_ref()
                        .is_some_and(|run| run.outcome == Some(RunOutcome::Completed))
                    && record
                        .turns
                        .iter()
                        .filter(|turn| turn.state == TurnState::Done)
                        .count()
                        == 1
            })
    }

    /// Records how `key`'s title was made. A session without a thread record
    /// is left alone. Answers whether anything changed.
    pub fn set_title_source(&mut self, key: &str, source: TitleSource) -> bool {
        let Ok(index) = self.index(key) else {
            return false;
        };
        let record = &mut self.records[index];
        let changed = record.title_source != Some(source);
        record.title_source = Some(source);
        changed
    }

    /// Brings records in line with the registry: seeds a record for every
    /// listed, unarchived session without one, marks records whose session
    /// left the registry or the Threads surface missing (dropping them once
    /// missing for [`MISSING_TTL_MS`]), closes runs nothing is running any
    /// more (as interrupted), and answers archived threads whose owned
    /// worktree still has to be released. While the registry is not
    /// `complete` (its cache did not load, so it is being rebuilt from the
    /// providers' listings) nothing is marked missing or dropped.
    ///
    /// Seeding: a session started in Interface is active; one found in
    /// provider history is settled `external` unless it is busy. At the first
    /// reconcile only, sessions started in Interface whose last activity is
    /// older than `autoSettleDays` are settled `idle`, and nothing seeded goes
    /// to the top: it sorts by creation. Later seeds go to the top of Active.
    ///
    /// A running record is closed only when its session is not busy and the
    /// host does not account for its run (`run_accounted`): the host's run
    /// tap neither holds it open nor has an end for it on its way to the
    /// store. So a run cut off by a restart or a vanished session ends as
    /// interrupted, and a quickly finished run whose end the host saw but has
    /// not recorded yet is left for that end (never filed as stopped).
    pub fn reconcile(
        &mut self,
        sessions: &[ObservedSession],
        run_accounted: impl Fn(&str) -> bool,
        complete: bool,
        now: u64,
    ) -> Reconciled {
        let by_key = sessions
            .iter()
            .map(|session| (session.key.as_str(), session))
            .collect::<BTreeMap<_, _>>();
        let mut reconciled = Reconciled::default();
        let records = std::mem::take(&mut self.records);
        for mut record in records {
            let Some(session) = by_key
                .get(record.key.as_str())
                .filter(|session| session.listed)
            else {
                if complete {
                    let since = *record.missing_since.get_or_insert(now);
                    if now.saturating_sub(since) >= MISSING_TTL_MS {
                        reconciled.dropped.push(record);
                        continue;
                    }
                }
                self.records.push(record);
                continue;
            };
            record.missing_since = None;
            if record.running() && !session.busy && !run_accounted(&record.key) {
                let mut ended = None;
                if let Some(run) = record.last_run.as_mut() {
                    ended = Some(now.max(run.started_at));
                    run.ended_at = ended;
                    run.outcome = Some(RunOutcome::Interrupted);
                }
                for turn in &mut record.turns {
                    if turn.state == TurnState::Running {
                        turn.state = TurnState::Interrupted;
                        turn.ended_at = ended;
                    }
                }
            }
            if session.archived {
                if record.release.is_none()
                    && record.workspace.as_ref().is_some_and(|workspace| {
                        workspace.owned && workspace.mode == WorkspaceMode::Worktree
                    })
                {
                    reconciled.to_release.push(record.key.clone());
                }
            } else {
                record.release = None;
            }
            self.records.push(record);
        }
        self.seed(sessions, now);
        self.seeded = true;
        reconciled
    }

    fn seed(&mut self, sessions: &[ObservedSession], now: u64) {
        let known = self
            .records
            .iter()
            .map(|record| record.key.clone())
            .collect::<BTreeSet<_>>();
        let mut unseen = sessions
            .iter()
            .filter(|session| session.listed && !session.archived && !known.contains(&session.key))
            .collect::<Vec<_>>();
        if unseen.is_empty() {
            return;
        }
        // Oldest first, so the newest ends on top; at capacity the newest win.
        unseen.sort_by_key(|session| (session.first_seen.unwrap_or(now), session.key.clone()));
        let first = !self.seeded;
        let stale_before = (self.settings.auto_settle_days > 0)
            .then(|| now.saturating_sub(u64::from(self.settings.auto_settle_days) * DAY_MS));
        for session in unseen.into_iter().rev() {
            let settled = match session.origin {
                SeedOrigin::Store if !session.busy => Some(SettledReason::External),
                SeedOrigin::App
                    if first
                        && !session.busy
                        && stale_before
                            .is_some_and(|before| session.last_activity.unwrap_or(0) < before) =>
                {
                    Some(SettledReason::Idle)
                }
                _ => None,
            };
            if self.records.len() >= MAX_RECORDS
                && (settled.is_some() || !self.evict_oldest_missing())
            {
                continue;
            }
            let mut record = ThreadRecord::new(&session.key, session.first_seen.unwrap_or(now));
            if let Some(reason) = settled {
                record.settle(reason, now);
            }
            self.records.push(record);
        }
        if first {
            return;
        }
        // Later seeds go on top of Active, the newest topmost.
        let mut fresh = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                !known.contains(&record.key) && record.section(now) == ThreadSection::Active
            })
            .map(|(index, record)| (record.created_at, record.key.clone(), index))
            .collect::<Vec<_>>();
        fresh.sort();
        for (_, _, index) in fresh {
            self.place_first(index, OrderField::Active, now);
        }
    }

    /// Settles every active thread auto-settle may settle and answers their
    /// keys. `busy` answers whether a session is working or waiting on the
    /// owner, `None` when the registry does not know it. A thread is settled
    /// `idle` when the newest of its creation, un-settle and last run start or
    /// end is `autoSettleDays` old, and `merged` when its pull request merged
    /// and it has not run since that was seen.
    pub fn auto_settle(&mut self, now: u64, busy: impl Fn(&str) -> Option<bool>) -> Vec<String> {
        let days = self.settings.auto_settle_days;
        let on_merge = self.settings.settle_on_merge;
        let mut settled = Vec::new();
        for record in &mut self.records {
            if record.auto_settle_disabled
                || !record.listed()
                || record.section(now) != ThreadSection::Active
                || busy(&record.key) != Some(false)
            {
                continue;
            }
            let newest = [
                Some(record.created_at),
                record.unsettled_at,
                record.last_run.as_ref().map(|run| run.started_at),
                record.last_run.as_ref().and_then(|run| run.ended_at),
            ]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(0);
            let idle = days > 0 && newest.saturating_add(u64::from(days) * DAY_MS) <= now;
            let merged = on_merge
                && record
                    .pull_request
                    .as_ref()
                    .is_some_and(|pr| pr.state == PullRequestState::Merged)
                && record.merged_seen_at.is_some_and(|seen| {
                    record
                        .last_run
                        .as_ref()
                        .is_none_or(|run| run.started_at <= seen)
                });
            let reason = if merged {
                SettledReason::Merged
            } else if idle {
                SettledReason::Idle
            } else {
                continue;
            };
            record.settle(reason, now);
            settled.push(record.key.clone());
        }
        settled
    }

    /// The earliest snooze still ahead of `now`.
    #[must_use]
    pub fn next_snooze_wake(&self, now: u64) -> Option<u64> {
        self.records
            .iter()
            .filter_map(|record| record.snoozed_until)
            .filter(|until| *until > now)
            .min()
    }

    /// The pull request lookups due now: while tracking is on, unsettled
    /// worktree threads by their branch and any other unsettled thread that
    /// already has a pull request by its number; whatever the setting, a
    /// thread whose pull request Interface opened (`pull_request_tracked`),
    /// by its number — each at most every [`PULL_REQUEST_INTERVAL_MS`].
    /// `only` forces one thread's lookup.
    #[must_use]
    pub fn pull_requests_due(&self, now: u64, only: Option<&str>) -> Vec<PullRequestCheck> {
        let tracking = self.settings.track_pull_requests;
        self.records
            .iter()
            .filter(|record| match only {
                Some(key) => record.key == key,
                None => record
                    .pull_request_checked_at
                    .is_none_or(|checked| checked.saturating_add(PULL_REQUEST_INTERVAL_MS) <= now),
            })
            .filter(|record| record.settled_at.is_none())
            .filter_map(|record| {
                let workspace = record.workspace.as_ref()?;
                if workspace.mode == WorkspaceMode::None || workspace.project_root.is_empty() {
                    return None;
                }
                let branch = workspace
                    .branch
                    .clone()
                    .filter(|_| tracking && workspace.mode == WorkspaceMode::Worktree);
                let by = match (branch, &record.pull_request) {
                    (Some(branch), _) => PullRequestLookupBy::Branch(branch),
                    (None, Some(pull_request)) if tracking || record.pull_request_tracked => {
                        PullRequestLookupBy::Number(pull_request.number)
                    }
                    _ => return None,
                };
                Some(PullRequestCheck {
                    key: record.key.clone(),
                    project_root: record
                        .git_root
                        .clone()
                        .unwrap_or_else(|| workspace.project_root.clone()),
                    by,
                })
            })
            .collect()
    }

    /// Stores a lookup: the pull request it found (or none), and when a
    /// merge was first seen.
    pub fn record_pull_request(&mut self, key: &str, found: Option<FoundPullRequest>, now: u64) {
        let Ok(index) = self.index(key) else {
            return;
        };
        let record = &mut self.records[index];
        record.pull_request_checked_at = Some(now);
        if found
            .as_ref()
            .is_some_and(|found| found.state == PullRequestState::Merged)
        {
            record.merged_seen_at.get_or_insert(now);
        } else {
            record.merged_seen_at = None;
        }
        record.pull_request = found.map(|found| ThreadPullRequest {
            url: found.url,
            number: found.number,
            state: found.state,
            checked_at: now,
        });
    }

    /// What shipping `key`'s work covers: its folder, whether it owns it (a
    /// worktree) or shares the owner's checkout (Local, where only the files
    /// its own turns named are its), its pull request and its prompts.
    ///
    /// # Errors
    /// Refuses an unknown thread, a plain chat and a thread outside git.
    pub fn ship_scope(&self, key: &str) -> Result<ShipScope, ThreadRefusal> {
        let record = &self.records[self.index(key)?];
        let not_shippable = || {
            ThreadRefusal::new(
                THREADS_NOT_SHIPPABLE_CODE,
                "Only a project thread in a git repository can commit, push or open a pull request.",
            )
        };
        let workspace = record.workspace.as_ref().ok_or_else(not_shippable)?;
        let dir = record
            .workspace_dir()
            .filter(|_| record.git_root.is_some() && workspace.mode != WorkspaceMode::None)
            .ok_or_else(not_shippable)?
            .to_owned();
        let worktree = workspace.mode == WorkspaceMode::Worktree;
        // A switched thread is one thread: every segment's finished turns
        // count (store v5), oldest activation first.
        let members = self.chain_members(key);
        let finished = || {
            members
                .iter()
                .flat_map(|member| &member.turns)
                .filter(|turn| turn.state != TurnState::Running)
        };
        let attributed = finished()
            .filter_map(|turn| turn.touched.as_ref())
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        let touched = (!worktree).then(|| attributed.clone());
        let touched_unknown = !worktree && finished().any(|turn| turn.touched.is_none());
        let prompts = members
            .iter()
            .flat_map(|member| &member.turns)
            .filter(|turn| !chain::is_handoff_preview(&turn.preview))
            .collect::<Vec<_>>();
        let skip = prompts.len().saturating_sub(COMMIT_PROMPTS);
        Ok(ShipScope {
            dir,
            worktree,
            touched,
            attributed,
            touched_unknown,
            pull_request: record.pull_request.clone(),
            prompts: prompts
                .into_iter()
                .skip(skip)
                .map(|turn| turn.preview.clone())
                .collect(),
        })
    }

    /// Records the pull request Interface opened for `key` (or found open
    /// while opening one), and tracks it from then on whatever
    /// `trackPullRequests` says.
    pub fn record_opened_pull_request(&mut self, key: &str, found: FoundPullRequest, now: u64) {
        self.record_pull_request(key, Some(found), now);
        if let Ok(index) = self.index(key) {
            self.records[index].pull_request_tracked = true;
        }
    }

    /// Records the outcome of releasing an archived thread's worktree.
    pub fn record_release(&mut self, key: &str, outcome: ReleaseOutcome, now: u64) {
        if let Ok(index) = self.index(key) {
            self.records[index].release = Some(WorkspaceRelease { at: now, outcome });
        }
    }

    /// The owner's own release (`ReleaseWorkspace`) of `key`'s worktree
    /// succeeded: a worktree an earlier archive kept is no longer reported
    /// as kept.
    pub fn record_owner_release(&mut self, key: &str, outcome: ReleaseOutcome, now: u64) {
        if let Ok(index) = self.index(key)
            && self.records[index]
                .release
                .is_some_and(|release| release.outcome.kept().is_some())
        {
            self.records[index].release = Some(WorkspaceRelease { at: now, outcome });
        }
    }

    /// Who else uses `dir`: another live session working inside or around
    /// it, or another unarchived thread whose folder overlaps it — only an
    /// unsettled one unless `settled_too` (a release must not pull a folder
    /// from under a settled fork that may be picked up again).
    #[must_use]
    pub fn shared_with(
        &self,
        key: &str,
        dir: &str,
        uses: &[SessionUse],
        settled_too: bool,
    ) -> Option<WorktreeUse> {
        let archived = uses
            .iter()
            .map(|session| (session.key.as_str(), session.archived))
            .collect::<BTreeMap<_, _>>();
        uses.iter()
            .find(|session| session.key != key && session.live && overlaps(&session.cwd, dir))
            .map(|session| WorktreeUse::Live(session.key.clone()))
            .or_else(|| {
                self.records
                    .iter()
                    .find(|record| {
                        record.key != key
                            && (settled_too || record.settled_at.is_none())
                            && archived.get(record.key.as_str()) == Some(&false)
                            && record
                                .workspace_dir()
                                .is_some_and(|other| overlaps(other, dir))
                    })
                    .map(|record| WorktreeUse::Thread(record.key.clone()))
            })
    }

    /// What `key`'s finished turn is measured over. A turn's checkpoints
    /// snapshot a whole checkout — the repository a Local thread sits in, or
    /// its worktree — so where someone else works in it too, a whole count
    /// would hold their edits: then the turn counts only the files its own
    /// edits named (`touched`), and nothing when those are unknown or none.
    /// Someone else is what [`Self::shared_with`] finds (another live session
    /// inside or around the checkout, another unsettled thread whose folder
    /// overlaps it) or another unsettled Local thread in the same repository.
    /// An unreadable registry (`uses` is `None`), an unknown thread and one
    /// whose checkout cannot be named count as shared.
    #[must_use]
    pub fn turn_measure(
        &self,
        key: &str,
        uses: Option<&[SessionUse]>,
        touched: Option<&BTreeSet<String>>,
    ) -> TurnMeasure {
        if uses.is_some_and(|uses| !self.checkout_shared(key, uses)) {
            return TurnMeasure::Checkout;
        }
        match touched {
            Some(paths) if !paths.is_empty() => TurnMeasure::Files(paths.iter().cloned().collect()),
            _ => TurnMeasure::Unknown,
        }
    }

    fn checkout_shared(&self, key: &str, uses: &[SessionUse]) -> bool {
        let Ok(index) = self.index(key) else {
            return true;
        };
        let ours = checkout_folders(&self.records[index]);
        if ours.is_empty() {
            return true;
        }
        if ours
            .iter()
            .any(|dir| self.shared_with(key, dir, uses, false).is_some())
        {
            return true;
        }
        // `shared_with` compares folders; a Local thread's checkpoints cover
        // its whole repository, whichever folder of it the thread names.
        let unarchived = uses
            .iter()
            .filter(|session| !session.archived)
            .map(|session| session.key.as_str())
            .collect::<BTreeSet<_>>();
        self.records.iter().any(|record| {
            record.key != key
                && record.settled_at.is_none()
                && unarchived.contains(record.key.as_str())
                && checkout_folders(record)
                    .iter()
                    .any(|theirs| ours.iter().any(|dir| overlaps(theirs, dir)))
        })
    }

    /// The refusal for a worktree `other` uses, worded by what the owner can
    /// do about it. Settling frees the folder only from an idle thread a
    /// restore would otherwise wait for; a live session holds it until it is
    /// closed (archiving a thread closes it), whatever its thread's section;
    /// and a release also waits for settled threads, which only archiving
    /// lets go.
    fn shared(&self, other: &WorktreeUse, uses: &[SessionUse], release: bool) -> ThreadRefusal {
        let key = other.key();
        let title = uses
            .iter()
            .find(|session| session.key == key)
            .map_or(key, |session| session.title.as_str());
        let message = match other {
            WorktreeUse::Live(_) if self.record(key).is_some() => format!(
                "“{title}” is still open in this worktree. Archive it to close it, then try again."
            ),
            WorktreeUse::Live(_) => format!(
                "Another session, “{title}”, is still open in this worktree. Close that session, then try again."
            ),
            WorktreeUse::Thread(_) if release => {
                format!("“{title}” also uses this worktree. Archive it, then try again.")
            }
            WorktreeUse::Thread(_) => {
                format!("“{title}” also works in this worktree. Settle it, then try again.")
            }
        };
        ThreadRefusal::new(THREADS_SHARED_CODE, message)
    }

    /// Whether turn `ordinal` of `key` may be restored, and how. Allowed only
    /// in a worktree the thread owns, while its own agent is not busy, when no
    /// other live session or unsettled thread uses that folder, and when the
    /// turn has a start checkpoint.
    ///
    /// # Errors
    /// Refuses with the reason; nothing has been changed.
    pub fn restore_plan(
        &self,
        key: &str,
        ordinal: u32,
        busy: bool,
        worktree_present: bool,
        uses: &[SessionUse],
        now: u64,
    ) -> Result<RestorePlan, ThreadRefusal> {
        let record = &self.records[self.index(key)?];
        let dir = owned_worktree(record).ok_or_else(|| {
            ThreadRefusal::new(
                THREADS_NOT_OWNED_CODE,
                "Files can be restored only in a worktree this thread created. Fork into a new worktree to rewind safely.",
            )
        })?;
        if !worktree_present {
            return Err(ThreadRefusal::new(
                THREADS_WORKTREE_MISSING_CODE,
                "This thread's worktree is missing. Send a prompt to recreate it, then restore.",
            ));
        }
        let start_ref = record
            .turns
            .iter()
            .find(|turn| turn.ordinal == ordinal)
            .and_then(|turn| turn.start_ref.clone())
            .ok_or_else(|| {
                ThreadRefusal::new(
                    THREADS_NO_CHECKPOINT_CODE,
                    format!("Turn {} has no checkpoint to restore.", ordinal + 1),
                )
            })?;
        if busy {
            return Err(ThreadRefusal::new(
                THREADS_BUSY_CODE,
                "The agent is still working in this worktree. Stop it before restoring files.",
            ));
        }
        if let Some(other) = self.shared_with(key, dir, uses, false) {
            return Err(self.shared(&other, uses, false));
        }
        Ok(RestorePlan {
            dir: dir.to_owned(),
            start_ref,
            recovery_ref: recovery_ref(key, now),
        })
    }

    /// Whether `key`'s worktree may be released: owned, its own session not
    /// live, and no other live session or unarchived thread (settled or not)
    /// using it.
    ///
    /// # Errors
    /// Refuses with the reason; nothing has been changed.
    pub fn release_plan(
        &self,
        key: &str,
        own_live: bool,
        uses: &[SessionUse],
    ) -> Result<ReleasePlan, ThreadRefusal> {
        let record = &self.records[self.index(key)?];
        let (Some(dir), Some(git_root)) = (owned_worktree(record), record.git_root.as_deref())
        else {
            return Err(ThreadRefusal::new(
                THREADS_NOT_OWNED_CODE,
                "Only a worktree this thread created can be released.",
            ));
        };
        if own_live {
            return Err(ThreadRefusal::new(
                THREADS_BUSY_CODE,
                "This thread's agent is still running in the worktree. Stop it, then release the worktree.",
            ));
        }
        if let Some(other) = self.shared_with(key, dir, uses, true) {
            return Err(self.shared(&other, uses, true));
        }
        Ok(ReleasePlan {
            git_root: git_root.to_owned(),
            worktree_path: dir.to_owned(),
        })
    }

    /// Clears snoozes that ran out and were seen, and keeps turns within
    /// [`MAX_TURNS`]. Runs before every commit.
    fn normalize(&mut self, now: u64) {
        for record in &mut self.records {
            if record
                .snoozed_until
                .is_some_and(|until| until <= now && !record.woke(now))
            {
                record.snoozed_until = None;
                record.snoozed_at = None;
            }
            let excess = record.turns.len().saturating_sub(MAX_TURNS);
            if excess > 0 {
                record.turns.drain(..excess);
            }
        }
        self.repair_chains();
    }

    /// Frees about `over` bytes for a write refused at the byte cap. The
    /// turns of the threads idle longest (any section) are cut to their
    /// newest [`TRIM_FLOOR`] first, then records whose session is missing
    /// go (longest missing first), then the idlest threads' turns go
    /// entirely. A record whose session is listed is never dropped: it would
    /// seed again as a new, active thread. Answers whether anything changed.
    fn make_room(&mut self, mut over: usize) -> bool {
        let mut changed = self.trim_turns(TRIM_FLOOR, &mut over);
        while over > 0 {
            let Some(freed) = self.evict_oldest_missing_freeing() else {
                break;
            };
            over = over.saturating_sub(freed);
            changed = true;
        }
        self.trim_turns(0, &mut over) || changed
    }

    /// Cuts the idlest records' turns to their newest `floor` until about
    /// `over` bytes are freed.
    fn trim_turns(&mut self, floor: usize, over: &mut usize) -> bool {
        let mut idlest = (0..self.records.len()).collect::<Vec<_>>();
        idlest.sort_by_key(|&index| (self.records[index].last_activity(), index));
        let mut changed = false;
        for index in idlest {
            if *over == 0 {
                break;
            }
            let turns = &mut self.records[index].turns;
            let excess = turns.len().saturating_sub(floor);
            if excess == 0 {
                continue;
            }
            let dropped = turns.drain(..excess).collect::<Vec<_>>();
            *over = over.saturating_sub(encoded_len(&dropped));
            changed = true;
        }
        changed
    }
}

fn encoded_len(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}

fn owned_worktree(record: &ThreadRecord) -> Option<&str> {
    let workspace = record.workspace.as_ref()?;
    (workspace.mode == WorkspaceMode::Worktree && workspace.owned)
        .then_some(workspace.worktree_path.as_deref())
        .flatten()
}

fn prepared_full() -> ThreadRefusal {
    ThreadRefusal::new(
        THREADS_PREPARED_FULL_CODE,
        "Too many new threads are still starting. Wait for them to start, then try again.",
    )
}

/// The folders a record's turn checkpoints cover: its own folder and, for a
/// Local thread, the repository it sits in (a checkpoint snapshots all of it).
fn checkout_folders(record: &ThreadRecord) -> Vec<&str> {
    let local = record
        .workspace
        .as_ref()
        .is_some_and(|workspace| workspace.mode == WorkspaceMode::Local);
    record
        .workspace_dir()
        .into_iter()
        .chain(record.git_root.as_deref().filter(|_| local))
        .collect()
}

/// Whether one folder is, contains or is inside the other.
fn overlaps(left: &str, right: &str) -> bool {
    let (left, right) = (Path::new(left), Path::new(right));
    left.is_absolute()
        && right.is_absolute()
        && (left.starts_with(right) || right.starts_with(left))
}

// --- Validation -------------------------------------------------------------

fn text_ok(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn time_ok(value: Option<u64>) -> bool {
    value.is_none_or(|value| value <= MAX_TIME)
}

fn ref_ok(value: Option<&str>) -> bool {
    value.is_none_or(|value| {
        value.starts_with("refs/interface/threads/")
            && value.len() <= MAX_REF
            && value.bytes().all(|byte| byte.is_ascii_graphic())
    })
}

fn path_ok(value: &str) -> bool {
    text_ok(value, MAX_PATH) && Path::new(value).is_absolute()
}

fn workspace_ok(workspace: &ThreadWorkspace) -> bool {
    let root = match workspace.mode {
        WorkspaceMode::None => workspace.project_root.is_empty(),
        WorkspaceMode::Local | WorkspaceMode::Worktree => path_ok(&workspace.project_root),
    };
    root && workspace.missing.is_none()
        && workspace.worktree_path.as_deref().is_none_or(path_ok)
        && (workspace.mode == WorkspaceMode::Worktree) == workspace.worktree_path.is_some()
        && [&workspace.branch, &workspace.base_ref]
            .into_iter()
            .flatten()
            .all(|name| text_ok(name, MAX_REF))
}

fn turn_ok(turn: &TurnRecord) -> bool {
    turn.prompt_digest.len() == 16
        && turn
            .prompt_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && turn.sent_at <= MAX_TIME
        && time_ok(turn.ended_at)
        && (turn.state != TurnState::Running || turn.ended_at.is_none())
        && ref_ok(turn.start_ref.as_deref())
        && ref_ok(turn.end_ref.as_deref())
        && turn.preview.chars().count() <= PREVIEW_CHARS
        && !turn.preview.contains('\0')
        && turn.touched.as_deref().is_none_or(touched_ok)
}

/// A repository-relative path as git lists it: `/`-separated names, never
/// absolute, never `.` or `..`, no control characters.
fn repo_path_ok(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|name| !name.is_empty() && name != "." && name != "..")
}

fn touched_ok(paths: &[String]) -> bool {
    paths.len() <= MAX_TURN_PATHS
        && paths.iter().map(String::len).sum::<usize>() <= MAX_TURN_PATH_BYTES
        && paths.iter().all(|path| repo_path_ok(path))
        && paths.windows(2).all(|pair| pair[0] < pair[1])
}

/// `paths` sorted and deduplicated, or `None` (unknown) when any is not a
/// repository-relative path or they break a bound.
fn touched_within_bounds(mut paths: Vec<String>) -> Option<Vec<String>> {
    paths.sort_unstable();
    paths.dedup();
    touched_ok(&paths).then_some(paths)
}

fn record_ok(record: &ThreadRecord) -> bool {
    text_ok(&record.key, MAX_KEY)
        && [
            Some(record.created_at),
            record.pinned_at,
            record.settled_at,
            record.unsettled_at,
            record.snoozed_until,
            record.snoozed_at,
            record.last_visited_at,
            record.pull_request_checked_at,
            record.merged_seen_at,
            record.missing_since,
        ]
        .into_iter()
        .all(time_ok)
        && [&record.pin_order, &record.active_order]
            .into_iter()
            .flatten()
            .all(|key| order_key_valid(key))
        && record.settled_at.is_some() == record.settled_reason.is_some()
        && record.last_run.as_ref().is_none_or(|run| {
            run.started_at <= MAX_TIME
                && time_ok(run.ended_at)
                && run.ended_at.is_some() == run.outcome.is_some()
        })
        && record.workspace.as_ref().is_none_or(workspace_ok)
        && record.git_root.as_deref().is_none_or(path_ok)
        && record
            .lineage
            .as_ref()
            .is_none_or(|lineage| text_ok(&lineage.parent, MAX_KEY) && lineage.at <= MAX_TIME)
        && record
            .pull_request
            .as_ref()
            .is_none_or(|pr| text_ok(&pr.url, MAX_URL) && pr.checked_at <= MAX_TIME)
        && record
            .chain
            .as_ref()
            .is_none_or(|chain| text_ok(&chain.id, MAX_KEY))
        && record.segment_of.as_deref().is_none_or(|head| {
            record.chain.is_some() && head != record.key && text_ok(head, MAX_KEY)
        })
        && record.turns.len() <= MAX_TURNS
        && record.turns.iter().all(turn_ok)
        && record
            .turns
            .windows(2)
            .all(|pair| pair[0].ordinal < pair[1].ordinal)
}

fn prepared_ok(prepared: &PreparedRecord) -> bool {
    text_ok(&prepared.token, MAX_TOKEN)
        && path_ok(&prepared.cwd)
        && workspace_ok(&prepared.workspace)
        && prepared.workspace.mode != WorkspaceMode::None
        && prepared.git_root.as_deref().is_none_or(path_ok)
        && ref_ok(prepared.baseline_ref.as_deref())
        && prepared.created_at <= MAX_TIME
        && prepared.lineage.as_ref().is_none_or(|lineage| {
            matches!(
                lineage.relation,
                LineageRelation::Handoff | LineageRelation::Child
            ) && text_ok(&lineage.parent, MAX_KEY)
                && lineage.at <= MAX_TIME
        })
}

/// Whether `document` is a version 5 document within every bound.
#[must_use]
pub fn document_valid(document: &ThreadsDocument) -> bool {
    document.version == THREADS_DOCUMENT_VERSION && contents_valid(document)
}

/// Whether `document` holds none of version 5's fields: no record is part
/// of a chain.
fn without_v5_fields(document: &ThreadsDocument) -> bool {
    document
        .records
        .iter()
        .all(|record| record.chain.is_none() && record.segment_of.is_none())
}

/// Whether `document` is a version 4 document within every bound: none of
/// version 5's fields, which only a version 5 store records.
#[must_use]
pub fn v4_document_valid(document: &ThreadsDocument) -> bool {
    document.version == THREADS_V4_VERSION
        && without_v5_fields(document)
        && contents_valid(document)
}

/// Whether `document` holds none of version 4's fields: no `child` lineage
/// on a thread or a prepared workspace, and team tools at their default.
fn without_v4_fields(document: &ThreadsDocument) -> bool {
    let not_child = |lineage: &Option<ThreadLineage>| {
        lineage
            .as_ref()
            .is_none_or(|lineage| lineage.relation != LineageRelation::Child)
    };
    document.settings.team_tools
        && document
            .records
            .iter()
            .all(|record| not_child(&record.lineage))
        && document
            .prepared
            .iter()
            .all(|prepared| not_child(&prepared.lineage))
}

/// Whether `document` is a version 3 document within every bound: none of
/// version 4's fields, which only a version 4 store records.
#[must_use]
pub fn v3_document_valid(document: &ThreadsDocument) -> bool {
    document.version == THREADS_V3_VERSION
        && without_v4_fields(document)
        && without_v5_fields(document)
        && contents_valid(document)
}

/// Whether `document` holds none of version 3's fields: no `handoff`
/// lineage on a thread or a prepared workspace.
fn without_v3_fields(document: &ThreadsDocument) -> bool {
    document.records.iter().all(|record| {
        record
            .lineage
            .as_ref()
            .is_none_or(|lineage| lineage.relation != LineageRelation::Handoff)
    }) && document
        .prepared
        .iter()
        .all(|prepared| prepared.lineage.is_none())
}

/// Whether `document` is a version 2 document within every bound: none of
/// version 3's fields, which only a version 3 store records.
#[must_use]
pub fn v2_document_valid(document: &ThreadsDocument) -> bool {
    document.version == THREADS_V2_VERSION
        && without_v3_fields(document)
        && without_v4_fields(document)
        && without_v5_fields(document)
        && contents_valid(document)
}

/// Whether `document` is a version 1 document within every bound: none of
/// version 2's turn fields or version 3's lineage, which only those stores
/// record.
#[must_use]
pub fn legacy_document_valid(document: &ThreadsDocument) -> bool {
    document.version == THREADS_LEGACY_VERSION
        && without_v3_fields(document)
        && without_v4_fields(document)
        && without_v5_fields(document)
        && document
            .records
            .iter()
            .flat_map(|record| &record.turns)
            .all(|turn| turn.ended_at.is_none() && turn.touched.is_none() && !turn.shared)
        && document
            .records
            .iter()
            .all(|record| !record.pull_request_tracked)
        && contents_valid(document)
}

fn contents_valid(document: &ThreadsDocument) -> bool {
    let mut keys = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    document.revision < 9_007_199_254_740_991
        && document.settings.auto_settle_days <= MAX_AUTO_SETTLE_DAYS
        && document.records.len() <= MAX_RECORDS
        && document.prepared.len() <= MAX_PREPARED
        && document
            .records
            .iter()
            .all(|record| keys.insert(record.key.as_str()) && record_ok(record))
        && document
            .prepared
            .iter()
            .all(|prepared| tokens.insert(prepared.token.as_str()) && prepared_ok(prepared))
        && document.chains_valid()
}

/// Whether `record`'s title is still its launch prompt's seed, which an
/// automatic title may replace. A switched thread's title was made before
/// its handoff; the handoff prompt never names it.
fn named_by_its_seed(record: &ThreadRecord) -> bool {
    record.chain.is_none() && record.title_source == Some(TitleSource::Prompt)
}

// --- The store ----------------------------------------------------------------

/// The leased single writer of `threads.v5.json`.
pub struct ThreadStore {
    workflow_file: PathBuf,
    workflows: Option<crate::workflows::WorkflowStore>,
    document: LeasedDocument,
    state: ThreadsDocument,
    /// Where an unreadable file found at open was set aside.
    quarantined: Option<PathBuf>,
}

impl ThreadStore {
    /// Opens (or creates) the store at `file`. A document that does not
    /// decode, has an unknown version or breaks a bound is quarantined and the
    /// store starts empty. An empty store's revision starts at `now` (ms).
    ///
    /// When `file` does not exist, the newest older store beside it — the
    /// version 4 store ([`THREADS_V4_FILE_NAME`]), else the version 3 store
    /// ([`THREADS_V3_FILE_NAME`]), else the version 2 store
    /// ([`THREADS_V2_FILE_NAME`]), else the version 1 store
    /// ([`THREADS_LEGACY_FILE_NAME`]) — is read once, never changed, moved or
    /// quarantined, and carried over as version 5 with the newer fields
    /// absent (unknown, or their default). An older file that cannot be read
    /// or is invalid is left for the build that wrote it; the store starts
    /// empty.
    ///
    /// # Errors
    /// Refuses unsafe storage, a lease held by another writer, or a file over
    /// the byte bound.
    pub fn open(file: &Path, now: u64) -> Result<Self, SettingsError> {
        let limits = JsonLimits::new(BYTES, 24, 1_000_000)?;
        let (document, state, quarantined) =
            LeasedDocument::open_or_quarantine(file, LOCK_NAME, limits, UNCERTAIN, document_valid)?;
        let legacy = (state.is_none() && quarantined.is_none())
            .then(|| read_older(file, limits))
            .flatten();
        let migrated = legacy.is_some();
        let mut store = Self {
            workflow_file: file.with_file_name(crate::workflows::WORKFLOWS_FILE),
            workflows: None,
            document,
            state: state.or(legacy).unwrap_or_else(|| ThreadsDocument {
                revision: now.min(MAX_TIME),
                ..ThreadsDocument::default()
            }),
            quarantined,
        };
        if migrated {
            // Written at once, so the legacy file is never read again.
            let next = store.state.clone();
            store.commit(next).map_err(|error| match error {
                ThreadStoreError::Storage(error) => error,
                ThreadStoreError::Refused(refusal) => {
                    SettingsError::Storage(StorageError::Corrupt {
                        path: file.to_path_buf(),
                        reason: refusal.message,
                    })
                }
            })?;
        }
        Ok(store)
    }

    /// Turns generated titles on once for a store written while they were
    /// off by default (design "Titles"): a store from before the default
    /// changed holds that old default, `false`, which its owner never
    /// chose. [`GENERATED_TITLES_ON_MARKER`], beside the store and written
    /// under its lease before the change, records that this ran, so it
    /// never runs twice and an owner's later off stays off. The document's
    /// schema is untouched, so older builds still read it. Answers whether
    /// the setting changed.
    ///
    /// # Errors
    /// Unsafe storage, or the marker or the change could not be written:
    /// then the setting is left as it was (a written marker with a failed
    /// change leaves titles off rather than risk turning them on twice).
    pub fn turn_generated_titles_on_once(&mut self, now: u64) -> Result<bool, ThreadStoreError> {
        self.document.ensure_writable()?;
        if self.document.has_marker(GENERATED_TITLES_ON_MARKER)? {
            return Ok(false);
        }
        self.document.write_marker(
            GENERATED_TITLES_ON_MARKER,
            b"Generated titles were turned on once for this store.\n",
        )?;
        if self.state.settings.generated_titles {
            return Ok(false);
        }
        self.apply(now, |document| {
            document.settings.generated_titles = true;
            Ok(())
        })
        .map(|((), changed)| changed)
    }

    /// Whether the owner turned "Pane tools without asking" on
    /// ([`PANE_TOOLS_MARKER`]). Unreadable means off.
    #[must_use]
    pub fn pane_tools_without_asking(&self) -> bool {
        self.document
            .read_marker(PANE_TOOLS_MARKER)
            .is_ok_and(|bytes| bytes.as_deref() == Some(PANE_TOOLS_ON))
    }

    /// Records the owner's "Pane tools without asking" switch in
    /// [`PANE_TOOLS_MARKER`]. The document is untouched.
    ///
    /// # Errors
    /// Unsafe storage, or the marker could not be written.
    pub fn set_pane_tools_without_asking(&mut self, on: bool) -> Result<(), ThreadStoreError> {
        self.document.ensure_writable()?;
        self.document.replace_marker(
            PANE_TOOLS_MARKER,
            if on { PANE_TOOLS_ON } else { PANE_TOOLS_OFF },
        )?;
        Ok(())
    }

    /// The independent workflow store, opened lazily under the same host owner.
    /// # Errors
    /// Unsafe or unreadable workflow state refuses execution.
    pub fn workflows(&mut self) -> Result<&mut crate::workflows::WorkflowStore, SettingsError> {
        if self.workflows.is_none() {
            self.workflows = Some(crate::workflows::WorkflowStore::open(&self.workflow_file)?);
        }
        self.workflows
            .as_mut()
            .ok_or_else(|| SettingsError::InvalidAgent("The workflow store did not open.".into()))
    }

    /// The file was unreadable and was quarantined at open.
    #[must_use]
    pub const fn quarantined(&self) -> bool {
        self.quarantined.is_some()
    }

    /// Where that unreadable file now lives.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    #[must_use]
    pub const fn document(&self) -> &ThreadsDocument {
        &self.state
    }

    /// Applies `edit` to a copy of the document and commits it with the next
    /// revision when anything changed. Answers the edit's value and whether a
    /// commit happened.
    ///
    /// # Errors
    /// The edit's refusal (nothing changes), or the storage failure.
    pub fn apply<T>(
        &mut self,
        now: u64,
        edit: impl FnOnce(&mut ThreadsDocument) -> Result<T, ThreadRefusal>,
    ) -> Result<(T, bool), ThreadStoreError> {
        self.document.ensure_writable()?;
        let mut next = self.state.clone();
        let value = edit(&mut next)?;
        next.normalize(now);
        if next == self.state {
            return Ok((value, false));
        }
        self.commit(next)?;
        Ok((value, true))
    }

    /// Commits the next revision even though nothing else changed, so a
    /// derived change (a snooze running out) reaches every view as new.
    ///
    /// # Errors
    /// The storage failure.
    pub fn bump(&mut self, now: u64) -> Result<(), ThreadStoreError> {
        self.document.ensure_writable()?;
        let mut next = self.state.clone();
        next.normalize(now);
        self.commit(next)
    }

    fn commit(&mut self, mut next: ThreadsDocument) -> Result<(), ThreadStoreError> {
        next.revision = self.state.revision + 1;
        if !document_valid(&next) {
            return Err(ThreadRefusal::invalid(
                "That change would break the thread store's bounds.",
            )
            .into());
        }
        match self.document.commit(&next) {
            // Nothing was written; making room allows one retry.
            Err(SettingsError::Storage(StorageError::BoundsExceeded { actual_bytes, .. }))
                if next.make_room(
                    usize::try_from(actual_bytes)
                        .unwrap_or(usize::MAX)
                        .saturating_sub(SOFT_BYTES),
                ) =>
            {
                self.document.commit(&next)?;
            }
            result => result?,
        }
        self.state = next;
        Ok(())
    }
}

/// The newest older store beside `file` that exists, carried over to the
/// current version: the version 4 file when it exists (even when it cannot
/// be used: an older file is never newer news), else the version 3 file,
/// else the version 2 file, else the version 1 file. `None` when none can be
/// used.
fn read_older(file: &Path, limits: JsonLimits) -> Option<ThreadsDocument> {
    let v4 = file.with_file_name(THREADS_V4_FILE_NAME);
    if v4.symlink_metadata().is_ok() {
        return read_legacy(&v4, limits, v4_document_valid);
    }
    let v3 = file.with_file_name(THREADS_V3_FILE_NAME);
    if v3.symlink_metadata().is_ok() {
        return read_legacy(&v3, limits, v3_document_valid);
    }
    let v2 = file.with_file_name(THREADS_V2_FILE_NAME);
    if v2.symlink_metadata().is_ok() {
        return read_legacy(&v2, limits, v2_document_valid);
    }
    read_legacy(
        &file.with_file_name(THREADS_LEGACY_FILE_NAME),
        limits,
        legacy_document_valid,
    )
}

/// The older store at `file`, carried over to the current version; `None`
/// when it is absent, unreadable or not `valid`. Read-only: the older file
/// and its directory are never hardened, rewritten, moved or quarantined.
/// Whether the owner turned "Pane tools without asking" on, read from
/// [`PANE_TOOLS_MARKER`] in `data_root` without the store's lease: the host
/// reads it once at start, before the store opens, for the panes it launches.
/// Missing, unsafe or unreadable means off.
#[must_use]
pub fn pane_tools_without_asking_at(data_root: &Path) -> bool {
    PrivateDirectory::open(data_root)
        .ok()
        .and_then(|directory| {
            interface_storage::read_bounded(&directory, PANE_TOOLS_MARKER, 64)
                .ok()
                .flatten()
        })
        .is_some_and(|file| file.bytes() == PANE_TOOLS_ON)
}

fn read_legacy(
    file: &Path,
    limits: JsonLimits,
    valid: fn(&ThreadsDocument) -> bool,
) -> Option<ThreadsDocument> {
    let parent = file.parent()?;
    let name = file.file_name()?.to_str()?;
    let directory = PrivateDirectory::open(parent).ok()?;
    let mut document = read_json::<ThreadsDocument>(&directory, name, limits)
        .ok()??
        .into_parts()
        .0;
    if !valid(&document) {
        return None;
    }
    document.version = THREADS_DOCUMENT_VERSION;
    Some(document)
}

#[cfg(test)]
#[path = "threads_tests.rs"]
mod tests;
