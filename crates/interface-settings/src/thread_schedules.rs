//! Scheduled prompts and pull-request watch (T3 section 7): the rules a
//! scheduled prompt keeps, and `thread-schedules.v2.json`, where each
//! thread's schedules and its watch live.
//!
//! A scheduled prompt is a provider effect the owner is not watching, so
//! the rules are strict: each firing has an idempotent occurrence id
//! (`scheduled:{id}:{occurrence}`, the occurrence being its due time), is
//! recorded before its send and is delivered at most once, ever; a firing
//! whose outcome is unknown is never sent again. Everything here but
//! [`ThreadScheduleStore`]'s open and commit is a pure function the host
//! asks before it acts: when a schedule is due (never a burst after a long
//! sleep), whether a late firing still runs, whether the thread may take it
//! now (the delivery gate), what a pull request's checks and review threads
//! changed since the last look, and every prompt's wording. The host holds
//! the one writer; the file is leased, written atomically, bounded and
//! quarantined when it does not decode. Design:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 7.
//!
//! Resume at reset (section 10) is a one-time schedule of kind `resume`,
//! one per thread, due a minute after its provider's usage limit resets;
//! it goes through the same gate, occurrence ids and one-attempt rule. The
//! store is `thread-schedules.v2.json` since then (the kind is new on
//! disk); a `thread-schedules.v1.json` is read once and carried over.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use interface_contracts::{
    agent_work::{AgentCalendarSchedule, AgentMissedRunPolicy},
    engine::UsageWindow,
    recovery::{PromptDelivery, PromptRecoveryReceipt},
    threads::{
        ReviewComment, ScheduleCalendar, ScheduleKind, ScheduleOutcome, ScheduleRun, ScheduleSetBy,
        ScheduleWhen, ThreadScheduleView, ThreadSchedules, ThreadWatchView,
    },
};
use interface_storage::{JsonLimits, PrivateDirectory, read_json};
use serde::{Deserialize, Serialize};

use crate::{SettingsError, agent_calendar::calendar_occurrences, leased::LeasedDocument};

/// The store's file in the private data root, beside the Threads store
/// (version 2 since resume at reset, T3 section 10).
pub const THREAD_SCHEDULES_FILE_NAME: &str = "thread-schedules.v2.json";
/// The version 1 store (protocol 62), read once and carried over.
pub const THREAD_SCHEDULES_V1_FILE_NAME: &str = "thread-schedules.v1.json";
/// The store's document version.
const STORE_VERSION: u8 = 2;
/// Schedules kept across every thread.
pub const MAX_SCHEDULES: usize = 256;
/// Pull-request watches kept across every thread.
pub const MAX_WATCHES: usize = 64;
/// A scheduled prompt is at most this many bytes.
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;
/// A schedule's name is at most this many characters.
pub const MAX_NAME_CHARS: usize = 80;
/// The shortest interval, in minutes.
pub const MIN_INTERVAL_MINUTES: u32 = 5;
/// The longest interval: 30 days, in minutes.
pub const MAX_INTERVAL_MINUTES: u32 = 30 * 24 * 60;
/// A firing more late than this (the app was closed or the Mac asleep) is
/// skipped and recorded instead of sent.
pub const LATE_LIMIT_MS: u64 = 24 * 60 * 60 * 1000;
/// The failing checks and review threads a watch remembers.
pub const MAX_FINGERPRINT_IDS: usize = 200;
/// A label made from a prompt's first words is cut to this many characters.
pub const LABEL_CHARS: usize = 40;
/// The first line every scheduled prompt starts with, up to its label.
pub const SCHEDULED_PREFIX: &str = "[Interface] Scheduled prompt \"";
/// The first line every pull-request watch prompt starts with.
pub const WATCH_PREFIX: &str = "[Interface] Watching pull request #";

/// The schedule input was refused (definitely nothing changed).
pub const SCHEDULE_INVALID_CODE: &str = "threads-schedule-invalid";
/// No schedule has that id (or name).
pub const SCHEDULE_UNKNOWN_CODE: &str = "threads-schedule-unknown";
/// The store holds as many schedules or watches as it keeps.
pub const SCHEDULE_FULL_CODE: &str = "threads-schedule-full";
/// A run of that schedule is already waiting for its thread.
pub const SCHEDULE_WAITING_CODE: &str = "threads-schedule-waiting";
/// The schedule is turned off.
pub const SCHEDULE_OFF_CODE: &str = "threads-schedule-off";

const LOCK_NAME: &str = ".thread-schedules.lock";
const BYTES: usize = 8 * 1024 * 1024;
const KEY_CHARS: usize = 512;
const ID_CHARS: usize = 64;
const DETAIL_CHARS: usize = 400;
const UNCERTAIN: &str =
    "The scheduled prompts need recovery after an uncertain write. Restart Interface to retry.";

// --- Refusals -----------------------------------------------------------------

/// A typed refusal: nothing changed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleRefusal {
    pub code: &'static str,
    pub message: String,
}

impl ScheduleRefusal {
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(SCHEDULE_INVALID_CODE, message)
    }

    fn unknown() -> Self {
        Self::new(
            SCHEDULE_UNKNOWN_CODE,
            "That scheduled prompt no longer exists.",
        )
    }
}

/// Why a store change did not happen.
#[derive(Debug)]
pub enum ScheduleError {
    Refused(ScheduleRefusal),
    Storage(SettingsError),
}

impl From<ScheduleRefusal> for ScheduleError {
    fn from(refusal: ScheduleRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<SettingsError> for ScheduleError {
    fn from(error: SettingsError) -> Self {
        Self::Storage(error)
    }
}

impl std::fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => formatter.write_str(&refusal.message),
            Self::Storage(error) => write!(formatter, "{error}"),
        }
    }
}

// --- When a schedule is due ----------------------------------------------------

fn agent_calendar(calendar: &ScheduleCalendar) -> AgentCalendarSchedule {
    AgentCalendarSchedule {
        timezone: calendar.timezone.clone(),
        weekdays: calendar.weekdays.clone(),
        hour: calendar.hour,
        minute: calendar.minute,
        missed: AgentMissedRunPolicy::Skip,
        pause_on_failure: false,
    }
}

/// Up to `count` calendar dates strictly after `after`.
fn calendar_after(calendar: &ScheduleCalendar, after: u64, count: usize) -> Vec<u64> {
    calendar_occurrences(&agent_calendar(calendar), after, count).unwrap_or_default()
}

fn minutes_ms(minutes: u32) -> u64 {
    u64::from(minutes) * 60_000
}

/// Checks `when`, `prompt` and `name`, and answers the first due time from
/// `now`.
///
/// # Errors
/// An empty or oversized prompt, a name that is too long or has control
/// characters, a time not in the future, an interval under
/// [`MIN_INTERVAL_MINUTES`] or over [`MAX_INTERVAL_MINUTES`], an invalid
/// calendar.
pub fn validate(
    prompt: &str,
    name: Option<&str>,
    when: &ScheduleWhen,
    now: u64,
) -> Result<u64, ScheduleRefusal> {
    if prompt.trim().is_empty() {
        return Err(ScheduleRefusal::invalid("Write the prompt to send."));
    }
    if prompt.len() > MAX_PROMPT_BYTES || prompt.contains('\0') {
        return Err(ScheduleRefusal::invalid(
            "A scheduled prompt is at most 16 KiB of text.",
        ));
    }
    if let Some(name) = name
        && (name.trim().is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(char::is_control))
    {
        return Err(ScheduleRefusal::invalid(format!(
            "A schedule's name is one line of at most {MAX_NAME_CHARS} characters."
        )));
    }
    first_due(when, now)
}

/// The first due time of a new or changed schedule.
///
/// # Errors
/// See [`validate`].
pub fn first_due(when: &ScheduleWhen, now: u64) -> Result<u64, ScheduleRefusal> {
    match when {
        ScheduleWhen::Once { at } => {
            if *at <= now {
                return Err(ScheduleRefusal::invalid("Choose a time in the future."));
            }
            Ok(*at)
        }
        ScheduleWhen::Interval { minutes } => {
            if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(minutes) {
                return Err(ScheduleRefusal::invalid(format!(
                    "Repeat every {MIN_INTERVAL_MINUTES} minutes at the least and every 30 days at the most."
                )));
            }
            Ok(now + minutes_ms(*minutes))
        }
        ScheduleWhen::Calendar { calendar } => {
            calendar_occurrences(&agent_calendar(calendar), now, 1)
                .ok()
                .and_then(|dates| dates.first().copied())
                .ok_or_else(|| {
                    ScheduleRefusal::invalid(
                        "Choose an IANA timezone, a time of day and the days it runs.",
                    )
                })
        }
    }
}

/// What a schedule whose due time is `next_at` does at `now`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Due {
    /// Not yet.
    NotYet,
    /// Fire `occurrence` (the latest due time at or before `now`) once;
    /// `next` is the following due time (none: a one-time schedule).
    Fire { occurrence: u64, next: Option<u64> },
    /// `occurrence` is more than [`LATE_LIMIT_MS`] late: skip it and record
    /// that; `next` as for `Fire`.
    Missed { occurrence: u64, next: Option<u64> },
}

/// When a schedule due at `next_at` fires at `now`, and what comes after.
/// Only the latest due time at or before `now` is ever fired (never a
/// burst): an interval goes on from that time when it is on time, and from
/// `now` after a sleep that passed more than one due time; a calendar goes
/// on at its first date after `now`. A firing more than a day late is
/// skipped instead.
#[must_use]
pub fn due(when: &ScheduleWhen, next_at: u64, now: u64) -> Due {
    if next_at > now {
        return Due::NotYet;
    }
    let (occurrence, next) = match when {
        ScheduleWhen::Once { .. } => (next_at, None),
        ScheduleWhen::Interval { minutes } => {
            let step = minutes_ms(*minutes).max(1);
            let passed = (now - next_at) / step;
            let occurrence = next_at + passed * step;
            let next = if passed == 0 {
                occurrence + step
            } else {
                now + step
            };
            (occurrence, Some(next))
        }
        ScheduleWhen::Calendar { calendar } => {
            // Every weekday pattern has a date within a week, so the latest
            // date at or before `now` is among the ten after a week ago.
            let from = next_at
                .saturating_sub(1)
                .max(now.saturating_sub(8 * 24 * 60 * 60 * 1000));
            let dates = calendar_after(calendar, from, 10);
            let occurrence = dates
                .iter()
                .copied()
                .filter(|date| *date <= now)
                .max()
                .unwrap_or(next_at);
            let next = dates
                .iter()
                .copied()
                .find(|date| *date > now)
                .or_else(|| calendar_after(calendar, now, 1).first().copied());
            (occurrence, next)
        }
    };
    if now - occurrence > LATE_LIMIT_MS {
        Due::Missed { occurrence, next }
    } else {
        Due::Fire { occurrence, next }
    }
}

/// A firing's idempotent id.
#[must_use]
pub fn occurrence_id(id: &str, occurrence: u64) -> String {
    format!("scheduled:{id}:{occurrence}")
}

// --- The delivery gate -----------------------------------------------------------

/// What the host knows about a thread when a firing would go out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct GateFacts {
    /// The thread has a listed record and its session is in the registry.
    pub known: bool,
    pub archived: bool,
    pub settled: bool,
    /// An Away loop runs the thread.
    pub away: bool,
    /// A Code pane drives the thread's session.
    pub pane: bool,
    /// A turn is running or waiting on the owner.
    pub busy: bool,
    /// No guarded runtime to send through.
    pub no_runtime: bool,
    /// How long the firing has waited for the thread.
    pub waited_ms: u64,
}

/// Whether a firing goes out now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Gate {
    /// Record the attempt, then send it once.
    Deliver,
    /// Wait for the thread's turn to end.
    Wait,
    /// Not sent, for this reason (`outcome` says how it is recorded); never
    /// tried again before the next occurrence.
    Drop {
        outcome: ScheduleOutcome,
        detail: String,
    },
}

fn dropped(outcome: ScheduleOutcome, detail: &str) -> Gate {
    Gate::Drop {
        outcome,
        detail: detail.to_owned(),
    }
}

/// The delivery gate, as the phone's send: the thread must be idle or
/// failed, not held by an Away loop or a Code pane, not settled or archived.
/// A busy thread is waited for (up to [`LATE_LIMIT_MS`]); anything else
/// drops the firing with its reason.
#[must_use]
pub fn gate(facts: GateFacts) -> Gate {
    if !facts.known {
        return dropped(
            ScheduleOutcome::Refused,
            "The thread is no longer in Threads.",
        );
    }
    if facts.archived {
        return dropped(ScheduleOutcome::Refused, "The thread is archived.");
    }
    if facts.settled {
        return dropped(
            ScheduleOutcome::Skipped,
            "Paused while the thread is settled.",
        );
    }
    if facts.no_runtime {
        return dropped(
            ScheduleOutcome::Refused,
            "Interface could not send prompts yet.",
        );
    }
    if facts.away {
        return dropped(
            ScheduleOutcome::Refused,
            "An Away loop is running this thread.",
        );
    }
    if facts.pane {
        return dropped(
            ScheduleOutcome::Refused,
            "The thread is open in a Code pane.",
        );
    }
    if facts.busy {
        if facts.waited_ms > LATE_LIMIT_MS {
            return dropped(
                ScheduleOutcome::Skipped,
                "The thread was busy for more than a day.",
            );
        }
        return Gate::Wait;
    }
    Gate::Deliver
}

// --- Prompt text ------------------------------------------------------------------

fn cut_chars(text: &str, limit: usize) -> (&str, bool) {
    match text.char_indices().nth(limit) {
        Some((end, _)) => (&text[..end], true),
        None => (text, false),
    }
}

/// A schedule's label: its name, else its prompt's first words.
#[must_use]
pub fn schedule_label(name: Option<&str>, prompt: &str) -> String {
    if let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_owned();
    }
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Scheduled prompt");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    match cut_chars(&collapsed, LABEL_CHARS) {
        (cut, true) => format!("{}…", cut.trim_end()),
        (whole, false) => whole.to_owned(),
    }
}

/// What a scheduled prompt sends: one line naming it, then the prompt.
#[must_use]
pub fn scheduled_prompt(name: Option<&str>, prompt: &str) -> String {
    format!(
        "{SCHEDULED_PREFIX}{}\":\n{prompt}",
        schedule_label(name, prompt)
    )
}

/// The detail kept from an error (one line, bounded).
#[must_use]
pub fn detail(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match cut_chars(&collapsed, DETAIL_CHARS) {
        (cut, true) => format!("{cut}…"),
        (whole, false) => whole.to_owned(),
    }
}

// --- Resume at reset (T3 section 10) -------------------------------------------------

/// The first line a resume sends, up to its colon.
pub const RESUME_PREFIX: &str = "[Interface] Resumed after the usage limit reset";
/// What a resume sends when the owner left nothing unsent for the thread.
pub const DEFAULT_RESUME_PROMPT: &str = "Continue from where you stopped.";
/// A resume is due this long after the limit resets.
pub const RESUME_DELAY_MS: u64 = 60_000;
/// The thread has no usage limit with a known reset to resume at.
pub const LIMIT_UNKNOWN_CODE: &str = "threads-limit-unknown";

/// Milliseconds since the epoch of a UTC ISO time as usage windows and
/// rate-limit notices write it (`YYYY-MM-DDTHH:MM:SS[.fff…]Z`); `None` for
/// anything else.
#[must_use]
pub fn iso_millis(text: &str) -> Option<u64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-');
    let year = date
        .next()
        .filter(|part| part.len() == 4)?
        .parse::<i64>()
        .ok()?;
    let month = date
        .next()
        .filter(|part| part.len() == 2)?
        .parse::<i64>()
        .ok()?;
    let day = date
        .next()
        .filter(|part| part.len() == 2)?
        .parse::<i64>()
        .ok()?;
    if date.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.split(':');
    let mut part = |limit: u32| {
        clock
            .next()
            .filter(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_digit()))?
            .parse::<u32>()
            .ok()
            .filter(|value| *value <= limit)
    };
    let (hour, minute, second) = (part(23)?, part(59)?, part(60)?);
    if clock.next().is_some() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut millis = fraction.chars().take(3).collect::<String>();
    while millis.len() < 3 {
        millis.push('0');
    }
    let days = interface_common::time::days_from_civil(year, month, day);
    let seconds =
        days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    u64::try_from(seconds * 1_000 + millis.parse::<i64>().ok()?).ok()
}

/// When the usage limit resets by the provider's usage windows: among the
/// windows at or over their limit with a reset still ahead, the latest one
/// (the provider takes prompts again only once every full window has
/// reset). `None` when no window is full or none says when it resets.
#[must_use]
pub fn usage_windows_reset(windows: &[UsageWindow], now: u64) -> Option<String> {
    windows
        .iter()
        .filter(|window| window.utilization.is_some_and(|used| used >= 100.0))
        .filter_map(|window| {
            let text = window.resets_at.as_deref()?;
            let at = iso_millis(text).filter(|at| *at > now)?;
            Some((at, text))
        })
        .max_by_key(|(at, _)| *at)
        .map(|(_, text)| text.to_owned())
}

/// When a resume for a limit resetting at `resets_at` is due: a minute
/// after the reset, and never sooner than a minute from `now` (a reset
/// already past resumes a minute after the owner chose it).
#[must_use]
pub fn resume_at(resets_at: u64, now: u64) -> u64 {
    resets_at
        .saturating_add(RESUME_DELAY_MS)
        .max(now.saturating_add(RESUME_DELAY_MS))
}

/// A prompt a resume may carry: one line of text at most
/// [`MAX_PROMPT_BYTES`], not blank.
fn resumable(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_PROMPT_BYTES && !text.contains('\0')
}

/// What a resume sends, and the prompt-recovery receipt it consumes (its
/// id) when the prompt is one the limit refused before it reached the
/// provider. In order: the owner's queued follow-up that never went out
/// (`queued`); the thread's latest prompt-recovery receipt when it is
/// definitely unsent and carries no attachments (attachments are checked
/// again only by the owner's own Retry); otherwise
/// [`DEFAULT_RESUME_PROMPT`]. A receipt whose delivery was accepted or is
/// unknown is never reused.
#[must_use]
pub fn resume_prompt_choice(
    queued: Option<&str>,
    receipt: Option<&PromptRecoveryReceipt>,
) -> (String, Option<String>) {
    if let Some(queued) = queued.filter(|text| resumable(text)) {
        return (queued.to_owned(), None);
    }
    if let Some(receipt) = receipt.filter(|receipt| {
        receipt.delivery == PromptDelivery::Unsent
            && receipt.input.context.is_empty()
            && receipt.input.source.is_none()
            && resumable(&receipt.input.text)
            && id_ok(&receipt.id)
    }) {
        return (receipt.input.text.clone(), Some(receipt.id.clone()));
    }
    (DEFAULT_RESUME_PROMPT.to_owned(), None)
}

/// What a resume sends: one line saying why, then the prompt.
#[must_use]
pub fn resume_prompt(prompt: &str) -> String {
    format!("{RESUME_PREFIX}:\n{prompt}")
}

// --- Pull-request watch -------------------------------------------------------------

/// What a watch has seen on its pull request.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fingerprint {
    /// Each failing check as `<workflow / name>@<run id>` (the run's job too
    /// when known), sorted.
    pub failing_checks: Vec<String>,
    /// Each unresolved review thread's first comment URL, sorted.
    pub review_threads: Vec<String>,
}

/// A failing check's id: its name and the Actions run it failed in, so a
/// re-run that fails again is a new failure.
#[must_use]
pub fn check_id(workflow: Option<&str>, name: &str, link: Option<&str>) -> String {
    let named = workflow.map_or_else(
        || name.to_owned(),
        |workflow| format!("{workflow} / {name}"),
    );
    let run = link.and_then(|link| {
        let (_, rest) = link.split_once("/actions/runs/")?;
        let mut parts = rest.split(['/', '?', '#']);
        let run = parts.next().filter(|run| !run.is_empty())?;
        Some(match (parts.next(), parts.next()) {
            (Some("job"), Some(job)) if !job.is_empty() => format!("{run}/{job}"),
            _ => run.to_owned(),
        })
    });
    match run {
        Some(run) => format!("{named}@{run}"),
        None => named,
    }
}

/// What one look at a watched pull request found new.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Observation {
    /// The fingerprint to keep from now on.
    pub fingerprint: Fingerprint,
    /// A check is failing that was not before (or, on the first look, a
    /// check is failing right now).
    pub new_failure: bool,
    /// Review threads not seen before (never on the first look).
    pub new_reviews: Vec<String>,
}

impl Observation {
    /// Whether this look sends the thread a prompt.
    #[must_use]
    pub fn prompts(&self) -> bool {
        self.new_failure || !self.new_reviews.is_empty()
    }
}

fn sorted_bounded(ids: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids = ids
        .into_iter()
        .map(|id| cut_chars(&id, KEY_CHARS).0.to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    ids.truncate(MAX_FINGERPRINT_IDS);
    ids
}

/// Compares what the pull request shows now with what the watch saw. The
/// first look (`previous` none) only remembers, unless a check is failing
/// right then: that is sent once. After that, a failing check (by name and
/// run) or an unresolved review thread not seen before is new.
#[must_use]
pub fn observe(
    previous: Option<&Fingerprint>,
    failing_checks: impl IntoIterator<Item = String>,
    review_threads: impl IntoIterator<Item = String>,
) -> Observation {
    let fingerprint = Fingerprint {
        failing_checks: sorted_bounded(failing_checks),
        review_threads: sorted_bounded(review_threads),
    };
    let Some(previous) = previous else {
        return Observation {
            new_failure: !fingerprint.failing_checks.is_empty(),
            new_reviews: Vec::new(),
            fingerprint,
        };
    };
    let new_failure = fingerprint
        .failing_checks
        .iter()
        .any(|id| !previous.failing_checks.contains(id));
    let new_reviews = fingerprint
        .review_threads
        .iter()
        .filter(|id| !previous.review_threads.contains(id))
        .cloned()
        .collect();
    Observation {
        fingerprint,
        new_failure,
        new_reviews,
    }
}

/// A review thread as a diff quote, the way "Bring in review comments"
/// puts it in the draft (`formatDiffQuote`), or its path and comment for a
/// comment on a whole file.
fn review_block(comment: &ReviewComment) -> String {
    let Some(end) = comment.end_line else {
        return format!("{}\n{}\n\n", comment.path, comment.comment);
    };
    let start = comment.start_line.unwrap_or(end);
    let lines = if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    };
    let removed = if comment.side == interface_contracts::threads::DiffSide::Old {
        " (removed)"
    } else {
        ""
    };
    let longest = comment
        .code
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(3.max(longest + 1));
    let outdated = if comment.outdated {
        " (on lines that have changed since)"
    } else {
        ""
    };
    let said = format!("{}{outdated}", comment.comment);
    let said = said.trim();
    let mut parts = vec![
        format!("{}:{lines}{removed}", comment.path),
        format!("{fence}diff"),
        comment.code.clone(),
        fence,
    ];
    if !said.is_empty() {
        parts.push(said.to_owned());
    }
    parts.push(String::new());
    parts.push(String::new());
    parts.join("\n")
}

/// The text "Bring in review comments" adds to an empty draft
/// (`appendReviewComments` in `thread-ship.ts`): each review thread after
/// the last, one blank line apart.
#[must_use]
pub fn review_comments_text(comments: &[ReviewComment]) -> String {
    let mut text = String::new();
    for comment in comments {
        let block = review_block(comment);
        let current = text.trim_end();
        text = if current.is_empty() {
            block
        } else {
            format!("{current}\n\n{block}")
        };
    }
    text
}

/// The prompt a watch sends: one line naming the pull request and what
/// changed, then the failing-checks prompt "Fix failing checks" sends,
/// then the new review threads as "Bring in review comments" quotes them.
#[must_use]
pub fn watch_prompt(
    number: u64,
    url: &str,
    checks_prompt: Option<&str>,
    review_text: Option<&str>,
) -> String {
    let what = match (checks_prompt.is_some(), review_text.is_some()) {
        (true, true) => "checks are failing and there are new review comments",
        (true, false) => "checks are failing",
        _ => "there are new review comments",
    };
    let mut prompt = format!("{WATCH_PREFIX}{number} ({url}): {what}.");
    if let Some(checks) = checks_prompt {
        prompt.push_str("\n\n");
        prompt.push_str(checks.trim_end());
    }
    if let Some(review) = review_text {
        prompt.push_str("\n\nNew review comments:\n\n");
        prompt.push_str(review.trim_end());
        prompt.push_str("\n\nAddress these review comments.");
    }
    prompt
}

// --- The store ----------------------------------------------------------------------

/// A firing waiting for its thread (definitely not sent yet).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pending {
    pub occurrence: u64,
    pub since: u64,
}

/// One scheduled prompt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub thread_key: String,
    pub prompt: String,
    pub when: ScheduleWhen,
    pub enabled: bool,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ScheduleRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
    #[serde(default, skip_serializing_if = "is_owner")]
    pub set_by: ScheduleSetBy,
    /// Version 2: `resume` for Resume at reset; absent for a prompt.
    #[serde(default, skip_serializing_if = "ScheduleKind::is_prompt")]
    pub kind: ScheduleKind,
    /// Version 2, a resume only: the prompt-recovery receipt its prompt came
    /// from, consumed (as a Retry) by its one send, so the prompt never goes
    /// twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
fn is_owner(set_by: &ScheduleSetBy) -> bool {
    *set_by == ScheduleSetBy::Owner
}

/// What a watch kind watches. Only pull requests so far.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchKind {
    PullRequest,
}

/// A watch prompt waiting for its thread.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchPending {
    pub occurrence: u64,
    pub since: u64,
    pub prompt: String,
}

/// One thread's pull-request watch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Watch {
    pub thread_key: String,
    pub kind: WatchKind,
    pub enabled: bool,
    /// Absent until the first look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<ScheduleRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<WatchPending>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub needs_gh: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    schedules: Vec<Schedule>,
    watches: Vec<Watch>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            schedules: Vec::new(),
            watches: Vec::new(),
        }
    }
}

fn key_ok(key: &str) -> bool {
    !key.is_empty() && key.chars().count() <= KEY_CHARS && !key.contains('\0')
}

fn id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_CHARS
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn run_ok(run: Option<&ScheduleRun>) -> bool {
    run.is_none_or(|run| {
        run.detail
            .as_ref()
            .is_none_or(|detail| detail.chars().count() <= DETAIL_CHARS + 1)
    })
}

fn valid_state(state: &State) -> bool {
    state.version == STORE_VERSION && valid_content(state)
}

/// A version 1 document (protocol 62): no resume existed yet.
fn valid_v1_state(state: &State) -> bool {
    state.version == 1
        && state
            .schedules
            .iter()
            .all(|schedule| schedule.kind == ScheduleKind::Prompt && schedule.receipt.is_none())
        && valid_content(state)
}

fn valid_content(state: &State) -> bool {
    let mut ids = BTreeSet::new();
    let mut watched = BTreeSet::new();
    let mut resumed = BTreeSet::new();
    state.schedules.len() <= MAX_SCHEDULES
        && state.watches.len() <= MAX_WATCHES
        && state.schedules.iter().all(|schedule| {
            id_ok(&schedule.id)
                && ids.insert(schedule.id.as_str())
                && key_ok(&schedule.thread_key)
                && !schedule.prompt.is_empty()
                && schedule.prompt.len() <= MAX_PROMPT_BYTES
                && schedule
                    .name
                    .as_ref()
                    .is_none_or(|name| name.chars().count() <= MAX_NAME_CHARS)
                && run_ok(schedule.last_run.as_ref())
                && match schedule.kind {
                    ScheduleKind::Prompt => schedule.receipt.is_none(),
                    // One resume per thread, always a one-time schedule.
                    ScheduleKind::Resume => {
                        matches!(schedule.when, ScheduleWhen::Once { .. })
                            && resumed.insert(schedule.thread_key.as_str())
                            && schedule.receipt.as_deref().is_none_or(id_ok)
                    }
                }
        })
        && state.watches.iter().all(|watch| {
            key_ok(&watch.thread_key)
                && watched.insert(watch.thread_key.as_str())
                && watch.fingerprint.as_ref().is_none_or(|fingerprint| {
                    fingerprint.failing_checks.len() <= MAX_FINGERPRINT_IDS
                        && fingerprint.review_threads.len() <= MAX_FINGERPRINT_IDS
                })
                && watch
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.prompt.len() <= 4 * MAX_PROMPT_BYTES)
                && run_ok(watch.last_run.as_ref())
        })
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(
        BYTES,
        10,
        64 * (MAX_SCHEDULES + MAX_WATCHES * MAX_FINGERPRINT_IDS),
    )?)
}

fn run(
    occurrence: u64,
    at: u64,
    outcome: ScheduleOutcome,
    detail_text: Option<&str>,
) -> ScheduleRun {
    ScheduleRun {
        occurrence,
        at,
        outcome,
        detail: detail_text.map(detail),
    }
}

/// A new schedule, as the host builds it.
#[derive(Clone, Debug)]
pub struct NewSchedule {
    pub id: String,
    pub thread_key: String,
    pub name: Option<String>,
    pub prompt: String,
    pub when: ScheduleWhen,
    pub set_by: ScheduleSetBy,
    /// Protocol 66: a resume, and the receipt its prompt came from.
    pub kind: ScheduleKind,
    pub receipt: Option<String>,
}

/// What a due pass did to one schedule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Advanced {
    /// A firing now waits for its thread.
    Waiting { id: String, occurrence: u64 },
    /// A firing was skipped and recorded.
    Skipped { id: String, occurrence: u64 },
}

/// The leased single writer of `thread-schedules.v2.json`.
pub struct ThreadScheduleStore {
    document: LeasedDocument,
    state: State,
    quarantined: Option<PathBuf>,
}

impl ThreadScheduleStore {
    /// Opens (or creates) the store at `file`. A document that does not
    /// decode or breaks a bound is quarantined and the store starts empty.
    ///
    /// # Errors
    /// Refuses unsafe storage, a lease held by another writer, or a file over
    /// the byte bound.
    ///
    /// When `file` does not exist, a version 1 store beside it
    /// ([`THREAD_SCHEDULES_V1_FILE_NAME`]) is read once, never changed,
    /// moved or quarantined, and carried over as version 2 (written at
    /// once, so it is never read again). One that cannot be read or is
    /// invalid is left for the build that wrote it; the store starts empty.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let limits = limits()?;
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            file,
            LOCK_NAME,
            limits,
            UNCERTAIN,
            valid_state,
        )?;
        let older = (state.is_none() && quarantined.is_none())
            .then(|| read_v1(file, limits))
            .flatten();
        let mut store = Self {
            document,
            state: State::default(),
            quarantined,
        };
        match (state, older) {
            (Some(state), _) => store.state = state,
            (None, Some(older)) => store.commit(older).map_err(|error| match error {
                ScheduleError::Storage(error) => error,
                ScheduleError::Refused(refusal) => {
                    SettingsError::Storage(interface_storage::StorageError::Corrupt {
                        path: file.to_path_buf(),
                        reason: refusal.message,
                    })
                }
            })?,
            (None, None) => {}
        }
        Ok(store)
    }

    /// Where an unreadable file found at open was set aside.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    #[must_use]
    pub fn schedules(&self) -> &[Schedule] {
        &self.state.schedules
    }

    #[must_use]
    pub fn watches(&self) -> &[Watch] {
        &self.state.watches
    }

    #[must_use]
    pub fn schedule(&self, id: &str) -> Option<&Schedule> {
        self.state
            .schedules
            .iter()
            .find(|schedule| schedule.id == id)
    }

    #[must_use]
    pub fn watch(&self, key: &str) -> Option<&Watch> {
        self.state
            .watches
            .iter()
            .find(|watch| watch.thread_key == key)
    }

    /// The schedule named `target` by its id, else by its exact name.
    ///
    /// # Errors
    /// None matches, or the name is shared by several schedules.
    pub fn find(&self, target: &str) -> Result<&Schedule, ScheduleRefusal> {
        if let Some(schedule) = self.schedule(target) {
            return Ok(schedule);
        }
        let named = self
            .state
            .schedules
            .iter()
            .filter(|schedule| schedule.name.as_deref() == Some(target))
            .collect::<Vec<_>>();
        match named.as_slice() {
            [one] => Ok(one),
            [] => Err(ScheduleRefusal::new(
                SCHEDULE_UNKNOWN_CODE,
                format!("No scheduled prompt is called or numbered {target}."),
            )),
            _ => Err(ScheduleRefusal::new(
                SCHEDULE_INVALID_CODE,
                format!("Several scheduled prompts are called {target}; use its id."),
            )),
        }
    }

    /// The soonest due time of an enabled schedule, and whether any firing
    /// waits for its thread.
    #[must_use]
    pub fn next_wake(&self) -> Option<u64> {
        self.state
            .schedules
            .iter()
            .filter(|schedule| schedule.enabled)
            .filter_map(|schedule| schedule.next_at)
            .min()
    }

    /// Whether a schedule or watch firing waits for a thread.
    #[must_use]
    pub fn has_waiting(&self) -> bool {
        self.state
            .schedules
            .iter()
            .any(|schedule| schedule.pending.is_some())
            || self
                .state
                .watches
                .iter()
                .any(|watch| watch.pending.is_some())
    }

    /// Resume at reset (T3 section 10): `added` (a one-time resume) becomes
    /// thread `added.thread_key`'s one resume, due at `first`, replacing an
    /// earlier one of the thread, in one commit.
    ///
    /// # Errors
    /// Not a one-time resume, a full store, failed or uncertain storage.
    pub fn set_resume(
        &mut self,
        added: NewSchedule,
        first: u64,
        now: u64,
    ) -> Result<(), ScheduleError> {
        if added.kind != ScheduleKind::Resume
            || !matches!(added.when, ScheduleWhen::Once { .. })
            || !resumable(&added.prompt)
        {
            return Err(ScheduleRefusal::invalid("Invalid resume.").into());
        }
        let before = self.state.clone();
        self.state.schedules.retain(|schedule| {
            !(schedule.kind == ScheduleKind::Resume && schedule.thread_key == added.thread_key)
        });
        let result = self.add(added, first, now);
        if result.is_err() {
            self.state = before;
        }
        result
    }

    /// Adds `new`, due first at `first`.
    ///
    /// # Errors
    /// A full store, a repeated or invalid id, failed or uncertain storage.
    pub fn add(&mut self, added: NewSchedule, first: u64, now: u64) -> Result<(), ScheduleError> {
        if self.state.schedules.len() >= MAX_SCHEDULES {
            return Err(ScheduleRefusal::new(
                SCHEDULE_FULL_CODE,
                format!(
                    "Interface keeps at most {MAX_SCHEDULES} scheduled prompts. Delete some in Settings → Threads first."
                ),
            )
            .into());
        }
        if !id_ok(&added.id) || !key_ok(&added.thread_key) || self.schedule(&added.id).is_some() {
            return Err(ScheduleRefusal::invalid("Invalid scheduled prompt.").into());
        }
        let mut state = self.state.clone();
        state.schedules.push(Schedule {
            id: added.id,
            name: added.name.map(|name| name.trim().to_owned()),
            thread_key: added.thread_key,
            prompt: added.prompt,
            when: added.when,
            enabled: true,
            created_at: now,
            next_at: Some(first),
            last_run: None,
            pending: None,
            set_by: added.set_by,
            kind: added.kind,
            receipt: added.receipt,
        });
        self.commit(state)
    }

    /// Changes schedule `id`'s name, prompt and time; its due time is
    /// `first` and a firing waiting for the thread is dropped.
    ///
    /// # Errors
    /// An unknown id, a schedule of another thread, failed or uncertain
    /// storage.
    pub fn update(
        &mut self,
        id: &str,
        thread_key: &str,
        name: Option<String>,
        prompt: String,
        when: ScheduleWhen,
        first: u64,
    ) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        let schedule = state
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == id && schedule.thread_key == thread_key)
            .ok_or_else(ScheduleRefusal::unknown)?;
        schedule.name = name.map(|name| name.trim().to_owned());
        schedule.prompt = prompt;
        schedule.when = when;
        schedule.enabled = true;
        schedule.next_at = Some(first);
        schedule.pending = None;
        self.commit(state)
    }

    /// Deletes schedule `id`; answers its thread.
    ///
    /// # Errors
    /// An unknown id, failed or uncertain storage.
    pub fn delete(&mut self, id: &str) -> Result<String, ScheduleError> {
        let key = self
            .schedule(id)
            .map(|schedule| schedule.thread_key.clone())
            .ok_or_else(ScheduleRefusal::unknown)?;
        let mut state = self.state.clone();
        state.schedules.retain(|schedule| schedule.id != id);
        self.commit(state)?;
        Ok(key)
    }

    /// Turns schedule `id` on (due again from `now`; a one-time schedule
    /// whose time has passed is refused) or off (a waiting firing is
    /// dropped).
    ///
    /// # Errors
    /// An unknown id, a one-time schedule in the past, failed or uncertain
    /// storage.
    pub fn set_enabled(&mut self, id: &str, enabled: bool, now: u64) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        let schedule = state
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == id)
            .ok_or_else(ScheduleRefusal::unknown)?;
        if schedule.enabled == enabled {
            return Ok(());
        }
        if enabled {
            schedule.next_at = Some(first_due(&schedule.when, now)?);
        } else {
            schedule.pending = None;
        }
        schedule.enabled = enabled;
        self.commit(state)
    }

    /// The thread `key` was deleted, archived or forgotten: its schedules
    /// and watch go.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn remove_thread(&mut self, key: &str) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        state
            .schedules
            .retain(|schedule| schedule.thread_key != key);
        state.watches.retain(|watch| watch.thread_key != key);
        self.commit(state)
    }

    /// A switched thread (T3 section 5) runs on `to` now: its schedules and
    /// watch follow the head. Its resume goes: the limit it waited for was
    /// the old provider's.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn follow(&mut self, from: &str, to: &str) -> Result<(), ScheduleError> {
        if !key_ok(to) {
            return Err(ScheduleRefusal::invalid("Invalid thread to follow.").into());
        }
        let mut state = self.state.clone();
        state.schedules.retain(|schedule| {
            !(schedule.kind == ScheduleKind::Resume && schedule.thread_key == from)
        });
        for schedule in &mut state.schedules {
            if schedule.thread_key == from {
                to.clone_into(&mut schedule.thread_key);
            }
        }
        if !state.watches.iter().any(|watch| watch.thread_key == to) {
            for watch in &mut state.watches {
                if watch.thread_key == from {
                    to.clone_into(&mut watch.thread_key);
                }
            }
        }
        self.commit(state)
    }

    /// One due pass at `now`: each enabled schedule whose time came fires
    /// its latest due time once (it then waits for its thread) or records
    /// it skipped: more than a day late, paused while `settled(thread)`, or
    /// another firing of it still waiting. Its next due time is set in the
    /// same commit, so no firing is ever considered twice.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn advance(
        &mut self,
        now: u64,
        settled: impl Fn(&str) -> bool,
    ) -> Result<Vec<Advanced>, ScheduleError> {
        let mut state = self.state.clone();
        let mut advanced = Vec::new();
        for schedule in &mut state.schedules {
            let Some(next_at) = schedule.next_at.filter(|_| schedule.enabled) else {
                continue;
            };
            let (occurrence, next, skip) = match due(&schedule.when, next_at, now) {
                Due::NotYet => continue,
                Due::Missed { occurrence, next } => (
                    occurrence,
                    next,
                    Some("Missed by more than a day while Interface was closed or the Mac slept."),
                ),
                Due::Fire { occurrence, next } => {
                    let skip = if settled(&schedule.thread_key) {
                        Some("Paused while the thread is settled.")
                    } else if schedule.pending.is_some() {
                        Some("The last run was still waiting for the thread.")
                    } else {
                        None
                    };
                    (occurrence, next, skip)
                }
            };
            schedule.next_at = next;
            if let Some(why) = skip {
                schedule.last_run = Some(run(occurrence, now, ScheduleOutcome::Skipped, Some(why)));
                if next.is_none() && schedule.pending.is_none() {
                    schedule.enabled = false;
                }
                advanced.push(Advanced::Skipped {
                    id: schedule.id.clone(),
                    occurrence,
                });
            } else {
                schedule.pending = Some(Pending {
                    occurrence,
                    since: now,
                });
                advanced.push(Advanced::Waiting {
                    id: schedule.id.clone(),
                    occurrence,
                });
            }
        }
        if !advanced.is_empty() {
            self.commit(state)?;
        }
        Ok(advanced)
    }

    /// `iface trigger`: schedule `id` fires now, outside its times. Answers
    /// the firing's occurrence (`now`, or just above the last one recorded).
    ///
    /// # Errors
    /// An unknown id, a schedule turned off, one already waiting, failed or
    /// uncertain storage.
    pub fn trigger(&mut self, id: &str, now: u64) -> Result<u64, ScheduleError> {
        let mut state = self.state.clone();
        let schedule = state
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == id)
            .ok_or_else(ScheduleRefusal::unknown)?;
        if !schedule.enabled {
            return Err(ScheduleRefusal::new(
                SCHEDULE_OFF_CODE,
                "That scheduled prompt is turned off. Turn it on first.",
            )
            .into());
        }
        if schedule.pending.is_some() {
            return Err(ScheduleRefusal::new(
                SCHEDULE_WAITING_CODE,
                "A run of that scheduled prompt is already waiting for its thread.",
            )
            .into());
        }
        let occurrence = schedule
            .last_run
            .as_ref()
            .map_or(now, |last| now.max(last.occurrence + 1));
        schedule.pending = Some(Pending {
            occurrence,
            since: now,
        });
        self.commit(state)?;
        Ok(occurrence)
    }

    /// Records the attempt to send schedule `id`'s waiting `occurrence`,
    /// BEFORE the send: its last run says unknown until the send's outcome
    /// is recorded, the firing stops waiting, and a one-time schedule turns
    /// off. Answers the schedule as it was, or `None` when that firing no
    /// longer waits (then nothing may be sent).
    ///
    /// # Errors
    /// Failed or uncertain storage: nothing may be sent.
    pub fn begin_attempt(
        &mut self,
        id: &str,
        occurrence: u64,
        now: u64,
    ) -> Result<Option<Schedule>, ScheduleError> {
        let mut state = self.state.clone();
        let Some(schedule) = state.schedules.iter_mut().find(|schedule| {
            schedule.id == id
                && schedule.enabled
                && schedule
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.occurrence == occurrence)
        }) else {
            return Ok(None);
        };
        let before = schedule.clone();
        schedule.pending = None;
        schedule.last_run = Some(run(
            occurrence,
            now,
            ScheduleOutcome::Unknown,
            Some("Sending…"),
        ));
        if schedule.next_at.is_none() {
            schedule.enabled = false;
        }
        self.commit(state)?;
        Ok(Some(before))
    }

    /// Records what became of schedule `id`'s `occurrence`: its send's
    /// outcome, or, for a firing that never went out, why. A waiting firing
    /// of that occurrence stops waiting.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn record(
        &mut self,
        id: &str,
        occurrence: u64,
        outcome: ScheduleOutcome,
        detail_text: Option<&str>,
        now: u64,
    ) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        let Some(schedule) = state
            .schedules
            .iter_mut()
            .find(|schedule| schedule.id == id)
        else {
            return Ok(());
        };
        if schedule
            .pending
            .as_ref()
            .is_some_and(|pending| pending.occurrence == occurrence)
        {
            schedule.pending = None;
            if schedule.next_at.is_none() {
                schedule.enabled = false;
            }
        }
        schedule.last_run = Some(run(occurrence, now, outcome, detail_text));
        self.commit(state)
    }

    /// Turns thread `key`'s watch on (it looks again from scratch) or off.
    ///
    /// # Errors
    /// A full store, failed or uncertain storage.
    pub fn set_watch(&mut self, key: &str, enabled: bool) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        match state
            .watches
            .iter_mut()
            .find(|watch| watch.thread_key == key)
        {
            Some(watch) => {
                if watch.enabled == enabled {
                    return Ok(());
                }
                watch.enabled = enabled;
                watch.pending = None;
                if enabled {
                    watch.fingerprint = None;
                    watch.observed_at = None;
                    watch.needs_gh = false;
                }
            }
            None if !enabled => return Ok(()),
            None => {
                if state.watches.len() >= MAX_WATCHES {
                    return Err(ScheduleRefusal::new(
                        SCHEDULE_FULL_CODE,
                        format!(
                            "Interface watches at most {MAX_WATCHES} pull requests. Turn one off in Settings → Threads first."
                        ),
                    )
                    .into());
                }
                if !key_ok(key) {
                    return Err(ScheduleRefusal::invalid("Invalid thread.").into());
                }
                state.watches.push(Watch {
                    thread_key: key.to_owned(),
                    kind: WatchKind::PullRequest,
                    enabled: true,
                    fingerprint: None,
                    observed_at: None,
                    last_run: None,
                    pending: None,
                    needs_gh: false,
                });
            }
        }
        self.commit(state)
    }

    /// The watched pull request was merged or closed (`why`): the watch
    /// turns off.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn watch_off(&mut self, key: &str, why: &str, now: u64) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        let Some(watch) = state
            .watches
            .iter_mut()
            .find(|watch| watch.thread_key == key)
        else {
            return Ok(());
        };
        watch.enabled = false;
        watch.pending = None;
        watch.last_run = Some(run(
            watch.observed_at.unwrap_or(now),
            now,
            ScheduleOutcome::Skipped,
            Some(why),
        ));
        self.commit(state)
    }

    /// `gh` is missing or signed out (`true`), or answers again.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn set_needs_gh(&mut self, key: &str, needs: bool) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        let Some(watch) = state
            .watches
            .iter_mut()
            .find(|watch| watch.thread_key == key)
        else {
            return Ok(());
        };
        watch.needs_gh = needs;
        self.commit(state)
    }

    /// Records one look at thread `key`'s pull request: the fingerprint to
    /// keep, and (when it found something new) the prompt that now waits
    /// for the thread. A watch with a prompt still waiting keeps its old
    /// fingerprint and takes no new one, so what this look saw is found new
    /// again after that prompt went out.
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn observed(
        &mut self,
        key: &str,
        fingerprint: Fingerprint,
        prompt: Option<String>,
        now: u64,
    ) -> Result<bool, ScheduleError> {
        let mut state = self.state.clone();
        let Some(watch) = state
            .watches
            .iter_mut()
            .find(|watch| watch.thread_key == key && watch.enabled && watch.pending.is_none())
        else {
            return Ok(false);
        };
        watch.fingerprint = Some(fingerprint);
        watch.observed_at = Some(now);
        watch.needs_gh = false;
        let waiting = prompt.is_some();
        watch.pending = prompt.map(|prompt| WatchPending {
            occurrence: now,
            since: now,
            prompt,
        });
        self.commit(state)?;
        Ok(waiting)
    }

    /// Records the attempt to send thread `key`'s waiting watch prompt,
    /// BEFORE the send (as [`Self::begin_attempt`]). Answers the prompt and
    /// its occurrence, or `None` when nothing waits.
    ///
    /// # Errors
    /// Failed or uncertain storage: nothing may be sent.
    pub fn begin_watch_attempt(
        &mut self,
        key: &str,
        now: u64,
    ) -> Result<Option<WatchPending>, ScheduleError> {
        let mut state = self.state.clone();
        let Some(watch) = state
            .watches
            .iter_mut()
            .find(|watch| watch.thread_key == key && watch.enabled)
        else {
            return Ok(None);
        };
        let Some(pending) = watch.pending.take() else {
            return Ok(None);
        };
        watch.last_run = Some(run(
            pending.occurrence,
            now,
            ScheduleOutcome::Unknown,
            Some("Sending…"),
        ));
        self.commit(state)?;
        Ok(Some(pending))
    }

    /// Records what became of thread `key`'s watch prompt `occurrence` (a
    /// waiting one of that occurrence stops waiting).
    ///
    /// # Errors
    /// Failed or uncertain storage.
    pub fn record_watch(
        &mut self,
        key: &str,
        occurrence: u64,
        outcome: ScheduleOutcome,
        detail_text: Option<&str>,
        now: u64,
    ) -> Result<(), ScheduleError> {
        let mut state = self.state.clone();
        let Some(watch) = state
            .watches
            .iter_mut()
            .find(|watch| watch.thread_key == key)
        else {
            return Ok(());
        };
        if watch
            .pending
            .as_ref()
            .is_some_and(|pending| pending.occurrence == occurrence)
        {
            watch.pending = None;
        }
        watch.last_run = Some(run(occurrence, now, outcome, detail_text));
        self.commit(state)
    }

    /// Thread `key`'s schedules (soonest first) and watch as the view shows
    /// them; `paused` while the thread is settled. `None` when it has
    /// neither.
    #[must_use]
    pub fn view(&self, key: &str, paused: bool) -> Option<ThreadSchedules> {
        let mut schedules = self
            .state
            .schedules
            .iter()
            .filter(|schedule| schedule.thread_key == key)
            .map(|schedule| ThreadScheduleView {
                id: schedule.id.clone(),
                name: schedule.name.clone(),
                prompt: schedule.prompt.clone(),
                when: schedule.when.clone(),
                enabled: schedule.enabled,
                created_at: schedule.created_at,
                next_at: schedule.next_at,
                last_run: schedule.last_run.clone(),
                waiting: schedule.pending.as_ref().map(|pending| pending.occurrence),
                set_by: schedule.set_by,
                paused,
                kind: schedule.kind,
            })
            .collect::<Vec<_>>();
        // A firing waiting for the thread is due already: it comes first.
        schedules.sort_by_key(|schedule| {
            (
                !schedule.enabled,
                schedule.waiting.or(schedule.next_at).unwrap_or(u64::MAX),
                schedule.created_at,
            )
        });
        let watch = self.watch(key).map(|watch| ThreadWatchView {
            enabled: watch.enabled,
            needs_gh: watch.needs_gh,
            failing_checks: watch.fingerprint.as_ref().map_or(0, |fingerprint| {
                u32::try_from(fingerprint.failing_checks.len()).unwrap_or(u32::MAX)
            }),
            review_threads: watch.fingerprint.as_ref().map_or(0, |fingerprint| {
                u32::try_from(fingerprint.review_threads.len()).unwrap_or(u32::MAX)
            }),
            observed_at: watch.observed_at,
            last_run: watch.last_run.clone(),
            waiting: watch.pending.is_some(),
        });
        (!schedules.is_empty() || watch.is_some()).then_some(ThreadSchedules { schedules, watch })
    }

    fn commit(&mut self, state: State) -> Result<(), ScheduleError> {
        self.document.ensure_writable()?;
        if state == self.state {
            return Ok(());
        }
        if !valid_state(&state) {
            return Err(ScheduleRefusal::invalid(
                "That change would break the scheduled prompts' bounds.",
            )
            .into());
        }
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
}

/// The version 1 store beside `file`, carried over as version 2; `None` when
/// it is absent, unreadable or invalid. Read-only: the older file and its
/// directory are never hardened, rewritten, moved or quarantined.
fn read_v1(file: &Path, limits: JsonLimits) -> Option<State> {
    let older = file.with_file_name(THREAD_SCHEDULES_V1_FILE_NAME);
    older.symlink_metadata().ok()?;
    let directory = PrivateDirectory::open(older.parent()?).ok()?;
    let mut state = read_json::<State>(&directory, THREAD_SCHEDULES_V1_FILE_NAME, limits)
        .ok()??
        .into_parts()
        .0;
    if !valid_v1_state(&state) {
        return None;
    }
    state.version = STORE_VERSION;
    Some(state)
}

#[cfg(test)]
#[path = "thread_schedules_tests.rs"]
mod tests;
