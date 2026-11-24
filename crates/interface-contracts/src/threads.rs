//! Threads surface v1 (protocol 34): the `iface:threadControl` command, its
//! response, and the `ThreadsView` that `iface:threads` pushes. Protocol 36
//! adds each turn's `endedAt`; protocol 37 adds shipping a thread's work
//! (status, commit, a branch, push and pull request, checks, review
//! comments, merge); protocol 42 adds the bridges between Threads and Code
//! (the From terminals section, Open in terminal, Continue as thread);
//! protocol 46 adds a second opinion from another provider and Continue with
//! another provider (review B4), whose thread records a `handoff` lineage;
//! protocol 57 adds the `child` lineage of a thread another thread's agent
//! started through team tools, and the `teamTools` setting; protocol 58 adds
//! the view's `hiddenKeys`, the sessions Threads and Chat hide as workers.
//!
//! A thread's identity is its session key (`engine#sessionId`); thread state
//! is a Rust-owned overlay on the session registry. Rust derives every section,
//! unread flag, order key and restore permission; the renderer only draws the
//! view and sends commands. Times are milliseconds since the Unix epoch.
//! Design: `docs/superpowers/specs/2026-10-06-threads-design.md`.
use serde::{Deserialize, Serialize};

/// The `ThreadsView::version` this contract describes.
pub const THREADS_VIEW_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadSection {
    Pinned,
    Active,
    Snoozed,
    Settled,
    /// Protocol 42: a session started in a Code pane or the CLI that has
    /// never been brought into Threads (settled with reason `external`).
    FromTerminals,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SettledReason {
    Manual,
    Idle,
    Merged,
    Edited,
    External,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceMode {
    Local,
    Worktree,
    /// Plain chat ("No project"): no folder of the owner's.
    None,
}

/// Why an archive, delete or forget kept a thread's own worktree.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceKept {
    /// It had uncommitted changes.
    Dirty,
    /// It held ignored files (`.env`, dependencies, build output).
    Ignored,
    /// Another live session or unarchived thread still used it.
    InUse,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RunOutcome {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnState {
    Running,
    Done,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LineageRelation {
    Fork,
    Edit,
    /// Protocol 46: a new thread on another provider that continues this
    /// one's work (Continue with). It keeps none of the parent's turns.
    Handoff,
    /// Protocol 57: a thread the parent thread's agent started through team
    /// tools (`iface_team`). It keeps none of the parent's turns.
    Child,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TitleSource {
    Prompt,
    Generated,
    User,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PullRequestState {
    Open,
    Merged,
    Closed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffScope {
    Turn,
    Thread,
    Uncommitted,
    Branch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffOmitted {
    Sensitive,
    Large,
    Binary,
}

/// What the GitHub CLI can do for a thread (protocol 37).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GhState {
    Ready,
    /// `gh` is not installed.
    Missing,
    /// `gh` is installed but not signed in (`gh auth status` failed).
    SignedOut,
    /// `gh` did not answer.
    Unavailable,
}

/// `gh pr checks`' own grouping of a check's state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckBucket {
    Pass,
    Fail,
    Pending,
    Skipping,
    Cancel,
}

/// A pull request's checks, summed up: failing if any failed, else pending
/// if any still run, else passing; none when it has no checks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ChecksState {
    Passing,
    Failing,
    Pending,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MergeMethod {
    Squash,
    Merge,
    Rebase,
}

/// Which side of a diff a quoted line is on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffSide {
    New,
    Old,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShipAction {
    Commit,
    CreateBranch,
    OpenPullRequest,
    Merge,
}

/// How sure a ship effect's answer is. A refused effect is an error, not a
/// certainty: only what happened, or what could not be told, answers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShipCertainty {
    /// It happened (seen directly, or found on a re-read).
    Done,
    /// It may or may not have happened, and re-reading failed too. Check
    /// again before trying again; nothing retries it.
    Unknown,
}

/// Why Commit leaves a changed file out.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LeftOutReason {
    /// Credential-shaped (`.env`, keys): Interface never commits it.
    Credential,
    /// A Local thread commits only the files its own turns' edits named.
    NotThisThread,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadWorkspace {
    pub mode: WorkspaceMode,
    /// `""` for mode `none`.
    pub project_root: String,
    /// Mode `worktree` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    /// This thread created the worktree.
    pub owned: bool,
    /// Derived at read: the worktree directory is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing: Option<bool>,
    /// Derived at read: archiving, deleting or forgetting this thread's
    /// session kept the worktree it owns, and why. Cleared when a later
    /// release succeeds or the session is unarchived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept: Option<WorkspaceKept>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadLineage {
    pub parent: String,
    pub relation: LineageRelation,
    pub kept_turns: u32,
    pub at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadRun {
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RunOutcome>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadTurn {
    pub ordinal: u32,
    pub sent_at: u64,
    /// First 16 hex of sha256(normalized prompt).
    pub prompt_digest: String,
    pub has_start: bool,
    pub has_end: bool,
    pub state: TurnState,
    /// When the turn's run ended (protocol 36). Absent while it runs, for a
    /// turn recorded before protocol 36, and for one closed without an end
    /// the run tap saw (a newer send began first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_changed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadPullRequest {
    pub url: String,
    pub number: u64,
    pub state: PullRequestState,
    pub checked_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadView {
    pub key: String,
    pub section: ThreadSection,
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
    pub snoozed_until: Option<u64>,
    pub woke: bool,
    pub unread: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_visited_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ThreadRun>,
    pub auto_settle: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<ThreadWorkspace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<ThreadLineage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<ThreadPullRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_source: Option<TitleSource>,
    pub turns: Vec<ThreadTurn>,
    /// The Away loop this thread runs as (protocol 45): Rust joins it from
    /// the Away store at read; absent for every other thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away: Option<crate::away::ThreadAway>,
    /// Protocol 60: the added provider account the thread's session is bound
    /// to; Rust joins it from `session-accounts.v1.json` at read. Absent for
    /// the Default account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Protocol 59: the provider sessions this thread ran on, when its
    /// provider was switched (`switchProvider`); absent for every other
    /// thread. Only the head (the session the thread runs on now) is listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ThreadChain>,
    /// Protocol 62 (T3 section 7): the thread's scheduled prompts and its
    /// pull-request watch, joined from `thread-schedules.v2.json` at read;
    /// absent when it has neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedules: Option<ThreadSchedules>,
    /// Protocol 65: an agent outside Interface started this thread through
    /// `iface mcp`; the name its MCP client declared (sanitized, display
    /// only). Rust joins it from the team tools ledger at read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_from_outside: Option<String>,
    /// Protocol 66 (T3 section 10): the thread's provider reported its
    /// usage limit since the thread last completed a turn; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<ThreadLimit>,
    /// Protocol 67 (T3 section 12): the secret this thread's agent asks
    /// for through `request_secret`, while it waits (at most one); absent
    /// otherwise. Never a value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_request: Option<crate::thread_secrets::ThreadSecretRequest>,
    /// Protocol 67 (T3 section 12): the side question open on this thread
    /// (at most one); absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<ThreadSide>,
    /// Protocol 72: the thread's session runs on Claude (any account) in a
    /// folder nobody has trusted, so it runs without that folder's own
    /// settings and instruction file. Rust joins it at read; absent for
    /// every other thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_trust: Option<ThreadFolderTrust>,
}

/// Protocol 72: a Claude thread's folder is not trusted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadFolderTrust {
    /// The folder a trust answer covers (the repository's top folder, or
    /// the folder itself outside one), to show.
    pub folder: String,
    /// `false`: nobody has answered for this folder yet; `true`: the owner
    /// said Not now.
    pub asked: bool,
}

/// Protocol 72: where a folder stands, as `folderTrust` and `trustFolder`
/// answer it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FolderTrustState {
    Trusted,
    /// Nobody has answered for this folder yet.
    Unasked,
    /// The owner said Not now.
    Declined,
}

/// Protocol 72: the answer to `folderTrust` and `trustFolder`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderTrustAnswer {
    /// The folder the answer covers (the trust root).
    pub folder: String,
    pub state: FolderTrustState,
}

/// The longest folder path `folderTrust` and `trustFolder` accept, in bytes.
pub const FOLDER_TRUST_MAX_PATH_BYTES: usize = 4096;

/// Whether `folder` is a path `folderTrust` and `trustFolder` accept: absolute,
/// at most [`FOLDER_TRUST_MAX_PATH_BYTES`] bytes, without control characters.
#[must_use]
pub fn valid_folder_trust_path(folder: &str) -> bool {
    folder.starts_with('/')
        && folder.len() <= FOLDER_TRUST_MAX_PATH_BYTES
        && !folder.chars().any(char::is_control)
}

/// Protocol 66: a thread's provider hit its usage limit.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadLimit {
    /// When the limit resets, in the usage windows' ISO form: the
    /// provider's notice, else the latest reset among its usage windows at
    /// or over their limit. Absent when neither said (never guessed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

/// A side question (protocol 67): a fork of the thread at its latest turn,
/// asked one question without adding to the thread, hidden from the inbox.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSide {
    /// The side thread's own session key; its events carry the answer.
    pub key: String,
    pub question: String,
    pub asked_at: u64,
}

/// A thread whose provider was switched: its provider sessions, oldest
/// activation first, the head (this thread's own key) last.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadChain {
    pub segments: Vec<ThreadChainSegment>,
}

/// One provider session of a [`ThreadChain`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadChainSegment {
    pub key: String,
    pub engine: crate::engine::EngineId,
    /// Counts activations: 0 for the first segment, one more each time a
    /// segment became the head.
    pub order: u32,
    /// When the segment was created or first sent to.
    pub first_at: u64,
    /// The last time it ran (its last turn or run), else `first_at`.
    pub last_at: u64,
    /// Protocol 69: the lines the segment's own turns changed, summed over
    /// the turns that measured both counts (as a thread's totals are);
    /// both absent when none did. The head's are its own turns' totals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<u32>,
}

/// Whether a `switchProvider`'s handoff prompt reached the new head.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HandoffDelivery {
    /// The provider accepted it.
    Sent,
    /// It definitely did not go out; the switch stands and the owner types.
    Unsent,
    /// It may or may not have arrived; nothing retries it.
    Unknown,
}

/// Protocol 62: when a scheduled prompt runs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ScheduleWhen {
    /// Once, at `at` (ms since the epoch); then the schedule turns off.
    Once { at: u64 },
    /// Every `minutes` (at least 5) from its last due time.
    Interval { minutes: u32 },
    /// On local dates (the calendar teammate routines use).
    Calendar { calendar: ScheduleCalendar },
}

/// A calendar a scheduled prompt follows: `hour:minute` in `timezone` on
/// `weekdays` (ISO, Monday = 1; empty means every day).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleCalendar {
    pub timezone: String,
    pub weekdays: Vec<u8>,
    pub hour: u8,
    pub minute: u8,
}

/// What became of one firing of a schedule or watch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleOutcome {
    /// The provider took the prompt.
    Sent,
    /// It was not sent: missed by more than a day, paused while the thread
    /// is settled, or another run of it was still waiting.
    Skipped,
    /// It may or may not have arrived (also what an attempt is recorded as
    /// before it goes out); never sent again.
    Unknown,
    /// The thread could not take it (`detail` says why); nothing was sent.
    Refused,
}

/// The last firing of a schedule or watch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleRun {
    /// The firing's due time (ms), which names it: `scheduled:{id}:{occurrence}`.
    pub occurrence: u64,
    pub at: u64,
    pub outcome: ScheduleOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Who set a schedule up.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleSetBy {
    #[default]
    Owner,
    /// The thread's agent, through the `schedule_prompt` team tool.
    Agent,
}

/// What a scheduled prompt is for (protocol 66).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleKind {
    /// A prompt the owner or the agent scheduled.
    #[default]
    Prompt,
    /// Resume at reset (T3 section 10): a one-time prompt that carries the
    /// thread on a minute after its provider's usage limit resets. One per
    /// thread.
    Resume,
}

impl ScheduleKind {
    /// Whether this is the default kind (absent on the wire and on disk).
    #[must_use]
    #[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
    pub fn is_prompt(&self) -> bool {
        *self == Self::Prompt
    }
}

/// One scheduled prompt, as the thread pane and Settings show it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadScheduleView {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub prompt: String,
    pub when: ScheduleWhen,
    pub enabled: bool,
    pub created_at: u64,
    /// The next due time; absent once a one-time schedule has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ScheduleRun>,
    /// A firing waiting for the thread's turn to end (its occurrence).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting: Option<u64>,
    pub set_by: ScheduleSetBy,
    /// The thread is settled: nothing is sent until it is active again.
    pub paused: bool,
    /// Protocol 66: `resume` for Resume at reset; absent for a prompt.
    #[serde(default, skip_serializing_if = "ScheduleKind::is_prompt")]
    pub kind: ScheduleKind,
}

/// The thread's pull-request watch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadWatchView {
    pub enabled: bool,
    /// `gh` is missing or signed out: the watch sends nothing.
    pub needs_gh: bool,
    /// The failing checks and unresolved review threads it has seen.
    pub failing_checks: u32,
    pub review_threads: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ScheduleRun>,
    /// A prompt waiting for the thread's turn to end.
    pub waiting: bool,
}

/// A thread's schedules (soonest first) and its watch.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSchedules {
    pub schedules: Vec<ThreadScheduleView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<ThreadWatchView>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Independent owner switches, one per setting.
pub struct ThreadSettings {
    /// `0` = never.
    pub auto_settle_days: u32,
    pub settle_on_merge: bool,
    /// Claude Haiku 5.5 names a thread from its first prompt. On by default
    /// (it spends a little of the owner's Claude plan); the host turns an
    /// existing store's old `false` default on once (`threads.rs`,
    /// `GENERATED_TITLES_ON_MARKER`), and an owner's later off stays off.
    pub generated_titles: bool,
    /// Needs `gh`; silent when it is absent.
    pub track_pull_requests: bool,
    /// Protocol 57: Threads sessions get the `iface_team` tools (start, read,
    /// wait for and message other threads). On by default; absent means on,
    /// and only `false` is written, so older views and files read unchanged.
    #[serde(
        default = "team_tools_default",
        skip_serializing_if = "is_team_tools_default"
    )]
    pub team_tools: bool,
    /// Protocol 65 (outside agents, `iface mcp`): what a Claude Code or Codex
    /// running outside Interface may do over the owner-only outside socket.
    /// Off by default; absent means off, and only another level is written.
    /// Rust keeps it in the team tools ledger, never in the Threads store,
    /// and joins it onto every view.
    #[serde(default, skip_serializing_if = "OutsideAgents::is_off")]
    pub outside_agents: OutsideAgents,
    /// Protocol 71: a Claude or Codex pane may type into another pane or
    /// start one (`send_to_pane`, `pass_off`) without asking the owner first.
    /// Off by default; absent means off, and only `true` is written. Rust
    /// keeps it beside the Threads store (`threads.pane-tools-without-asking`),
    /// never in it, and joins it onto every view; panes read it when
    /// Interface starts.
    #[serde(default, skip_serializing_if = "is_off")]
    pub pane_tools_without_asking: bool,
}

/// What agents outside Interface may do (protocol 65): nothing, read
/// threads, or also start threads in Interface's projects and message the
/// threads they started.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OutsideAgents {
    #[default]
    Off,
    Read,
    Full,
}

impl OutsideAgents {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Read => "read",
            Self::Full => "full",
        }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
    #[must_use]
    pub const fn is_off(&self) -> bool {
        matches!(self, Self::Off)
    }
}

const fn team_tools_default() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
const fn is_team_tools_default(value: &bool) -> bool {
    *value
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
const fn is_off(value: &bool) -> bool {
    !*value
}

impl Default for ThreadSettings {
    fn default() -> Self {
        Self {
            auto_settle_days: 3,
            settle_on_merge: true,
            generated_titles: true,
            track_pull_requests: true,
            team_tools: true,
            outside_agents: OutsideAgents::Off,
            pane_tools_without_asking: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadsView {
    /// Always [`THREADS_VIEW_VERSION`].
    pub version: u8,
    pub revision: u64,
    pub now: u64,
    pub settings: ThreadSettings,
    pub threads: Vec<ThreadView>,
    /// Protocol 58: the registry's sessions Threads leaves out as workers
    /// (orchestration workers, from the whole agent list, and Away's
    /// reviews), sorted. Chat's session list leaves them out too. Absent
    /// when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_keys: Vec<String>,
}

impl ThreadsView {
    /// A view with no threads, revision 0 and default settings.
    #[must_use]
    pub fn empty(now: u64) -> Self {
        Self {
            version: THREADS_VIEW_VERSION,
            revision: 0,
            now,
            settings: ThreadSettings::default(),
            threads: Vec::new(),
            hidden_keys: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedWorkspace {
    pub token: String,
    pub cwd: String,
    pub workspace: ThreadWorkspace,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchList {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    pub branches: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorktreeEntry {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffFile {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: DiffFileStatus,
    pub added: u32,
    pub removed: u32,
    pub binary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omitted: Option<DiffOmitted>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadDiff {
    pub scope: DiffScope,
    pub base: String,
    pub head: String,
    pub files: Vec<DiffFile>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreResult {
    pub restored: u32,
    pub recovery_ref: String,
}

/// A file Commit would stage, relative to the repository root.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShipFile {
    pub path: String,
    pub status: DiffFileStatus,
}

/// An exact checkout snapshot reviewed before an explicit partial commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitReview {
    pub revision: String,
    pub files: Vec<CommitReviewFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitReviewFile {
    pub path: String,
    pub status: DiffFileStatus,
    /// An edit event named the file, not proof of ownership of every line.
    pub attributed: bool,
    pub hunks: Vec<CommitReviewHunk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitReviewHunk {
    pub id: u32,
    pub patch: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommitChoice {
    pub path: String,
    /// Absent selects the whole file; an empty list selects nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunks: Option<Vec<u32>>,
}

/// A changed file Commit leaves out, and why.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShipLeftOut {
    pub path: String,
    pub reason: LeftOutReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShipPullRequest {
    pub number: u64,
    pub url: String,
    pub state: PullRequestState,
    pub title: String,
    /// The branch it merges into.
    pub base: String,
    pub draft: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShipCheck {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    pub bucket: CheckBucket,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShipChecks {
    pub state: ChecksState,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    /// Skipped and cancelled.
    pub skipped: u32,
    /// The failing checks, at most 20.
    pub failing: Vec<ShipCheck>,
}

/// A thread's checkout, read for shipping its work (protocol 37).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadShip {
    pub key: String,
    /// Absent on a detached HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<u32>,
    /// What Commit stages: a worktree thread's every change; a Local
    /// thread's changed files its own turns' edits named. Never a
    /// credential-shaped file.
    pub files: Vec<ShipFile>,
    pub left_out: Vec<ShipLeftOut>,
    /// A Local thread has turns whose files are unknown (or made by shell
    /// commands), so Commit may leave out some of its changes.
    pub files_unknown: bool,
    /// Why Commit cannot run now (a merge in progress, too many files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_blocked: Option<String>,
    /// The suggested message: the thread's title and its turns' prompts.
    pub message: String,
    /// Why Push & open PR is not offered (the default branch, a detached
    /// HEAD, no remote).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_blocked: Option<String>,
    /// On the default branch: a branch name to create first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_branch: Option<String>,
    pub gh: GhState,
    /// How to make `gh` work, when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gh_fix: Option<String>,
    /// The remote parts (`gh`, the remote's default branch) were read.
    pub remote_read: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<ShipPullRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<ShipChecks>,
}

/// How a ship effect ended, when it did not fail outright.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShipOutcome {
    pub action: ShipAction,
    pub certainty: ShipCertainty,
    pub message: String,
}

/// An unresolved pull request review thread, for the thread's draft.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReviewComment {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// Absent for a comment on the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub side: DiffSide,
    /// The commented lines from the review's diff hunk ("" when unknown).
    pub code: String,
    /// Every comment in the thread, "@author: body", credentials redacted.
    pub comment: String,
    pub url: String,
    /// The lines changed since the comment was made.
    pub outdated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadControlResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<crate::workflows::WorkflowView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<crate::workflows::SetupReport>,
    pub view: ThreadsView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared: Option<PreparedWorkspace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branches: Option<BranchList>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees: Option<Vec<WorktreeEntry>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<ThreadDiff>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreResult>,
    /// The new thread's key (`forkAt` / `editFrom`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// `editFrom`: the edited turn's prompt text when known;
    /// `failingChecksPrompt`: the prompt to send the thread;
    /// `switchProvider` (protocol 59): the handoff a new ACP session was
    /// launched with (no send echoed it), for the pane to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The thread's checkout for shipping (protocol 37).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship: Option<ThreadShip>,
    /// How a ship effect ended (protocol 37).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipped: Option<ShipOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_review: Option<CommitReview>,
    /// `reviewComments`: the pull request's unresolved review threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_comments: Option<Vec<ReviewComment>>,
    /// `openInTerminal` (protocol 42): the Code pane to open. Its structured
    /// session is closed and the session is held for this pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<ThreadTerminalHandoff>,
    /// `secondOpinion` (protocol 46): the consult it submitted, as the
    /// collaboration channels report it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consult: Option<crate::ipc::CollaborationStatusDto>,
    /// `switchProvider` (protocol 59): whether the handoff prompt reached
    /// the new head (`key`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<HandoffDelivery>,
    /// `suggestShipText` (protocol 67, T3 section 12): the text Claude
    /// wrote, for the owner to edit. Nothing was committed or pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ship_text: Option<String>,
    /// `folderTrust` and `trustFolder` (protocol 72): where the folder
    /// stands now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_trust: Option<FolderTrustAnswer>,
}

/// Protocol 67 (T3 section 12): which text Write with AI writes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShipTextKind {
    /// A commit message for the changes being committed.
    Commit,
    /// A pull request's title (the first line) and body (the rest).
    PullRequest,
}

/// Where and how a Code pane resumes a thread's provider session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadTerminalHandoff {
    /// `claude` or `codex`.
    pub profile_id: crate::terminal::TerminalLaunchProfileId,
    /// The provider's own session id, which the pane resumes.
    pub provider_session_id: String,
    /// The folder the session runs in (a worktree thread's worktree).
    pub cwd: String,
}

/// A response that carries only `view`.
impl From<ThreadsView> for ThreadControlResponse {
    fn from(view: ThreadsView) -> Self {
        Self {
            view,
            prepared: None,
            workflow: None,
            setup: None,
            branches: None,
            worktrees: None,
            diff: None,
            restore: None,
            key: None,
            prompt: None,
            ship: None,
            shipped: None,
            commit_review: None,
            review_comments: None,
            terminal: None,
            consult: None,
            handoff: None,
            ship_text: None,
            folder_trust: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadCommand {
    Read {},
    Workflow {
        command: crate::workflows::WorkflowCommand,
    },
    Pin {
        key: String,
    },
    Unpin {
        key: String,
    },
    /// Place `key` before `before_key` in `section` (pinned|active); absent = end.
    Reorder {
        key: String,
        section: ThreadSection,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_key: Option<String>,
    },
    Settle {
        key: String,
    },
    Unsettle {
        key: String,
    },
    Snooze {
        key: String,
        until: u64,
    },
    Unsnooze {
        key: String,
    },
    Visit {
        key: String,
    },
    MarkUnread {
        key: String,
    },
    SetAutoSettle {
        key: String,
        enabled: bool,
    },
    SetSettings {
        settings: ThreadSettings,
    },
    /// mode local|worktree|existing (`existing` requires `worktree_path`).
    PrepareWorkspace {
        project_root: String,
        mode: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_ref: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
        hint: String,
    },
    DiscardWorkspace {
        token: String,
    },
    /// After a launch answered `key`. `token` from `PrepareWorkspace`; absent for plain chat.
    Bind {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title_source: Option<TitleSource>,
    },
    ListBranches {
        project_root: String,
    },
    ListWorktrees {
        project_root: String,
    },
    TurnDiff {
        key: String,
        ordinal: u32,
    },
    ThreadDiff {
        key: String,
        scope: DiffScope,
    },
    RestoreTurn {
        key: String,
        ordinal: u32,
    },
    /// Keeps the thread's first `keep_turns` RECORDED turns (`turns[]`
    /// ordinals `0..keep_turns`), not the provider's first prompts: the host
    /// names the last kept turn's prompt to the provider by its digest.
    ForkAt {
        key: String,
        keep_turns: u32,
    },
    /// `ordinal` is a RECORDED turn's (`turns[]`); the new thread keeps the
    /// recorded turns before it, as `ForkAt { keep_turns: ordinal }` does.
    EditFrom {
        key: String,
        ordinal: u32,
        restore_files: bool,
    },
    ReleaseWorkspace {
        key: String,
    },
    RegenerateTitle {
        key: String,
    },
    RefreshPullRequest {
        key: String,
    },
    /// Protocol 37: reads the thread's checkout for shipping; `remote` also
    /// asks `gh` and the remote.
    ShipStatus {
        key: String,
        remote: bool,
    },
    /// Commits exactly `paths` — the files the last status listed — with
    /// `message`. Refused when they no longer are what Commit would stage.
    Commit {
        key: String,
        message: String,
        paths: Vec<String>,
    },
    ReviewCommit {
        key: String,
    },
    CommitSelected {
        key: String,
        revision: String,
        message: String,
        choices: Vec<CommitChoice>,
    },
    /// A Local thread's checkout switches to a new `branch` at HEAD.
    CreateBranch {
        key: String,
        branch: String,
    },
    /// Pushes the branch and opens its pull request, unless one is open.
    OpenPullRequest {
        key: String,
        title: String,
        body: String,
    },
    /// The prompt that asks the thread to fix its pull request's failing checks.
    FailingChecksPrompt {
        key: String,
    },
    /// The pull request's unresolved review threads.
    ReviewComments {
        key: String,
    },
    /// Merges the thread's pull request `number`, which the owner confirmed.
    MergePullRequest {
        key: String,
        number: u64,
        method: MergeMethod,
    },
    /// Protocol 42: closes the thread's structured session and holds the
    /// session for a Code pane, which the answer's `terminal` names. Refused
    /// while a turn runs or a pane already drives the session.
    OpenInTerminal {
        key: String,
    },
    /// Protocol 42: brings a session a Code pane or the CLI ran into Threads
    /// (Active, visited). Refused while a running pane still drives it; the
    /// pane is ended first.
    ContinueFromTerminal {
        key: String,
    },
    /// Protocol 46: asks `engine` for a second opinion on the thread's
    /// latest answer and its turn's changes, through the consult path. The
    /// answer's `consult` is the submitted collaboration; its reply reaches
    /// the thread only when the owner sends it on. Refused mid-turn, for a
    /// session a Code pane holds, and for the thread's own provider.
    SecondOpinion {
        key: String,
        engine: crate::ipc::CollaborationEngine,
        /// The collaboration's idempotency key.
        request_id: String,
    },
    /// Protocol 46: prepares a new thread on `engine` that continues this
    /// one: the same folder (Local) or worktree, and a first prompt carrying
    /// the conversation and the thread's changes, in the answer's `prepared`
    /// and `prompt`. The renderer launches it and binds it with the token;
    /// the new thread's lineage is `handoff`. Project threads only, between
    /// turns.
    ContinueWith {
        key: String,
        engine: crate::engine::EngineId,
        /// Protocol 60: the provider account the new thread will run on
        /// (the renderer launches it with this `LaunchOptions.account`). With
        /// one, `engine` may be the thread's own provider, on another account.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
    /// Protocol 59: carries on in the same thread on `engine`: back to that
    /// provider's earlier session in this thread when it still exists, else
    /// a new session in the thread's folder. The thread's place, pin,
    /// title, project, workspace and pull request move to the new head,
    /// which is sent a handoff (or what it missed). The answer's `key` is the
    /// new head and `handoff` says whether that prompt arrived. Project
    /// threads only, between turns. Not replay-safe.
    SwitchProvider {
        key: String,
        engine: crate::engine::EngineId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Protocol 62 (T3 section 7): schedules `prompt` for thread `key` (its
    /// head when `key` is an earlier segment), or, with `id`, changes that
    /// schedule (its due time is worked out again; a run waiting for the
    /// thread is dropped). Refused for a thread that is not listed, a prompt
    /// over 16 KiB, an interval under 5 minutes, a time in the past.
    ScheduleSet {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        prompt: String,
        when: ScheduleWhen,
    },
    /// Protocol 62: deletes schedule `id`.
    ScheduleDelete {
        id: String,
    },
    /// Protocol 62: turns schedule `id` on (its next due time from now) or
    /// off (a run waiting for the thread is dropped).
    ScheduleEnable {
        id: String,
        enabled: bool,
    },
    /// Protocol 62: turns thread `key`'s pull-request watch on (it needs an
    /// open pull request) or off.
    WatchSet {
        key: String,
        enabled: bool,
    },
    /// Protocol 66 (T3 section 10): Resume at reset. Schedules thread `key`
    /// to carry on by itself a minute after its provider's limit resets (the
    /// reset Rust recorded for it), replacing its earlier resume. `prompt`
    /// is the owner's queued follow-up that never went out, when there is
    /// one; otherwise Rust picks the prompt. Refused when the thread has no
    /// limit with a known reset.
    ResumeAtReset {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    /// Protocol 67 (T3 section 12): Write with AI. One tool-less Claude call
    /// (Claude's default account) writes a commit message (`paths`: the
    /// files being committed; absent = every file Commit would stage) or a
    /// pull request's text from the thread's changes and prompts, answered
    /// as `shipText`. Clicking it is the owner's consent; nothing is
    /// committed or pushed. Definitely unsent when it fails.
    SuggestShipText {
        key: String,
        what: ShipTextKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paths: Option<Vec<String>>,
    },
    /// Protocol 67 (T3 section 12): asks `question` without adding to the
    /// thread: Rust forks the thread at its latest turn (Claude and Codex
    /// only) into a side thread no inbox lists, runs it read-only where the
    /// provider can and with no team tools, and sends it the question. The
    /// answer's `key` is the side thread's; the thread's view carries
    /// `side`. One at a time per thread; refused mid-turn.
    SideAsk {
        key: String,
        question: String,
    },
    /// Protocol 67: closes thread `key`'s side question and removes its
    /// side thread (Bring back copies the answer into the draft first).
    SideClose {
        key: String,
    },
    /// Protocol 72: where `folder` (absolute) stands for Claude's Default
    /// account: the answer's `folderTrust` names the folder the answer
    /// covers. A read; the new-thread card asks it before a Claude thread's
    /// first prompt.
    FolderTrust {
        folder: String,
    },
    /// Protocol 72: records the owner's answer for `folder`'s trust root:
    /// `trust` trusts it, otherwise it is declined (a trusted folder stays
    /// trusted). After a trust, the folder's idle Claude threads end their
    /// provider process so their next message starts it with the folder's
    /// settings; a thread mid-turn finishes its turn first.
    TrustFolder {
        folder: String,
        trust: bool,
    },
}

impl ThreadCommand {
    /// The wire `kind` of this command.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Read {} => "read",
            Self::Workflow { .. } => "workflow",
            Self::Pin { .. } => "pin",
            Self::Unpin { .. } => "unpin",
            Self::Reorder { .. } => "reorder",
            Self::Settle { .. } => "settle",
            Self::Unsettle { .. } => "unsettle",
            Self::Snooze { .. } => "snooze",
            Self::Unsnooze { .. } => "unsnooze",
            Self::Visit { .. } => "visit",
            Self::MarkUnread { .. } => "markUnread",
            Self::SetAutoSettle { .. } => "setAutoSettle",
            Self::SetSettings { .. } => "setSettings",
            Self::PrepareWorkspace { .. } => "prepareWorkspace",
            Self::DiscardWorkspace { .. } => "discardWorkspace",
            Self::Bind { .. } => "bind",
            Self::ListBranches { .. } => "listBranches",
            Self::ListWorktrees { .. } => "listWorktrees",
            Self::TurnDiff { .. } => "turnDiff",
            Self::ThreadDiff { .. } => "threadDiff",
            Self::RestoreTurn { .. } => "restoreTurn",
            Self::ForkAt { .. } => "forkAt",
            Self::EditFrom { .. } => "editFrom",
            Self::ReleaseWorkspace { .. } => "releaseWorkspace",
            Self::RegenerateTitle { .. } => "regenerateTitle",
            Self::RefreshPullRequest { .. } => "refreshPullRequest",
            Self::ShipStatus { .. } => "shipStatus",
            Self::Commit { .. } => "commit",
            Self::ReviewCommit { .. } => "reviewCommit",
            Self::CommitSelected { .. } => "commitSelected",
            Self::CreateBranch { .. } => "createBranch",
            Self::OpenPullRequest { .. } => "openPullRequest",
            Self::FailingChecksPrompt { .. } => "failingChecksPrompt",
            Self::ReviewComments { .. } => "reviewComments",
            Self::MergePullRequest { .. } => "mergePullRequest",
            Self::OpenInTerminal { .. } => "openInTerminal",
            Self::ContinueFromTerminal { .. } => "continueFromTerminal",
            Self::SecondOpinion { .. } => "secondOpinion",
            Self::ContinueWith { .. } => "continueWith",
            Self::SwitchProvider { .. } => "switchProvider",
            Self::ScheduleSet { .. } => "scheduleSet",
            Self::ScheduleDelete { .. } => "scheduleDelete",
            Self::ScheduleEnable { .. } => "scheduleEnable",
            Self::WatchSet { .. } => "watchSet",
            Self::ResumeAtReset { .. } => "resumeAtReset",
            Self::SuggestShipText { .. } => "suggestShipText",
            Self::SideAsk { .. } => "sideAsk",
            Self::SideClose { .. } => "sideClose",
            Self::FolderTrust { .. } => "folderTrust",
            Self::TrustFolder { .. } => "trustFolder",
        }
    }
}

/// Every [`ThreadCommand`] kind, in declaration order.
pub const THREAD_COMMAND_KINDS: [&str; 50] = [
    "read",
    "workflow",
    "pin",
    "unpin",
    "reorder",
    "settle",
    "unsettle",
    "snooze",
    "unsnooze",
    "visit",
    "markUnread",
    "setAutoSettle",
    "setSettings",
    "prepareWorkspace",
    "discardWorkspace",
    "bind",
    "listBranches",
    "listWorktrees",
    "turnDiff",
    "threadDiff",
    "restoreTurn",
    "forkAt",
    "editFrom",
    "releaseWorkspace",
    "regenerateTitle",
    "refreshPullRequest",
    "shipStatus",
    "commit",
    "reviewCommit",
    "commitSelected",
    "createBranch",
    "openPullRequest",
    "failingChecksPrompt",
    "reviewComments",
    "mergePullRequest",
    "openInTerminal",
    "continueFromTerminal",
    "secondOpinion",
    "continueWith",
    "switchProvider",
    "scheduleSet",
    "scheduleDelete",
    "scheduleEnable",
    "watchSet",
    "resumeAtReset",
    "suggestShipText",
    "sideAsk",
    "sideClose",
    "folderTrust",
    "trustFolder",
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-v34.json"
        ))
        .unwrap()
    }

    fn round_trips<T: DeserializeOwned + Serialize>(value: &Value) -> T {
        let decoded: T = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), *value);
        decoded
    }

    fn rejects_extra<T: DeserializeOwned>(value: &Value, pointer: &str) {
        let mut foreign = value.clone();
        foreign
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("no object at {pointer}"))
            .insert("executeShell".into(), json!(true));
        assert!(
            serde_json::from_value::<T>(foreign).is_err(),
            "extra field accepted at {pointer:?}"
        );
    }

    fn fixture_v37() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-v37.json"
        ))
        .unwrap()
    }

    fn fixture_v42() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-v42.json"
        ))
        .unwrap()
    }

    fn fixture_v46() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-handoff-v46.json"
        ))
        .unwrap()
    }

    fn fixture_v57() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-team-v57.json"
        ))
        .unwrap()
    }

    fn fixture_v59() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-switch-v59.json"
        ))
        .unwrap()
    }

    fn fixture_v65() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/outside-agents-v65.json"
        ))
        .unwrap()
    }

    /// Protocol 65 (outside agents): the `outsideAgents` setting and a
    /// thread's `startedFromOutside` round-trip and refuse fields they do not
    /// know; an older view reads as off and writes nothing for it.
    #[test]
    fn shared_v65_outside_agents_setting_and_started_from_outside_round_trip() {
        let fixture = fixture_v65();
        assert_eq!(fixture["protocolVersion"], 65);
        let view: ThreadsView = round_trips(&fixture["view"]);
        assert_eq!(view.settings.outside_agents, OutsideAgents::Full);
        assert_eq!(
            view.threads[0].started_from_outside.as_deref(),
            Some("claude-code")
        );
        rejects_extra::<ThreadsView>(&fixture["view"], "/settings");
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0");
        let levels = fixture["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| match round_trips::<ThreadCommand>(value) {
                ThreadCommand::SetSettings { settings } => settings.outside_agents,
                other => panic!("unexpected command {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(levels, [OutsideAgents::Full, OutsideAgents::Read]);
        assert!(
            serde_json::from_value::<ThreadSettings>(json!({
                "autoSettleDays": 3, "settleOnMerge": true, "generatedTitles": false,
                "trackPullRequests": true, "outsideAgents": "admin"
            }))
            .is_err(),
            "a level the contract does not name"
        );
        let older: ThreadsView = round_trips(&fixture_v59()["view"]);
        assert_eq!(older.settings.outside_agents, OutsideAgents::Off);
        assert!(
            older
                .threads
                .iter()
                .all(|thread| thread.started_from_outside.is_none())
        );
        assert!(
            serde_json::to_value(ThreadSettings::default()).unwrap()["outsideAgents"].is_null(),
            "off is not written"
        );
    }

    fn fixture_v62() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-schedules-v62.json"
        ))
        .unwrap()
    }

    fn fixture_v66() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-resume-v66.json"
        ))
        .unwrap()
    }

    fn fixture_v67() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/quick-wins-v67.json"
        ))
        .unwrap()
    }

    fn fixture_v72() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/folder-trust-v72.json"
        ))
        .unwrap()
    }

    /// Protocol 72: folder trust. The two commands and their answers
    /// round-trip; a Claude thread in a folder nobody trusted carries
    /// `folderTrust`, every other thread and every older view carries
    /// nothing; unknown fields and states are refused.
    #[test]
    fn shared_v72_folder_trust_round_trips() {
        let fixture = fixture_v72();
        assert_eq!(fixture["protocolVersion"], 72);
        let commands: Vec<ThreadCommand> = fixture["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|command| {
                rejects_extra::<ThreadCommand>(command, "");
                round_trips(command)
            })
            .collect();
        assert_eq!(
            commands,
            [
                ThreadCommand::FolderTrust {
                    folder: "/Users/fixture/Downloads/cloned-tool".into()
                },
                ThreadCommand::TrustFolder {
                    folder: "/Users/fixture/Downloads/cloned-tool/src".into(),
                    trust: true
                },
                ThreadCommand::TrustFolder {
                    folder: "/Users/fixture/Downloads/cloned-tool".into(),
                    trust: false
                },
            ]
        );
        let states: Vec<FolderTrustState> = fixture["responses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|response| {
                rejects_extra::<ThreadControlResponse>(response, "/folderTrust");
                let response: ThreadControlResponse = round_trips(response);
                response.folder_trust.unwrap().state
            })
            .collect();
        assert_eq!(
            states,
            [
                FolderTrustState::Unasked,
                FolderTrustState::Trusted,
                FolderTrustState::Declined
            ]
        );
        assert!(
            serde_json::from_value::<FolderTrustAnswer>(json!({"folder": "/a", "state": "maybe"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({"kind": "trustFolder", "folder": "/a"}))
                .is_err()
        );
        let view: ThreadsView = round_trips(&fixture["view"]);
        assert_eq!(
            view.threads
                .iter()
                .map(|thread| thread.folder_trust.clone())
                .collect::<Vec<_>>(),
            [
                Some(ThreadFolderTrust {
                    folder: "/Users/fixture/Downloads/cloned-tool".into(),
                    asked: false
                }),
                Some(ThreadFolderTrust {
                    folder: "/Users/fixture/Downloads/other-tool".into(),
                    asked: true
                }),
                None,
            ]
        );
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0/folderTrust");
        // An older view carries none, and writes none back.
        let older: ThreadsView = round_trips(&fixture_v67()["view"]);
        assert!(
            older
                .threads
                .iter()
                .all(|thread| thread.folder_trust.is_none())
        );
        assert!(
            !serde_json::to_string(&fixture_v67()["view"])
                .unwrap()
                .contains("folderTrust")
        );
    }

    #[test]
    fn a_folder_trust_path_is_absolute_bounded_and_free_of_control_characters() {
        assert!(valid_folder_trust_path("/Users/me/code/app"));
        assert!(!valid_folder_trust_path("code/app"));
        assert!(!valid_folder_trust_path(""));
        assert!(!valid_folder_trust_path("/tmp/a\u{7}b"));
        assert!(!valid_folder_trust_path(&format!(
            "/{}",
            "a".repeat(FOLDER_TRUST_MAX_PATH_BYTES)
        )));
    }

    fn fixture_v69() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-totals-v69.json"
        ))
        .unwrap()
    }

    /// Protocol 69: each segment of a switched thread's chain carries the
    /// lines its own turns changed, both counts or neither; a segment with
    /// no measured turn and an older chain write neither, and the segment
    /// still refuses fields it does not know.
    #[test]
    fn shared_v69_chain_segment_line_totals_round_trip() {
        let fixture = fixture_v69();
        assert_eq!(fixture["protocolVersion"], 69);
        let view: ThreadsView = round_trips(&fixture["view"]);
        let segments = &view.threads[0].chain.as_ref().unwrap().segments;
        assert_eq!(
            segments
                .iter()
                .map(|segment| (segment.added, segment.removed))
                .collect::<Vec<_>>(),
            [(Some(7), Some(2)), (None, None), (Some(3), Some(1))]
        );
        assert!(view.threads[1].chain.is_none());
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0/chain/segments/0");
        let older: ThreadsView = round_trips(&fixture_v59()["view"]);
        let older = &older.threads[0].chain.as_ref().unwrap().segments;
        assert!(
            older
                .iter()
                .all(|segment| segment.added.is_none() && segment.removed.is_none())
        );
    }

    /// Protocol 66 (T3 section 10, resume at reset): a rate-limit notice
    /// carries its reset (or none), a thread its limit (with or without a
    /// known reset), a resume schedule its kind, and `resumeAtReset` its
    /// optional queued prompt; each refuses fields it does not know, and an
    /// older view or schedule has neither and writes neither.
    #[test]
    fn shared_v66_resume_at_reset_round_trips_and_rejects_extra_fields() {
        use crate::engine::EngineEventKind;
        let fixture = fixture_v66();
        assert_eq!(fixture["protocolVersion"], 66);
        let events = fixture["events"].as_array().unwrap();
        let first: EngineEventKind = round_trips(&events[0]);
        assert!(matches!(
            first,
            EngineEventKind::UsageUpdated { rate_limited: Some(ref notice), .. }
                if notice.resets_at.as_deref() == Some("2026-11-15T21:10:00.000Z")
        ));
        let second: EngineEventKind = round_trips(&events[1]);
        assert!(matches!(
            second,
            EngineEventKind::UsageUpdated { rate_limited: Some(ref notice), .. }
                if notice.resets_at.is_none()
        ));
        rejects_extra::<EngineEventKind>(&events[0], "/rateLimited");
        let view: ThreadsView = round_trips(&fixture["view"]);
        let limited = &view.threads[0];
        assert_eq!(
            limited.limit.as_ref().unwrap().resets_at.as_deref(),
            Some("2026-11-15T21:10:00.000Z")
        );
        let resume = &limited.schedules.as_ref().unwrap().schedules[0];
        assert_eq!(resume.kind, ScheduleKind::Resume);
        assert_eq!(
            resume.when,
            ScheduleWhen::Once {
                at: 1_794_777_060_000
            }
        );
        assert_eq!(view.threads[1].limit, Some(ThreadLimit::default()));
        for pointer in ["/threads/0/limit", "/threads/0/schedules/schedules/0"] {
            rejects_extra::<ThreadsView>(&fixture["view"], pointer);
        }
        let mut wrong = fixture["view"].clone();
        wrong["threads"][0]["schedules"]["schedules"][0]["kind"] = json!("later");
        assert!(serde_json::from_value::<ThreadsView>(wrong).is_err());
        // A prompt schedule and a thread with no limit write neither field.
        let older: ThreadsView = round_trips(&fixture_v62()["view"]);
        let older = serde_json::to_value(&older).unwrap();
        assert!(older["threads"][0].get("limit").is_none());
        assert!(
            older["threads"][0]["schedules"]["schedules"][0]
                .get("kind")
                .is_none()
        );
        let commands = fixture["commands"].as_array().unwrap();
        let bare: ThreadCommand = round_trips(&commands[0]);
        assert!(matches!(
            bare,
            ThreadCommand::ResumeAtReset { prompt: None, .. }
        ));
        let queued: ThreadCommand = round_trips(&commands[1]);
        assert!(matches!(
            queued,
            ThreadCommand::ResumeAtReset { prompt: Some(ref prompt), .. }
                if prompt == "Then run the migration on staging."
        ));
    }

    /// Protocol 67 (T3 section 12, quick wins): Write with AI's command for
    /// a commit (with the files being committed) and a pull request, and the
    /// answer's `shipText`, round-trip and refuse fields they do not know.
    #[test]
    fn shared_v67_quick_wins_round_trip() {
        let fixture = fixture_v67();
        assert_eq!(fixture["protocolVersion"], 67);
        let commands = fixture["commands"].as_array().unwrap();
        let first: ThreadCommand = round_trips(&commands[0]);
        assert_eq!(
            first,
            ThreadCommand::SuggestShipText {
                key: "claude#a".into(),
                what: ShipTextKind::Commit,
                paths: Some(vec!["src/lib.rs".into()]),
            }
        );
        let second: ThreadCommand = round_trips(&commands[1]);
        let _: ThreadCommand = round_trips(&commands[2]);
        assert!(matches!(
            second,
            ThreadCommand::SuggestShipText {
                what: ShipTextKind::PullRequest,
                paths: None,
                ..
            }
        ));
        for command in commands {
            rejects_extra::<ThreadCommand>(command, "");
        }
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({
                "kind": "suggestShipText", "key": "claude#a", "what": "release notes"
            }))
            .is_err()
        );
        let response: ThreadControlResponse = round_trips(&fixture["response"]);
        assert!(
            response
                .ship_text
                .unwrap()
                .starts_with("Fix the login test")
        );
        rejects_extra::<ThreadControlResponse>(&fixture["response"], "");
        // The private secret card: the open request a thread's view carries.
        let view: ThreadsView = round_trips(&fixture["view"]);
        let request = view.threads[0].secret_request.as_ref().unwrap();
        assert_eq!(
            (request.id.as_str(), request.agent.as_str()),
            ("secret-1", "Claude")
        );
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0/secretRequest");
        // The side question a thread's view carries, and its two commands.
        let side = view.threads[0].side.as_ref().unwrap();
        assert_eq!(side.key, "claude#side-7f3a");
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0/side");
        let kinds = commands
            .iter()
            .map(|value| value["kind"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            ["suggestShipText", "suggestShipText", "sideAsk", "sideClose"]
        );
    }

    /// Protocol 62 (scheduled prompts and pull-request watch): the four
    /// schedule commands, a thread's schedules of every kind and its watch
    /// round-trip and refuse fields they do not know; an older view has no
    /// schedules and writes none.
    #[test]
    fn shared_v62_schedules_and_watch_round_trip() {
        let fixture = fixture_v62();
        assert_eq!(fixture["protocolVersion"], 62);
        let view: ThreadsView = round_trips(&fixture["view"]);
        let schedules = view.threads[0].schedules.as_ref().unwrap();
        assert_eq!(schedules.schedules.len(), 3);
        assert_eq!(
            schedules.schedules[0].when,
            ScheduleWhen::Interval { minutes: 60 }
        );
        assert_eq!(schedules.schedules[0].set_by, ScheduleSetBy::Agent);
        assert!(matches!(
            schedules.schedules[1].when,
            ScheduleWhen::Calendar { ref calendar } if calendar.weekdays == [1, 2, 3, 4, 5]
        ));
        assert_eq!(schedules.schedules[1].waiting, Some(1_791_293_400_000));
        assert_eq!(
            schedules.schedules[2].last_run.as_ref().unwrap().outcome,
            ScheduleOutcome::Skipped
        );
        let watch = schedules.watch.as_ref().unwrap();
        assert_eq!(
            watch.last_run.as_ref().unwrap().outcome,
            ScheduleOutcome::Unknown
        );
        for pointer in [
            "/threads/0/schedules",
            "/threads/0/schedules/schedules/0",
            "/threads/0/schedules/schedules/0/when",
            "/threads/0/schedules/schedules/0/lastRun",
            "/threads/0/schedules/schedules/1/when/calendar",
            "/threads/0/schedules/watch",
        ] {
            rejects_extra::<ThreadsView>(&fixture["view"], pointer);
        }
        let _: ThreadControlResponse = round_trips(&fixture["response"]);
        let mut kinds = BTreeSet::new();
        for value in fixture["commands"].as_array().unwrap() {
            let command: ThreadCommand = round_trips(value);
            kinds.insert(command.kind());
            rejects_extra::<ThreadCommand>(value, "");
        }
        assert_eq!(
            kinds,
            BTreeSet::from([
                "scheduleSet",
                "scheduleDelete",
                "scheduleEnable",
                "watchSet"
            ])
        );
        rejects_extra::<ThreadCommand>(&fixture["commands"][0], "/when");
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({
                "kind": "scheduleSet", "key": "claude#a", "prompt": "p",
                "when": {"kind": "cron", "spec": "* * * * *"}
            }))
            .is_err(),
            "a kind of schedule the contract does not name"
        );
        let older: ThreadsView = round_trips(&fixture_v59()["view"]);
        assert!(
            older
                .threads
                .iter()
                .all(|thread| thread.schedules.is_none())
        );
    }

    /// Protocol 59 (switch provider): `switchProvider` with and without a
    /// model, a head's chain of segments and the answer's `handoff`
    /// round-trip and refuse fields they do not know; an older view has no
    /// chain and writes none.
    #[test]
    fn shared_v59_switch_provider_chain_and_handoff_round_trip() {
        let fixture = fixture_v59();
        assert_eq!(fixture["protocolVersion"], 59);
        let view: ThreadsView = round_trips(&fixture["view"]);
        let chain = view.threads[0].chain.as_ref().unwrap();
        assert_eq!(chain.segments.len(), 2);
        assert_eq!(
            chain.segments[1].key, view.threads[0].key,
            "the head is last"
        );
        assert_eq!(chain.segments[0].engine.as_str(), "claude");
        for pointer in ["/threads/0/chain", "/threads/0/chain/segments/0"] {
            rejects_extra::<ThreadsView>(&fixture["view"], pointer);
        }
        let response: ThreadControlResponse = round_trips(&fixture["response"]);
        assert_eq!(response.handoff, Some(HandoffDelivery::Unknown));
        assert_eq!(response.key.as_deref(), Some(view.threads[0].key.as_str()));
        rejects_extra::<ThreadControlResponse>(&fixture["response"], "");
        for value in fixture["commands"].as_array().unwrap() {
            let command: ThreadCommand = round_trips(value);
            assert_eq!(command.kind(), "switchProvider");
            rejects_extra::<ThreadCommand>(value, "");
        }
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({
                "kind": "switchProvider", "key": "claude#a", "engine": "gpt"
            }))
            .is_err(),
            "an engine id the contract does not admit"
        );
        let older: ThreadsView = round_trips(&fixture_v57()["view"]);
        assert!(older.threads.iter().all(|thread| thread.chain.is_none()));
    }

    /// Protocol 57 (team tools): a started thread's `child` lineage and the
    /// `teamTools` setting round-trip and refuse fields they do not know; a
    /// view without the setting reads it as on and writes nothing for it.
    #[test]
    fn shared_v57_child_lineage_and_team_tools_setting_round_trip() {
        let fixture = fixture_v57();
        assert_eq!(fixture["protocolVersion"], 57);
        let view: ThreadsView = round_trips(&fixture["view"]);
        let lineage = view.threads[0].lineage.as_ref().unwrap();
        assert_eq!(lineage.relation, LineageRelation::Child);
        assert_eq!(lineage.parent, view.threads[1].key);
        assert!(!view.settings.team_tools);
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0/lineage");
        rejects_extra::<ThreadsView>(&fixture["view"], "/settings");
        let command: ThreadCommand = round_trips(&fixture["commands"][0]);
        assert!(matches!(
            command,
            ThreadCommand::SetSettings { ref settings } if !settings.team_tools
        ));
        let older: ThreadsView = round_trips(&fixture_v46()["view"]);
        assert!(older.settings.team_tools, "absent means on");
        assert!(
            serde_json::to_value(ThreadSettings::default()).unwrap()["teamTools"].is_null(),
            "the default is not written"
        );
    }

    /// Protocol 71: the "Pane tools without asking" setting round-trips and
    /// refuses fields it does not know; settings without it read it as off
    /// and write nothing for it.
    #[test]
    fn shared_v71_pane_tools_setting_round_trips_and_its_absence_means_ask() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/ask-first-v71.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 71);
        let settings: ThreadSettings = round_trips(&fixture["threadSettings"]);
        assert!(settings.pane_tools_without_asking);
        let command: ThreadCommand = round_trips(&fixture["threadCommands"][0]);
        assert!(matches!(
            command,
            ThreadCommand::SetSettings { ref settings } if settings.pane_tools_without_asking
        ));
        rejects_extra::<ThreadCommand>(&fixture["threadCommands"][0], "/settings");
        let older: ThreadsView = round_trips(&fixture_v57()["view"]);
        assert!(
            !older.settings.pane_tools_without_asking,
            "absent means off"
        );
        assert!(
            serde_json::to_value(ThreadSettings::default()).unwrap()["paneToolsWithoutAsking"]
                .is_null(),
            "off is not written"
        );
    }

    /// Protocol 46 (review B4): Second opinion and Continue with round-trip
    /// and refuse fields they do not know; a handed-off thread's lineage has
    /// its wire name, and a response carries the consult it submitted.
    #[test]
    fn shared_v46_second_opinion_and_handoff_round_trip_and_reject_extra_fields() {
        let fixture = fixture_v46();
        assert_eq!(fixture["protocolVersion"], 46);
        let view: ThreadsView = round_trips(&fixture["view"]);
        let lineage = view.threads[0].lineage.as_ref().unwrap();
        assert_eq!(lineage.relation, LineageRelation::Handoff);
        assert_eq!(lineage.kept_turns, 0);
        assert_eq!(lineage.parent, view.threads[1].key);
        rejects_extra::<ThreadsView>(&fixture["view"], "/threads/0/lineage");
        let response: ThreadControlResponse = round_trips(&fixture["response"]);
        let consult = response.consult.as_ref().unwrap();
        assert_eq!(consult.receipt.mode, crate::ipc::CollaborationMode::Consult);
        assert_eq!(consult.source_session_key, view.threads[1].key);
        for pointer in ["", "/consult", "/consult/receipt", "/consult/approval"] {
            rejects_extra::<ThreadControlResponse>(&fixture["response"], pointer);
        }
        let commands = fixture["commands"].as_array().unwrap();
        for value in commands {
            let command: ThreadCommand = round_trips(value);
            assert!(matches!(
                command,
                ThreadCommand::SecondOpinion { .. } | ThreadCommand::ContinueWith { .. }
            ));
            rejects_extra::<ThreadCommand>(value, "");
        }
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({
                "kind": "secondOpinion", "key": "claude#a", "engine": "gemini", "requestId": "r"
            }))
            .is_err(),
            "a consult target the collaboration channels do not name"
        );
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({
                "kind": "continueWith", "key": "claude#a", "engine": "gpt"
            }))
            .is_err(),
            "an engine id the contract does not admit"
        );
    }

    /// Protocol 42: the Threads ↔ Code bridges round-trip, refuse fields they
    /// do not know, and the From terminals section has its wire name.
    #[test]
    fn shared_v42_terminal_bridges_round_trip_and_reject_extra_fields() {
        let fixture = fixture_v42();
        assert_eq!(fixture["protocolVersion"], 42);
        let view: ThreadsView = round_trips(&fixture["view"]);
        assert_eq!(view.threads[0].section, ThreadSection::FromTerminals);
        assert_eq!(
            serde_json::to_value(ThreadSection::FromTerminals).unwrap(),
            json!("fromTerminals")
        );
        let response: ThreadControlResponse = round_trips(&fixture["response"]);
        let terminal = response.terminal.as_ref().unwrap();
        assert_eq!(
            terminal.profile_id,
            crate::terminal::TerminalLaunchProfileId::Claude
        );
        rejects_extra::<ThreadControlResponse>(&fixture["response"], "/terminal");
        for value in fixture["commands"].as_array().unwrap() {
            let command: ThreadCommand = round_trips(value);
            assert!(matches!(
                command,
                ThreadCommand::OpenInTerminal { .. } | ThreadCommand::ContinueFromTerminal { .. }
            ));
            rejects_extra::<ThreadCommand>(value, "");
        }
    }

    /// Protocol 45 (review B3): a thread an Away loop runs as carries the
    /// loop's projection: its state, rounds with their verdicts, the thread
    /// turn each work round ran as and each review's own session.
    #[test]
    fn shared_v45_away_thread_round_trips_and_rejects_extra_fields() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-v45.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 45);
        let view: ThreadsView = round_trips(&fixture["view"]);
        let away = view.threads[0].away.as_ref().unwrap();
        assert_eq!(away.state, crate::away::State::Completed);
        assert_eq!(away.rounds[0].turn, Some(0));
        assert_eq!(away.rounds[1].verdict, Some(crate::away::Verdict::Revise));
        assert_eq!(away.rounds[4].state, crate::away::RoundState::Cancelled);
        for pointer in ["/threads/0/away", "/threads/0/away/rounds/1"] {
            rejects_extra::<ThreadsView>(&fixture["view"], pointer);
        }
        let older: ThreadsView = round_trips(&fixture_v42()["view"]);
        assert!(older.threads.iter().all(|thread| thread.away.is_none()));
    }

    /// Protocol 58: a view names the registry sessions Threads hid as
    /// workers, and leaves the field out when there are none.
    #[test]
    fn shared_v58_hidden_keys_round_trip_and_older_views_carry_none() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-hidden-v58.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 58);
        let view: ThreadsView = round_trips(&fixture["view"]);
        assert_eq!(
            view.hidden_keys,
            [
                "codex#0199f2a4-7c1e-7d30-9a5b-1c2d3e4f5a6b",
                "codex#0199f2a4-review"
            ]
        );
        rejects_extra::<ThreadsView>(&fixture["view"], "");
        let older: ThreadsView = round_trips(&fixture_v42()["view"]);
        assert!(older.hidden_keys.is_empty());
        assert!(
            serde_json::to_value(ThreadsView::empty(0))
                .unwrap()
                .get("hiddenKeys")
                .is_none()
        );
    }

    #[test]
    fn shared_v34_commands_round_trip_cover_every_kind_and_reject_extra_fields() {
        let mut kinds = BTreeSet::new();
        for fixture in [
            fixture(),
            fixture_v37(),
            fixture_v42(),
            fixture_v46(),
            serde_json::from_str(include_str!(
                "../../../electron/src/fixtures/threads-commit-v49.json"
            ))
            .unwrap(),
            serde_json::from_str(include_str!(
                "../../../electron/src/fixtures/workflows-v49.json"
            ))
            .unwrap(),
            fixture_v57(),
            fixture_v59(),
            fixture_v62(),
            fixture_v66(),
            fixture_v67(),
            fixture_v72(),
        ] {
            for value in fixture["commands"].as_array().unwrap() {
                let command: ThreadCommand = round_trips(value);
                assert_eq!(value["kind"], command.kind());
                kinds.insert(command.kind());
                rejects_extra::<ThreadCommand>(value, "");
            }
        }
        assert_eq!(kinds, THREAD_COMMAND_KINDS.into_iter().collect());
        assert!(serde_json::from_value::<ThreadCommand>(json!({"kind": "delete"})).is_err());
    }

    #[test]
    fn selected_commit_review_round_trips_and_rejects_extra_fields() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-commit-v49.json"
        ))
        .unwrap();
        let review: CommitReview = round_trips(&fixture["review"]);
        assert!(!review.files[1].attributed);
        for path in ["", "/files/0", "/files/0/hunks/0"] {
            rejects_extra::<CommitReview>(&fixture["review"], path);
        }
        rejects_extra::<ThreadCommand>(&fixture["commands"][1], "/choices/0");
    }

    /// Protocol 37: the ship commands and a response carrying every ship
    /// field round-trip and refuse fields they do not know.
    #[test]
    fn shared_v37_ship_commands_and_response_round_trip_and_reject_extra_fields() {
        let fixture = fixture_v37();
        let response: ThreadControlResponse = round_trips(&fixture["response"]);
        let ship = response.ship.as_ref().unwrap();
        assert_eq!(ship.gh, GhState::SignedOut);
        assert_eq!(ship.checks.as_ref().unwrap().state, ChecksState::Failing);
        assert_eq!(
            response.shipped.as_ref().unwrap().certainty,
            ShipCertainty::Unknown
        );
        assert_eq!(
            response.review_comments.as_ref().unwrap()[0].side,
            DiffSide::New
        );
        for pointer in [
            "",
            "/ship",
            "/ship/files/0",
            "/ship/leftOut/0",
            "/ship/pullRequest",
            "/ship/checks",
            "/ship/checks/failing/0",
            "/shipped",
            "/reviewComments/0",
        ] {
            rejects_extra::<ThreadControlResponse>(&fixture["response"], pointer);
        }
        assert!(
            serde_json::from_value::<ThreadCommand>(json!({
                "kind": "mergePullRequest", "key": "k", "number": 1, "method": "fastForward"
            }))
            .is_err(),
            "an unknown merge method"
        );
    }

    #[test]
    fn ship_enums_use_their_frozen_wire_strings() {
        assert_eq!(
            wire(&[
                GhState::Ready,
                GhState::Missing,
                GhState::SignedOut,
                GhState::Unavailable
            ]),
            ["ready", "missing", "signedOut", "unavailable"]
        );
        assert_eq!(
            wire(&[
                CheckBucket::Pass,
                CheckBucket::Fail,
                CheckBucket::Pending,
                CheckBucket::Skipping,
                CheckBucket::Cancel
            ]),
            ["pass", "fail", "pending", "skipping", "cancel"]
        );
        assert_eq!(
            wire(&[
                ChecksState::Passing,
                ChecksState::Failing,
                ChecksState::Pending,
                ChecksState::None
            ]),
            ["passing", "failing", "pending", "none"]
        );
        assert_eq!(
            wire(&[MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase]),
            ["squash", "merge", "rebase"]
        );
        assert_eq!(wire(&[DiffSide::New, DiffSide::Old]), ["new", "old"]);
        assert_eq!(
            wire(&[
                ShipAction::Commit,
                ShipAction::CreateBranch,
                ShipAction::OpenPullRequest,
                ShipAction::Merge
            ]),
            ["commit", "createBranch", "openPullRequest", "merge"]
        );
        assert_eq!(
            wire(&[ShipCertainty::Done, ShipCertainty::Unknown]),
            ["done", "unknown"]
        );
        assert_eq!(
            wire(&[LeftOutReason::Credential, LeftOutReason::NotThisThread]),
            ["credential", "notThisThread"]
        );
    }

    #[test]
    fn shared_v34_view_and_response_round_trip_and_reject_extra_fields() {
        let fixture = fixture();
        let view: ThreadsView = round_trips(&fixture["view"]);
        assert_eq!(view.version, THREADS_VIEW_VERSION);
        let sections = view
            .threads
            .iter()
            .map(|thread| thread.section)
            .collect::<BTreeSet<_>>();
        assert_eq!(sections.len(), 4, "the fixture covers every section");
        assert!(
            view.threads
                .iter()
                .filter_map(|thread| thread.workspace.as_ref())
                .any(|workspace| workspace.kept == Some(WorkspaceKept::Dirty)),
            "the fixture covers a kept worktree"
        );
        for pointer in [
            "",
            "/settings",
            "/threads/0",
            "/threads/0/lastRun",
            "/threads/0/turns/0",
            "/threads/1/workspace",
            "/threads/1/pullRequest",
            "/threads/2/lineage",
        ] {
            rejects_extra::<ThreadsView>(&fixture["view"], pointer);
        }

        let response: ThreadControlResponse = round_trips(&fixture["response"]);
        assert!(response.prepared.is_some() && response.branches.is_some());
        assert!(response.worktrees.is_some() && response.diff.is_some());
        assert!(response.restore.is_some() && response.key.is_some());
        assert!(response.prompt.is_some());
        for pointer in [
            "",
            "/prepared",
            "/prepared/workspace",
            "/branches",
            "/worktrees/0",
            "/diff",
            "/diff/files/0",
            "/restore",
        ] {
            rejects_extra::<ThreadControlResponse>(&fixture["response"], pointer);
        }
    }

    /// Protocol 36: a finished turn carries when its run ended; a turn from
    /// before it (or still running) has none, and the v34 shapes still decode.
    #[test]
    fn shared_v36_turns_carry_their_end_and_older_turns_stay_without_one() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/threads-v36.json"
        ))
        .unwrap();
        let view: ThreadsView = round_trips(&fixture["view"]);
        let turns = &view.threads[0].turns;
        assert_eq!(
            turns.iter().map(|turn| turn.ended_at).collect::<Vec<_>>(),
            [None, Some(1_791_298_534_000), Some(1_791_301_041_000), None]
        );
        assert_eq!(turns[3].state, TurnState::Running);
        for pointer in ["/threads/0/turns/1", "/threads/0/turns/2"] {
            rejects_extra::<ThreadsView>(&fixture["view"], pointer);
        }
        let older: ThreadsView = round_trips(&self::fixture()["view"]);
        assert!(
            older
                .threads
                .iter()
                .flat_map(|thread| &thread.turns)
                .all(|turn| turn.ended_at.is_none())
        );
    }

    fn wire<T: Serialize>(values: &[T]) -> Vec<String> {
        values
            .iter()
            .map(|value| {
                serde_json::to_value(value)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn inbox_enums_use_their_frozen_wire_strings() {
        use LineageRelation as L;
        use RunOutcome as O;
        use SettledReason as R;
        use ThreadSection as S;
        use TitleSource as Title;
        use TurnState as T;
        use WorkspaceKept as K;
        use WorkspaceMode as M;
        assert_eq!(
            wire(&[S::Pinned, S::Active, S::Snoozed, S::Settled]),
            ["pinned", "active", "snoozed", "settled"]
        );
        assert_eq!(
            wire(&[R::Manual, R::Idle, R::Merged, R::Edited, R::External]),
            ["manual", "idle", "merged", "edited", "external"]
        );
        assert_eq!(
            wire(&[M::Local, M::Worktree, M::None]),
            ["local", "worktree", "none"]
        );
        assert_eq!(
            wire(&[K::Dirty, K::Ignored, K::InUse]),
            ["dirty", "ignored", "inUse"]
        );
        assert_eq!(
            wire(&[O::Completed, O::Failed, O::Interrupted]),
            ["completed", "failed", "interrupted"]
        );
        assert_eq!(
            wire(&[T::Running, T::Done, T::Failed, T::Interrupted]),
            ["running", "done", "failed", "interrupted"]
        );
        assert_eq!(
            wire(&[L::Fork, L::Edit, L::Handoff]),
            ["fork", "edit", "handoff"]
        );
        assert_eq!(
            wire(&[Title::Prompt, Title::Generated, Title::User]),
            ["prompt", "generated", "user"]
        );
    }

    #[test]
    fn pull_request_and_diff_enums_use_their_frozen_wire_strings() {
        use DiffFileStatus as F;
        use DiffOmitted as O;
        use DiffScope as S;
        use PullRequestState as P;
        assert_eq!(
            wire(&[P::Open, P::Merged, P::Closed]),
            ["open", "merged", "closed"]
        );
        assert_eq!(
            wire(&[S::Turn, S::Thread, S::Uncommitted, S::Branch]),
            ["turn", "thread", "uncommitted", "branch"]
        );
        assert_eq!(
            wire(&[F::Added, F::Modified, F::Deleted, F::Renamed]),
            ["added", "modified", "deleted", "renamed"]
        );
        assert_eq!(
            wire(&[O::Sensitive, O::Large, O::Binary]),
            ["sensitive", "large", "binary"]
        );
    }

    #[test]
    fn defaults_and_the_empty_view_have_fixed_bytes() {
        assert_eq!(
            serde_json::to_value(ThreadsView::empty(1_791_300_000_000)).unwrap(),
            json!({
                "version": 1,
                "revision": 0,
                "now": 1_791_300_000_000_u64,
                "settings": {
                    "autoSettleDays": 3,
                    "settleOnMerge": true,
                    "generatedTitles": true,
                    "trackPullRequests": true
                },
                "threads": []
            })
        );
        assert_eq!(
            serde_json::to_value(ThreadControlResponse::from(ThreadsView::empty(5))).unwrap(),
            json!({"view": serde_json::to_value(ThreadsView::empty(5)).unwrap()})
        );
    }
}
