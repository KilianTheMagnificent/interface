//! Team tools (T3 section 2): the rules a Threads agent's `iface_team` tools
//! keep, and `thread-team.v2.json`, the ledger of the threads they started and
//! the receipts that make a retried call answer like the first. Outside agents
//! (T3 section 11, `iface mcp`) keep their rules here too: the level the
//! owner chose (kept in the ledger), what each level may call, their limits,
//! and which project a start may name.
//!
//! Everything here but [`ThreadTeamStore`]'s open and commit is a pure
//! function the host asks before it acts: the permission ladder (a started
//! thread never runs wider than the thread that started it), the start
//! limits, who may read and who may control which thread, the status a
//! thread reports, whether a finished thread's notice is sent, waits or is
//! dropped, and its wording. The host holds the one writer; the file is
//! leased, written atomically, bounded, and quarantined when it does not
//! decode. Design: `docs/superpowers/specs/2026-11-14-t3-systems-design.md`
//! section 2.

use std::path::{Path, PathBuf};

use interface_contracts::{
    thread_journal::{JournalEntry, JournalLine, TurnOrigin},
    threads::{OutsideAgents, RunOutcome},
};
use interface_storage::{JsonLimits, PrivateDirectory, read_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{SettingsError, leased::LeasedDocument};

/// The store's file in the private data root, beside the Threads store.
pub const THREAD_TEAM_FILE_NAME: &str = "thread-team.v2.json";
/// The version 1 store (protocol 57), read once when [`THREAD_TEAM_FILE_NAME`]
/// does not exist yet, and never changed.
pub const THREAD_TEAM_V1_FILE_NAME: &str = "thread-team.v1.json";
/// Started threads one caller may have running (or waiting on the owner) at once.
pub const MAX_RUNNING_PER_CALLER: usize = 4;
/// Threads one caller may start in all.
pub const MAX_STARTED_PER_CALLER: usize = 16;
/// Started threads the ledger keeps across every caller; past it a start is
/// refused until threads are deleted.
pub const MAX_STARTED: usize = 4_096;
/// The newest receipts kept; older ones go.
pub const MAX_RECEIPTS: usize = 1_024;
/// A receipt's result is at most this many bytes.
pub const MAX_RESULT_BYTES: usize = 8 * 1024;
/// `wait_thread`'s answer is cut to this many bytes, with `answerCut`.
pub const ANSWER_LIMIT_BYTES: usize = 32 * 1024;
/// A finished thread's notice quotes this many characters of its answer.
pub const NOTICE_ANSWER_CHARS: usize = 2_000;
/// `read_thread` answers about this many characters a call.
pub const READ_PAGE_CHARS: usize = 30_000;
/// `list_threads` answers at most this many threads.
pub const LIST_LIMIT: usize = 50;
/// `wait_thread`'s default and longest wait, in seconds.
pub const WAIT_DEFAULT_SECS: u64 = 120;
pub const WAIT_MAX_SECS: u64 = 600;
/// A task is at most this many bytes; a message to a thread too.
pub const MAX_TASK_BYTES: usize = 64 * 1024;
/// A title the caller gives is at most this many characters; one made from
/// the task is cut to [`TITLE_FROM_TASK_CHARS`].
pub const MAX_TITLE_CHARS: usize = 120;
pub const TITLE_FROM_TASK_CHARS: usize = 60;
/// A `requestId` is at most this many bytes.
pub const MAX_REQUEST_ID_BYTES: usize = 128;

const LOCK_NAME: &str = ".thread-team.lock";
const BYTES: usize = 4 * 1024 * 1024;
const KEY_CHARS: usize = 512;
const UNCERTAIN: &str =
    "The team tools ledger needs recovery after an uncertain write. Restart Interface to retry.";

// --- Refusals -----------------------------------------------------------------

/// A typed refusal the model reads: `{code, message}`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeamRefusal {
    pub code: &'static str,
    pub message: String,
}

impl TeamRefusal {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The JSON text a tool answers with.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({"code": self.code, "message": self.message})
    }
}

pub const NOT_A_THREAD: &str = "not-a-thread";
pub const NO_NESTING: &str = "no-nesting";
pub const NO_PROJECT: &str = "no-project";
pub const LIMIT_RUNNING: &str = "limit-running";
pub const LIMIT_TOTAL: &str = "limit-total";
pub const TEAM_FULL: &str = "team-full";
pub const NOT_FOUND: &str = "not-found";
pub const OTHER_PROJECT: &str = "other-project";
pub const NOT_STARTED_BY_YOU: &str = "not-started-by-you";
pub const THREAD_BUSY: &str = "thread-busy";
pub const THREAD_IDLE: &str = "thread-idle";
pub const MODE_UNAVAILABLE: &str = "mode-unavailable";
pub const OUTCOME_UNKNOWN: &str = "outcome-unknown";
pub const REQUEST_REUSED: &str = "request-reused";
pub const INVALID_INPUT: &str = "invalid-input";
pub const UNAVAILABLE: &str = "unavailable";
pub const REFUSED: &str = "refused";
/// `show_visual`'s page is over 512 KiB (T3 section 8).
pub const VISUAL_TOO_LARGE: &str = "visual-too-large";
/// `show_visual`'s page is not an HTML document (T3 section 8).
pub const VISUAL_NOT_HTML: &str = "visual-not-html";
/// Outside agents are off (T3 section 11).
pub const OUTSIDE_OFF: &str = "outside-agents-off";
/// The owner let outside agents read only; this tool changes something.
pub const READ_ONLY: &str = "read-only";
/// Outside agents started the most threads one day allows.
pub const LIMIT_DAY: &str = "limit-day";
/// `project` is not the root of a folder an Interface thread works in.
pub const UNKNOWN_PROJECT: &str = "unknown-project";
/// `iface mcp` found no running Interface to ask (answered by `iface`, never
/// by the host).
pub const NOT_RUNNING: &str = "interface-not-running";

// --- The permission ladder ------------------------------------------------------

/// One provider-neutral permission ladder: `plan < ask < edits < full`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Rung {
    /// Reads and plans; changes nothing.
    Plan,
    /// Asks before acting.
    Ask,
    /// Edits files without asking; asks for anything else.
    Edits,
    /// Acts without asking.
    Full,
}

impl Rung {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Ask => "ask",
            Self::Edits => "edits",
            Self::Full => "full",
        }
    }
}

/// Which family of modes an engine id speaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ModeFamily {
    Claude,
    Codex,
    Grok,
    /// Qwen and Custom ACP profiles: only `default` (the agent asks).
    PortableAcp,
    Unknown,
}

fn family(engine: &str) -> ModeFamily {
    match engine {
        "claude" => ModeFamily::Claude,
        "codex" => ModeFamily::Codex,
        "acp:grok" => ModeFamily::Grok,
        other if other.starts_with("acp:") => ModeFamily::PortableAcp,
        _ => ModeFamily::Unknown,
    }
}

/// The rung a caller in `mode` stands on, read conservatively: a mode whose
/// reach lies between two rungs counts as the lower one (Claude's `auto`
/// and Grok's `auto` as `edits`, Claude's `dontAsk` as `plan`). Codex's
/// `never` is `edits`, not `full`: it only says Codex never asks, and what it
/// can reach is its sandbox's to bound, which this ladder cannot see, so a
/// Codex caller never starts an unsandboxed thread on another provider. An
/// unknown or absent mode is `ask`.
#[must_use]
pub fn caller_rung(engine: &str, mode: Option<&str>) -> Rung {
    let Some(mode) = mode else {
        return Rung::Ask;
    };
    match (family(engine), mode) {
        (ModeFamily::Claude, "plan" | "dontAsk") => Rung::Plan,
        (ModeFamily::Claude, "acceptEdits" | "auto")
        | (ModeFamily::Codex, "on-request" | "on-failure" | "never")
        | (ModeFamily::Grok, "auto") => Rung::Edits,
        (ModeFamily::Claude | ModeFamily::Grok, "bypassPermissions") => Rung::Full,
        _ => Rung::Ask,
    }
}

/// The mode a started thread on `engine` runs in at `rung`, or the nearest
/// lower rung `engine` has; `None` when it has none at or below. Only modes
/// whose reach is exactly a rung are ever chosen (never Claude's `auto` or
/// Grok's `auto`, whose reach is the provider's to judge).
#[must_use]
pub fn child_mode(engine: &str, rung: Rung) -> Option<(Rung, &'static str)> {
    let table: &[(Rung, &'static str)] = match family(engine) {
        ModeFamily::Claude => &[
            (Rung::Plan, "plan"),
            (Rung::Ask, "default"),
            (Rung::Edits, "acceptEdits"),
            (Rung::Full, "bypassPermissions"),
        ],
        ModeFamily::Codex => &[
            (Rung::Ask, "untrusted"),
            (Rung::Edits, "on-request"),
            (Rung::Full, "never"),
        ],
        ModeFamily::Grok => &[(Rung::Ask, "default"), (Rung::Full, "bypassPermissions")],
        ModeFamily::PortableAcp => &[(Rung::Ask, "default")],
        ModeFamily::Unknown => &[],
    };
    table
        .iter()
        .rev()
        .find(|(at, _)| *at <= rung)
        .map(|(at, mode)| (*at, *mode))
}

/// The mode a thread on `target` started by a caller on `caller_engine` in
/// `caller_mode` runs in: the caller's rung or the nearest lower one.
///
/// # Errors
/// `mode-unavailable` when `target` has no mode that low.
pub fn started_mode(
    caller_engine: &str,
    caller_mode: Option<&str>,
    target: &str,
) -> Result<(Rung, &'static str), TeamRefusal> {
    let rung = caller_rung(caller_engine, caller_mode);
    child_mode(target, rung).ok_or_else(|| {
        TeamRefusal::new(
            MODE_UNAVAILABLE,
            format!(
                "{target} has no permission mode at or below this thread's ({}), so no thread was started. Start it on another provider.",
                rung.as_str()
            ),
        )
    })
}

// --- Limits and authorization -------------------------------------------------

/// Whether a caller with `running` started threads running (or waiting on
/// the owner) and `total` started in all may start another, with the ledger
/// holding `ledger` threads.
///
/// # Errors
/// The limit that is reached.
pub fn check_start_limits(running: usize, total: usize, ledger: usize) -> Result<(), TeamRefusal> {
    if total >= MAX_STARTED_PER_CALLER {
        return Err(TeamRefusal::new(
            LIMIT_TOTAL,
            format!(
                "This thread has started {MAX_STARTED_PER_CALLER} threads, the most one thread may start. Nothing was started."
            ),
        ));
    }
    if running >= MAX_RUNNING_PER_CALLER {
        return Err(TeamRefusal::new(
            LIMIT_RUNNING,
            format!(
                "{MAX_RUNNING_PER_CALLER} threads this thread started are still running. Wait for one (wait_thread) or stop one (stop_thread), then start another."
            ),
        ));
    }
    if ledger >= MAX_STARTED {
        return Err(TeamRefusal::new(
            TEAM_FULL,
            "Interface keeps track of at most 4096 started threads. Delete threads you no longer need, then try again.",
        ));
    }
    Ok(())
}

/// Read and list: a thread in the caller's own project root. A thread with
/// no project (a plain chat) is nobody's to read.
#[must_use]
pub fn may_read(caller_project: Option<&str>, target_project: Option<&str>) -> bool {
    matches!((caller_project, target_project), (Some(caller), Some(target)) if !caller.is_empty() && caller == target)
}

/// Send and stop: only a thread the caller started.
#[must_use]
pub fn may_control(started_by: Option<&str>, caller: &str) -> bool {
    started_by == Some(caller)
}

// --- Outside agents (T3 section 11) ---------------------------------------------

/// The ledger's caller key for one outside session: the connection the host
/// accepted on its outside socket, named by the host (never by the caller).
pub const OUTSIDE_CALLER_PREFIX: &str = "outside:";
/// The receipts key every outside session shares. A session is a
/// connection, and the one call whose outcome is unknown is the one whose
/// connection was lost, so its retry (same `requestId`, as the refusal asks)
/// always comes from a new session: keyed per session, it would act again.
/// Never a session's key (those are 12 hex digits after the prefix).
pub const OUTSIDE_RECEIPTS_CALLER: &str = "outside:receipts";
/// Threads one outside session may have running (or waiting on the owner).
pub const MAX_OUTSIDE_RUNNING: usize = 4;
/// Threads one outside session may start in all.
pub const MAX_OUTSIDE_STARTED_PER_SESSION: usize = 16;
/// Threads every outside session together may start in any 24 hours.
pub const MAX_OUTSIDE_STARTED_PER_DAY: usize = 64;
/// The window [`MAX_OUTSIDE_STARTED_PER_DAY`] counts over.
pub const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
/// An MCP client's declared name keeps this many characters (display only).
pub const MAX_CLIENT_NAME_CHARS: usize = 64;
/// What an outside agent is called when its client named nothing usable.
pub const UNNAMED_CLIENT: &str = "an outside agent";
/// The tools a `read` level may call.
pub const OUTSIDE_READ_TOOLS: [&str; 5] = [
    "list_projects",
    "list_threads",
    "read_thread",
    "thread_status",
    "wait_thread",
];
/// The tools only the `full` level may call.
pub const OUTSIDE_FULL_TOOLS: [&str; 3] = ["start_thread", "send_to_thread", "stop_thread"];
/// Where the outside socket lives under the data root.
pub const OUTSIDE_SOCKET_DIRECTORY: &str = "team";
pub const OUTSIDE_SOCKET_NAME: &str = "outside.sock";
/// macOS refuses a longer `AF_UNIX` path.
pub const MAX_SOCKET_PATH_BYTES: usize = 103;

/// The ledger key of outside session `id`.
#[must_use]
pub fn outside_caller(id: &str) -> String {
    format!("{OUTSIDE_CALLER_PREFIX}{id}")
}

/// Whether `key` is an outside session rather than a thread.
#[must_use]
pub fn is_outside_caller(key: &str) -> bool {
    key.starts_with(OUTSIDE_CALLER_PREFIX)
}

/// The name an MCP client declared, made safe to show: control characters
/// and runs of whitespace become one space, at most
/// [`MAX_CLIENT_NAME_CHARS`] characters; nothing usable is
/// [`UNNAMED_CLIENT`].
#[must_use]
pub fn client_name(declared: Option<&str>) -> String {
    let cleaned = declared
        .unwrap_or_default()
        .chars()
        .map(|character| {
            if character.is_control() || character.is_whitespace() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let (cut, _) = cut_chars(&collapsed, MAX_CLIENT_NAME_CHARS);
    let cut = cut.trim();
    if cut.is_empty() {
        UNNAMED_CLIENT.to_owned()
    } else {
        cut.to_owned()
    }
}

/// Whether an outside agent at `level` may call `tool`.
///
/// # Errors
/// `outside-agents-off` at Off, `read-only` for a changing tool at Read,
/// `invalid-input` for a tool outside agents do not have.
pub fn outside_may_call(level: OutsideAgents, tool: &str) -> Result<(), TeamRefusal> {
    if level == OutsideAgents::Off {
        return Err(TeamRefusal::new(
            OUTSIDE_OFF,
            "Interface does not let outside agents in. The owner can turn it on in Interface: Settings → Threads → Outside agents.",
        ));
    }
    if OUTSIDE_READ_TOOLS.contains(&tool) {
        return Ok(());
    }
    if !OUTSIDE_FULL_TOOLS.contains(&tool) {
        return Err(TeamRefusal::new(INVALID_INPUT, "Unknown tool."));
    }
    if level == OutsideAgents::Read {
        return Err(TeamRefusal::new(
            READ_ONLY,
            "Interface lets outside agents read threads only; nothing was changed. The owner can allow more in Interface: Settings → Threads → Outside agents → Can start and message threads.",
        ));
    }
    Ok(())
}

/// Whether an outside session with `running` started threads running (or
/// waiting on the owner) and `session_total` started in all may start
/// another, with every outside session together having started `day_total`
/// in the last 24 hours and the ledger holding `ledger` threads.
///
/// # Errors
/// The limit that is reached.
pub fn check_outside_start_limits(
    running: usize,
    session_total: usize,
    day_total: usize,
    ledger: usize,
) -> Result<(), TeamRefusal> {
    if day_total >= MAX_OUTSIDE_STARTED_PER_DAY {
        return Err(TeamRefusal::new(
            LIMIT_DAY,
            format!(
                "Outside agents have started {MAX_OUTSIDE_STARTED_PER_DAY} threads in the last 24 hours, the most Interface allows. Nothing was started."
            ),
        ));
    }
    if session_total >= MAX_OUTSIDE_STARTED_PER_SESSION {
        return Err(TeamRefusal::new(
            LIMIT_TOTAL,
            format!(
                "This session has started {MAX_OUTSIDE_STARTED_PER_SESSION} threads, the most one outside session may start. Nothing was started."
            ),
        ));
    }
    if running >= MAX_OUTSIDE_RUNNING {
        return Err(TeamRefusal::new(
            LIMIT_RUNNING,
            format!(
                "{MAX_OUTSIDE_RUNNING} threads this session started are still running. Wait for one (wait_thread) or stop one (stop_thread), then start another."
            ),
        ));
    }
    if ledger >= MAX_STARTED {
        return Err(TeamRefusal::new(
            TEAM_FULL,
            "Interface keeps track of at most 4096 started threads. The owner can delete threads they no longer need.",
        ));
    }
    Ok(())
}

/// A project's name as outside agents see it: its folder's last component.
#[must_use]
pub fn project_name(root: &str) -> String {
    Path::new(root).file_name().map_or_else(
        || root.to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The project root `wanted` names among `known` (the roots Interface
/// threads work in): its exact root, or a name only one of them has.
/// Nothing else: an outside agent never starts work in a folder of its own
/// choosing.
///
/// # Errors
/// `unknown-project`, naming the projects there are.
pub fn resolve_project(known: &[String], wanted: &str) -> Result<String, TeamRefusal> {
    if let Some(root) = known.iter().find(|root| root.as_str() == wanted) {
        return Ok(root.clone());
    }
    let named = known
        .iter()
        .filter(|root| project_name(root) == wanted)
        .collect::<Vec<_>>();
    if let [only] = named.as_slice() {
        return Ok((*only).clone());
    }
    let mut every = known
        .iter()
        .map(|root| project_name(root))
        .collect::<Vec<_>>();
    every.sort_unstable();
    every.dedup();
    let message = if named.len() > 1 {
        format!(
            "Several Interface projects are named {wanted}; give the project's full path from list_projects."
        )
    } else if every.is_empty() {
        "Interface has no projects yet: a project is a folder an Interface thread works in. Nothing was started.".to_owned()
    } else {
        format!(
            "{wanted} is not an Interface project (a folder an Interface thread works in). Projects: {}. Nothing was started.",
            every.join(", ")
        )
    };
    Err(TeamRefusal::new(UNKNOWN_PROJECT, message))
}

/// Where the outside socket of the Interface whose data root is `data_root`
/// lives: `<data root>/team/outside.sock`, or, when that path is too long to
/// bind, `/private/tmp/iface-out-<uid>-<8 hex of the data root>/s.sock`. The
/// host and `iface mcp` both ask this, so neither records the choice.
#[must_use]
pub fn outside_socket_path(data_root: &Path, uid: u32) -> PathBuf {
    let path = data_root
        .join(OUTSIDE_SOCKET_DIRECTORY)
        .join(OUTSIDE_SOCKET_NAME);
    if path.as_os_str().len() <= MAX_SOCKET_PATH_BYTES {
        return path;
    }
    let digest = Sha256::digest(data_root.as_os_str().as_encoded_bytes());
    PathBuf::from("/private/tmp")
        .join(format!("iface-out-{uid}-{}", hex(&digest[..4])))
        .join("s.sock")
}

// --- Status ---------------------------------------------------------------------

/// What a thread is doing, as `wait_thread` and `thread_status` say.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TeamStatus {
    Running,
    NeedsYou,
    Done,
    Failed,
    Stopped,
}

impl TeamStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::NeedsYou => "needsYou",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    /// Whether a wait ends here: the run is over or the owner is needed.
    #[must_use]
    pub const fn settles_wait(self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// What a thread needs from the owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Needs {
    Approval,
    Question,
}

impl Needs {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
        }
    }
}

/// What the host knows about a thread's run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatusFacts {
    /// The run tap holds an open run, or the record's last run has no end.
    pub run_open: bool,
    /// The journal's newest turn has not ended yet (its writer lags the tap).
    pub journal_open: bool,
    /// The run stopped on an approval or a question.
    pub waiting: Option<Needs>,
    /// How the record's last run ended.
    pub outcome: Option<RunOutcome>,
}

/// The thread's status: waiting on the owner while its run stopped on one,
/// running while the run (or the journal's record of it) is open, else how
/// its last run ended. A thread that never ran is done.
#[must_use]
pub fn status(facts: StatusFacts) -> (TeamStatus, Option<Needs>) {
    if facts.run_open
        && let Some(needs) = facts.waiting
    {
        return (TeamStatus::NeedsYou, Some(needs));
    }
    if facts.run_open || facts.journal_open {
        return (TeamStatus::Running, None);
    }
    let status = match facts.outcome {
        Some(RunOutcome::Failed) => TeamStatus::Failed,
        Some(RunOutcome::Interrupted) => TeamStatus::Stopped,
        Some(RunOutcome::Completed) | None => TeamStatus::Done,
    };
    (status, None)
}

/// `text` cut to at most `limit` bytes at a character boundary, and whether it was cut.
#[must_use]
pub fn cut_bytes(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// `text`'s first `limit` characters, and whether it was cut.
#[must_use]
pub fn cut_chars(text: &str, limit: usize) -> (&str, bool) {
    match text.char_indices().nth(limit) {
        Some((end, _)) => (&text[..end], true),
        None => (text, false),
    }
}

/// The title a started thread gets when the caller names none: the task's
/// first non-empty line, whitespace collapsed, cut to
/// [`TITLE_FROM_TASK_CHARS`] characters with "…".
#[must_use]
pub fn title_from_task(task: &str) -> String {
    let line = task
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Started thread");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    match cut_chars(&collapsed, TITLE_FROM_TASK_CHARS) {
        (cut, true) => format!("{}…", cut.trim_end()),
        (whole, false) => whole.to_owned(),
    }
}

// --- Notices --------------------------------------------------------------------

/// What the host knows when a started thread's run ended.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct NoticeFacts {
    /// The started thread asked for notices (`notify`, default on).
    pub notify: bool,
    /// How its run ended.
    pub outcome: Option<RunOutcome>,
    /// The caller already has this result: it read it through `wait_thread`
    /// or `thread_status`, or a notice for it was already attempted.
    pub handled: bool,
    /// The caller's thread is settled, its session archived, an Away loop
    /// holds it, or a Code pane drives it.
    pub caller_settled: bool,
    pub caller_archived: bool,
    pub caller_away: bool,
    pub caller_pane: bool,
    /// The caller is mid-turn.
    pub caller_running: bool,
}

/// What becomes of a finished thread's notice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeDecision {
    /// Nothing to send: notices are off, the run was stopped, or the caller has it.
    None,
    /// Send it now, once.
    Send,
    /// Wait for the caller's turn to end.
    Wait,
    /// Drop it: the caller is settled, archived or held.
    Drop,
}

/// Whether a finished thread's notice is sent now, waits for the caller's
/// turn to end, or is dropped. Only a completed or failed run makes one.
#[must_use]
pub fn notice_decision(facts: NoticeFacts) -> NoticeDecision {
    if !facts.notify
        || facts.handled
        || !matches!(
            facts.outcome,
            Some(RunOutcome::Completed | RunOutcome::Failed)
        )
    {
        return NoticeDecision::None;
    }
    if facts.caller_settled || facts.caller_archived || facts.caller_away || facts.caller_pane {
        return NoticeDecision::Drop;
    }
    if facts.caller_running {
        return NoticeDecision::Wait;
    }
    NoticeDecision::Send
}

/// The one message a caller is sent when a thread it started finished or
/// failed. A finished thread's answer is quoted to
/// [`NOTICE_ANSWER_CHARS`] characters; when that cut it, "…" and a pointer
/// to `read_thread` follow.
#[must_use]
pub fn notice_text(
    title: &str,
    provider: &str,
    outcome: RunOutcome,
    answer: Option<&str>,
    error: Option<&str>,
) -> String {
    let head = format!("[Interface] The thread \"{title}\" ({provider}) you started");
    if outcome == RunOutcome::Failed {
        let error = error
            .map(str::trim)
            .filter(|error| !error.is_empty())
            .unwrap_or("its run ended with an error");
        let (error, cut) = cut_chars(error, NOTICE_ANSWER_CHARS);
        let tail = if cut {
            "… Call read_thread for the rest."
        } else {
            ""
        };
        return format!("{head} failed: {error}{tail}");
    }
    let answer = answer
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
        .unwrap_or("(no answer)");
    match cut_chars(answer, NOTICE_ANSWER_CHARS) {
        (cut, true) => format!("{head} finished: {cut}… Call read_thread for the rest."),
        (whole, false) => format!("{head} finished: {whole}"),
    }
}

// --- Reading a thread -----------------------------------------------------------

/// A prompt or an answer keeps this many characters in `read_thread`.
pub const READ_TEXT_CHARS: usize = 8_000;
/// Any other item keeps this many, on one line.
pub const READ_LINE_CHARS: usize = 400;

fn one_line(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match cut_chars(&collapsed, READ_LINE_CHARS) {
        (cut, true) => format!("{cut}…"),
        (whole, false) => whole.to_owned(),
    }
}

fn kept(text: &str) -> String {
    match cut_chars(text, READ_TEXT_CHARS) {
        (cut, true) => format!("{cut}…"),
        (whole, false) => whole.to_owned(),
    }
}

fn line_count(text: Option<&str>) -> usize {
    text.map_or(0, |text| text.lines().count())
}

/// One journal line as `read_thread` gives it: `{seq, turn, kind, text}`.
/// Prompts and answers keep their text (to [`READ_TEXT_CHARS`]); tool
/// calls, results, approvals and the rest are one line each; a diff is its
/// path and line counts.
#[must_use]
#[allow(clippy::too_many_lines)] // One arm per journal entry kind.
pub fn compact_line(line: &JournalLine) -> Value {
    let agent = |agent_key: &Option<String>| {
        agent_key
            .as_deref()
            .map_or_else(String::new, |agent| format!("[{agent}] "))
    };
    let (kind, text) = match &line.entry {
        JournalEntry::TurnStarted { prompt, origin, .. } => (
            "prompt",
            match origin {
                TurnOrigin::Host => format!("(from Interface) {}", kept(prompt)),
                _ => kept(prompt),
            },
        ),
        JournalEntry::TurnRetracted {} => {
            ("retracted", "the prompt never reached the provider".into())
        }
        JournalEntry::Message {
            text, agent_key, ..
        } => match agent_key {
            None => ("answer", kept(text)),
            Some(_) => ("message", one_line(&format!("{}{text}", agent(agent_key)))),
        },
        JournalEntry::ToolCall {
            tool,
            summary,
            agent_key,
            ..
        } => (
            "toolCall",
            one_line(&format!("{}{tool}: {summary}", agent(agent_key))),
        ),
        JournalEntry::ToolResult {
            ok,
            summary,
            agent_key,
            ..
        } => (
            "toolResult",
            one_line(&format!(
                "{}{}: {summary}",
                agent(agent_key),
                if *ok { "ok" } else { "failed" }
            )),
        ),
        JournalEntry::ApprovalRequested { tool, summary, .. } => {
            ("approval", one_line(&format!("{tool}: {summary}")))
        }
        JournalEntry::Diff {
            path,
            before,
            after,
            ..
        } => (
            "diff",
            one_line(&format!(
                "{path} ({} → {} lines)",
                line_count(before.as_deref()),
                line_count(after.as_deref())
            )),
        ),
        JournalEntry::Plan { entries } => (
            "plan",
            one_line(
                &entries
                    .iter()
                    .map(|entry| entry.content.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
        ),
        JournalEntry::SubagentStarted {
            agent_key,
            description,
            ..
        } => (
            "subagent",
            one_line(&format!("[{agent_key}] started: {description}")),
        ),
        JournalEntry::SubagentFinished {
            agent_key,
            ok,
            summary,
        } => (
            "subagent",
            one_line(&format!(
                "[{agent_key}] {}: {summary}",
                if *ok { "finished" } else { "failed" }
            )),
        ),
        JournalEntry::PeerResult {
            source_engine,
            text,
            ..
        } => ("peerResult", one_line(&format!("{source_engine}: {text}"))),
        JournalEntry::Notice { message, .. } => ("notice", one_line(message)),
        JournalEntry::Error { message, .. } => ("error", one_line(message)),
        JournalEntry::Compact { .. } => ("compact", "the conversation was compacted".into()),
        JournalEntry::Visual { title, .. } => {
            ("visual", one_line(&format!("showed a visual: {title}")))
        }
        JournalEntry::TurnEnded { outcome, .. } => (
            "turnEnded",
            match outcome {
                RunOutcome::Completed => "completed",
                RunOutcome::Failed => "failed",
                RunOutcome::Interrupted => "stopped",
            }
            .into(),
        ),
        JournalEntry::SessionClosed {} => ("sessionClosed", "the session closed".into()),
    };
    let mut item = json!({"seq": line.seq, "kind": kind, "text": text});
    if let Some(turn) = line.turn {
        item["turn"] = turn.into();
    }
    item
}

/// A page of `read_thread` items within about [`READ_PAGE_CHARS`] of text
/// (always at least one), and the `next` to pass as `after` when more
/// follow: the last item's seq.
#[must_use]
pub fn compact_page(lines: &[JournalLine], has_more: bool) -> (Vec<Value>, Option<u64>) {
    let mut items = Vec::new();
    let mut used = 0;
    let mut cut = false;
    for line in lines {
        let item = compact_line(line);
        let size = item["text"].as_str().map_or(0, |text| text.chars().count());
        if !items.is_empty() && used + size > READ_PAGE_CHARS {
            cut = true;
            break;
        }
        used += size;
        items.push(item);
    }
    let next = (cut || has_more)
        .then(|| items.last().and_then(|item| item["seq"].as_u64()))
        .flatten();
    (items, next)
}

// --- Receipts -------------------------------------------------------------------

/// What a call with a `requestId` does about an earlier call with the same one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptDecision {
    /// No earlier call: go ahead (and reserve the receipt first).
    Fresh,
    /// The earlier call finished: answer its result again.
    Replay(String),
    /// The earlier call's outcome is unknown: never act again.
    Unknown,
    /// The id was used for another call.
    Reused,
}

/// The digest a receipt keeps of a call's tool and input.
#[must_use]
pub fn call_digest(tool: &str, input: &Value) -> String {
    let mut hash = Sha256::new();
    hash.update(tool.as_bytes());
    hash.update([0]);
    hash.update(input.to_string().as_bytes());
    hex(&hash.finalize()[..16])
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// [`ReceiptDecision`] for a call whose digest is `digest`, given the receipt
/// its caller and `requestId` already have.
#[must_use]
pub fn receipt_decision(existing: Option<&Receipt>, digest: &str) -> ReceiptDecision {
    match existing {
        None => ReceiptDecision::Fresh,
        Some(receipt) if receipt.digest != digest => ReceiptDecision::Reused,
        Some(Receipt {
            result: Some(result),
            ..
        }) => ReceiptDecision::Replay(result.clone()),
        Some(_) => ReceiptDecision::Unknown,
    }
}

// --- The store ------------------------------------------------------------------

/// How a started thread's newest result reached its caller.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryHow {
    /// The caller read it through `wait_thread` or `thread_status`.
    Read,
    /// A notice was sent and the provider took it.
    Sent,
    /// A notice was sent and never reached the provider.
    Unsent,
    /// A notice was sent and its outcome is unknown (also what a notice is
    /// recorded as before it goes out); never sent again.
    Unknown,
    /// The caller was settled, archived or held, so no notice was sent.
    Dropped,
}

/// The newest result of a started thread its caller has, by journal turn.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Delivered {
    pub turn: u32,
    pub how: DeliveryHow,
}

/// One thread team tools started.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartedThread {
    /// The thread that started it, or an outside session
    /// ([`OUTSIDE_CALLER_PREFIX`]).
    pub parent: String,
    pub child: String,
    pub started_at: u64,
    pub notify: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<Delivered>,
    /// Version 2: an agent outside Interface started it (T3 section 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<StartedBy>,
}

/// Who outside Interface started a thread: display only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartedBy {
    pub kind: StartedByKind,
    /// The name its MCP client declared, as [`client_name`] made it.
    pub client: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StartedByKind {
    Outside,
}

/// One call with a `requestId`: reserved before its effect (no result:
/// outcome unknown), settled with its result after.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub caller: String,
    pub request_id: String,
    pub tool: String,
    pub digest: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// The current version of the ledger's document.
const VERSION: u8 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    /// Version 2: what outside agents may do (T3 section 11); absent is off.
    #[serde(default, skip_serializing_if = "OutsideAgents::is_off")]
    outside_agents: OutsideAgents,
    started: Vec<StartedThread>,
    receipts: Vec<Receipt>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: VERSION,
            outside_agents: OutsideAgents::Off,
            started: Vec::new(),
            receipts: Vec::new(),
        }
    }
}

fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= KEY_CHARS && !key.contains('\0')
}

fn valid_state(state: &State) -> bool {
    state.version == VERSION && valid_entries(state)
}

/// A version 1 document (protocol 57): the same bounds, nothing outside.
fn valid_v1_state(state: &State) -> bool {
    state.version == 1
        && state.outside_agents == OutsideAgents::Off
        && state
            .started
            .iter()
            .all(|started| started.started_by.is_none())
        && valid_entries(state)
}

/// A thread an outside session started says so, and only such a thread.
fn started_by_ok(started: &StartedThread) -> bool {
    match &started.started_by {
        None => !is_outside_caller(&started.parent),
        Some(by) => {
            is_outside_caller(&started.parent)
                && !by.client.trim().is_empty()
                && by.client.chars().count() <= MAX_CLIENT_NAME_CHARS
                && !by.client.chars().any(char::is_control)
        }
    }
}

fn valid_entries(state: &State) -> bool {
    let mut children = std::collections::BTreeSet::new();
    let mut requests = std::collections::BTreeSet::new();
    state.started.len() <= MAX_STARTED
        && state.receipts.len() <= MAX_RECEIPTS
        && state.started.iter().all(|started| {
            key_ok(&started.parent)
                && key_ok(&started.child)
                && started.parent != started.child
                && !is_outside_caller(&started.child)
                && started_by_ok(started)
                && children.insert(started.child.as_str())
        })
        && state.receipts.iter().all(|receipt| {
            key_ok(&receipt.caller)
                && !receipt.request_id.is_empty()
                && receipt.request_id.len() <= MAX_REQUEST_ID_BYTES
                && receipt.tool.len() <= 64
                && receipt.digest.len() <= 64
                && receipt
                    .result
                    .as_ref()
                    .is_none_or(|result| result.len() <= MAX_RESULT_BYTES)
                && requests.insert((receipt.caller.as_str(), receipt.request_id.as_str()))
        })
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(
        BYTES,
        8,
        64 * (MAX_STARTED + MAX_RECEIPTS),
    )?)
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

/// The leased single writer of `thread-team.v2.json`.
pub struct ThreadTeamStore {
    document: LeasedDocument,
    state: State,
    quarantined: Option<PathBuf>,
}

impl ThreadTeamStore {
    /// Opens (or creates) the store at `file`. A document that does not
    /// decode or breaks a bound is quarantined and the store starts empty.
    ///
    /// When `file` does not exist, the version 1 store beside it
    /// ([`THREAD_TEAM_V1_FILE_NAME`]) is read once, never changed, moved or
    /// quarantined, and carried over as version 2 (outside agents off); one
    /// that cannot be read or is invalid is left for the build that wrote it.
    ///
    /// # Errors
    /// Refuses unsafe storage, a lease held by another writer, or a file over
    /// the byte bound.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let limits = limits()?;
        let (mut document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            file,
            LOCK_NAME,
            limits,
            UNCERTAIN,
            valid_state,
        )?;
        let older = (state.is_none() && quarantined.is_none())
            .then(|| read_v1(&file.with_file_name(THREAD_TEAM_V1_FILE_NAME), limits))
            .flatten();
        if let Some(older) = &older {
            // Written at once, so the version 1 file is never read again.
            document.commit(older)?;
        }
        Ok(Self {
            document,
            state: state.or(older).unwrap_or_default(),
            quarantined,
        })
    }

    /// What outside agents may do (T3 section 11).
    #[must_use]
    pub const fn outside_agents(&self) -> OutsideAgents {
        self.state.outside_agents
    }

    /// Records what outside agents may do.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn set_outside_agents(&mut self, level: OutsideAgents) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        state.outside_agents = level;
        self.commit(state)
    }

    /// Every thread an outside agent started, with its client's name.
    pub fn outside_started(&self) -> impl Iterator<Item = (&str, &str)> {
        self.state.started.iter().filter_map(|entry| {
            entry
                .started_by
                .as_ref()
                .map(|by| (entry.child.as_str(), by.client.as_str()))
        })
    }

    /// How many threads outside agents started at or after `since` (ms).
    #[must_use]
    pub fn outside_started_since(&self, since: u64) -> usize {
        self.state
            .started
            .iter()
            .filter(|entry| entry.started_by.is_some() && entry.started_at >= since)
            .count()
    }

    /// Where an unreadable file found at open was set aside.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// Who started `child`, when team tools did.
    #[must_use]
    pub fn started(&self, child: &str) -> Option<&StartedThread> {
        self.state.started.iter().find(|entry| entry.child == child)
    }

    /// Every thread team tools started.
    pub fn started_children(&self) -> impl Iterator<Item = &str> {
        self.state.started.iter().map(|entry| entry.child.as_str())
    }

    /// The threads `parent` started, oldest first.
    #[must_use]
    pub fn children(&self, parent: &str) -> Vec<&StartedThread> {
        self.state
            .started
            .iter()
            .filter(|entry| entry.parent == parent)
            .collect()
    }

    /// How many started threads the ledger holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.started.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.started.is_empty()
    }

    /// Records a thread team tools started.
    ///
    /// # Errors
    /// Refuses an invalid or repeated child, a full ledger, and failed or
    /// uncertain storage.
    pub fn record_start(&mut self, entry: StartedThread) -> Result<(), SettingsError> {
        if !key_ok(&entry.parent) || !key_ok(&entry.child) || entry.parent == entry.child {
            return Err(invalid("Invalid started thread."));
        }
        if self.started(&entry.child).is_some() {
            return Err(invalid("That thread is already recorded as started."));
        }
        if self.state.started.len() >= MAX_STARTED {
            return Err(invalid("The started-thread ledger is full."));
        }
        let mut state = self.state.clone();
        state.started.push(entry);
        self.commit(state)
    }

    /// Records how `child`'s result of `turn` reached its caller. An older
    /// turn than the one recorded changes nothing.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn mark_delivered(
        &mut self,
        child: &str,
        turn: u32,
        how: DeliveryHow,
    ) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        let Some(entry) = state.started.iter_mut().find(|entry| entry.child == child) else {
            return Ok(());
        };
        if entry
            .delivered
            .is_some_and(|delivered| delivered.turn > turn)
        {
            return Ok(());
        }
        entry.delivered = Some(Delivered { turn, how });
        self.commit(state)
    }

    /// Forgets a deleted or forgotten session: the threads it started lose
    /// their caller's control (they stay threads), and its own entry goes.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn forget(&mut self, key: &str) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        state
            .started
            .retain(|entry| entry.child != key && entry.parent != key);
        state.receipts.retain(|receipt| receipt.caller != key);
        self.commit(state)
    }

    /// A switched thread (T3 section 5) runs on `to` now instead of `from`:
    /// the ledger follows its head. Its entry as a started thread names `to`
    /// with turn `next_turn` (the handoff's: `to`'s journal counts its own
    /// turns) and earlier marked as handled, so its caller hears of the turns
    /// after the handoff; the threads it started name `to` as their caller,
    /// and its receipts move with it.
    ///
    /// # Errors
    /// Refuses an invalid key or one the ledger already holds as started,
    /// and failed or uncertain storage.
    pub fn follow(&mut self, from: &str, to: &str, next_turn: u32) -> Result<(), SettingsError> {
        if !key_ok(to) || from == to {
            return Err(invalid("Invalid thread to follow."));
        }
        if self.started(to).is_some() && self.started(from).is_some() {
            return Err(invalid("That thread is already recorded as started."));
        }
        let mut state = self.state.clone();
        for entry in &mut state.started {
            if entry.child == from {
                to.clone_into(&mut entry.child);
                // Nothing of `to`'s up to its handoff is news to the caller.
                entry.delivered = Some(Delivered {
                    turn: next_turn,
                    how: DeliveryHow::Dropped,
                });
            }
            if entry.parent == from {
                to.clone_into(&mut entry.parent);
            }
        }
        for receipt in &mut state.receipts {
            if receipt.caller == from {
                to.clone_into(&mut receipt.caller);
            }
        }
        self.commit(state)
    }

    /// The receipt `caller` holds for `request_id`.
    #[must_use]
    pub fn receipt(&self, caller: &str, request_id: &str) -> Option<&Receipt> {
        self.state
            .receipts
            .iter()
            .find(|receipt| receipt.caller == caller && receipt.request_id == request_id)
    }

    /// Reserves a receipt before its call's effect: until it is settled, a
    /// retry answers that the outcome is unknown. Past [`MAX_RECEIPTS`] the
    /// oldest go.
    ///
    /// # Errors
    /// Refuses an invalid or already used id, and failed or uncertain storage.
    pub fn reserve(
        &mut self,
        caller: &str,
        request_id: &str,
        tool: &str,
        digest: &str,
        now: u64,
    ) -> Result<(), SettingsError> {
        if !key_ok(caller) || request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES {
            return Err(invalid("Invalid requestId."));
        }
        if self.receipt(caller, request_id).is_some() {
            return Err(invalid("That requestId was already used."));
        }
        let mut state = self.state.clone();
        state.receipts.push(Receipt {
            caller: caller.to_owned(),
            request_id: request_id.to_owned(),
            tool: tool.chars().take(64).collect(),
            digest: digest.to_owned(),
            at: now,
            result: None,
        });
        while state.receipts.len() > MAX_RECEIPTS {
            let oldest = state
                .receipts
                .iter()
                .enumerate()
                .min_by_key(|(_, receipt)| receipt.at)
                .map(|(index, _)| index)
                .unwrap_or_default();
            state.receipts.remove(oldest);
        }
        self.commit(state)
    }

    /// Settles a reserved receipt with its call's result (cut to
    /// [`MAX_RESULT_BYTES`]; a result that does not fit stays unknown).
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn settle(
        &mut self,
        caller: &str,
        request_id: &str,
        result: &str,
    ) -> Result<(), SettingsError> {
        if result.len() > MAX_RESULT_BYTES {
            return Ok(());
        }
        let mut state = self.state.clone();
        if let Some(receipt) = state
            .receipts
            .iter_mut()
            .find(|receipt| receipt.caller == caller && receipt.request_id == request_id)
        {
            receipt.result = Some(result.to_owned());
        }
        self.commit(state)
    }

    fn commit(&mut self, state: State) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if state == self.state {
            return Ok(());
        }
        if !valid_state(&state) {
            return Err(invalid("That change would break the ledger's bounds."));
        }
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
}

/// The version 1 store at `file`, carried over as version 2; `None` when it
/// is absent, unreadable or invalid. Read-only: the file and its directory are
/// never hardened, rewritten, moved or quarantined.
fn read_v1(file: &Path, limits: JsonLimits) -> Option<State> {
    let directory = PrivateDirectory::open(file.parent()?).ok()?;
    let name = file.file_name()?.to_str()?;
    let mut state = read_json::<State>(&directory, name, limits)
        .ok()??
        .into_parts()
        .0;
    if !valid_v1_state(&state) {
        return None;
    }
    state.version = VERSION;
    Some(state)
}

#[cfg(test)]
#[path = "thread_team_tests.rs"]
mod tests;
