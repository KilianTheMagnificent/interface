//! Desktop project Away v1. Independent of the selected-file CLI cycle.
//!
//! Protocol 45 runs each loop as a Threads thread: a run names its thread
//! (`Run.thread`) and each work turn the thread turn it ran as
//! (`Turn.threadTurn`), both absent for loops from before. The wire snapshot
//! keeps version 1 with those optional fields; the store on disk is version 2
//! (`project-away.v2.json`, [`STORE_VERSION`]).
//!
//! Protocol 47 lets a loop's reviews run on another provider
//! (`Options.reviewer`; absent means the worker's own, as every loop before
//! it). The store moves to version 3 (`project-away.v3.json`).
use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
/// Additive read-only summary; persisted snapshots and mutation requests stay v1.
pub const STATUS_VERSION: u8 = 2;
/// The persisted store's version since protocol 47 (`project-away.v3.json`):
/// options may name a reviewer. A version 2 or version 1 file is carried
/// over once and left as it was; its options have no reviewer, which means
/// the worker's own provider reviews, as it always did.
pub const STORE_VERSION: u8 = 3;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    pub objective: String,
    pub provider: String,
    pub model: Option<String>,
    pub duration_seconds: u64,
    pub max_iterations: u8,
    /// The provider whose fresh sessions review each round (protocol 47):
    /// `codex` or `claude`, at its own default model. Absent: the worker's
    /// provider and model, as for every loop before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            objective: "Improve this project. Follow its instructions and existing to-do list; choose useful, evidence-backed work, implement it, run the relevant checks, and stop when no justified work remains.".into(),
            provider: "codex".into(), model: None, duration_seconds: 3600, max_iterations: 12,
            reviewer: None,
        }
    }
}
impl Options {
    /// The provider that reviews each round: the chosen reviewer, else the
    /// worker's own.
    #[must_use]
    pub fn review_provider(&self) -> &str {
        self.reviewer.as_deref().unwrap_or(&self.provider)
    }
    /// # Errors
    /// Rejects unsupported providers, empty objectives and unbounded runs.
    pub fn validate(&self) -> Result<(), String> {
        if self.objective.trim().is_empty()
            || self.objective.len() > 8192
            || self.objective.contains('\0')
            || !matches!(self.provider.as_str(), "codex" | "claude")
            || !matches!(self.review_provider(), "codex" | "claude")
            || !(60..=86400).contains(&self.duration_seconds)
            || !(1..=20).contains(&self.max_iterations)
            || self
                .model
                .as_ref()
                .is_some_and(|m| m.is_empty() || m.len() > 128 || m.chars().any(char::is_control))
        {
            return Err(
                "Away requires an objective, Codex or Claude, 1–1440 minutes and 1–20 iterations."
                    .into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_status_fixture_round_trips_without_shape_translation() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/project-away-status-v28.json"
        ))
        .unwrap();
        let request: Request = serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(request.version, STATUS_VERSION);
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
        let response: Status = serde_json::from_value(fixture["response"].clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
    }
    /// Protocol 47: options may name the provider that reviews each round,
    /// in commands, presets and runs, and a thread's projection names it;
    /// options without one review with the worker's provider.
    #[test]
    fn away_reviewer_fixture_round_trips_and_defaults_to_the_worker() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/project-away-v47.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 47);
        for command in fixture["commands"].as_array().unwrap() {
            let decoded: Request = serde_json::from_value(command.clone()).unwrap();
            assert_eq!(serde_json::to_value(&decoded).unwrap(), *command);
            let (Command::Start {
                options: Some(options),
                ..
            }
            | Command::Save { options, .. }) = decoded.command
            else {
                panic!("a start or save with options");
            };
            assert_eq!(options.review_provider(), "claude");
            assert!(options.validate().is_ok());
        }
        let snapshot: Snapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            fixture["snapshot"]
        );
        assert_eq!(snapshot.defaults.reviewer, None);
        assert_eq!(snapshot.defaults.review_provider(), "codex");
        assert_eq!(snapshot.runs[0].options.reviewer.as_deref(), Some("claude"));
        let away: ThreadAway = serde_json::from_value(fixture["threadAway"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&away).unwrap(), fixture["threadAway"]);
        assert_eq!(away.reviewer.as_deref(), Some("claude"));
        let grok = Options {
            reviewer: Some("grok".into()),
            ..Options::default()
        };
        assert!(
            grok.validate().is_err(),
            "Away reviews with Codex or Claude only"
        );
    }
    /// Protocol 45 (review B3): a run names the thread it runs as and each
    /// work turn the thread turn it ran as; both round-trip, a run from
    /// before carries neither, and unknown fields are refused.
    #[test]
    fn away_thread_fixture_round_trips_and_older_runs_carry_no_thread() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/project-away-v45.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 45);
        let snapshot: Snapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            fixture["snapshot"]
        );
        assert_eq!(snapshot.version, VERSION);
        let run = &snapshot.runs[0];
        let thread = run.thread.as_ref().unwrap();
        assert_eq!(Some(&thread.key), run.turns[0].session_key.as_ref());
        assert_eq!(run.turns[0].thread_turn, Some(0));
        assert_eq!(run.turns[1].thread_turn, None);
        for pointer in ["/runs/0/thread", "/runs/0/turns/0"] {
            let mut foreign = fixture["snapshot"].clone();
            foreign
                .pointer_mut(pointer)
                .and_then(serde_json::Value::as_object_mut)
                .unwrap()
                .insert("executeShell".into(), serde_json::json!(true));
            assert!(
                serde_json::from_value::<Snapshot>(foreign).is_err(),
                "{pointer}"
            );
        }
        let older: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/project-away-v22.json"
        ))
        .unwrap();
        let older: Snapshot = serde_json::from_value(older["snapshot"].clone()).unwrap();
        assert!(
            older.runs.iter().all(|run| run.thread.is_none()
                && run.turns.iter().all(|turn| turn.thread_turn.is_none()))
        );
        assert_eq!(RoundState::parse("cancelled"), Some(RoundState::Cancelled));
        assert_eq!(RoundState::parse("launching"), None);
    }
    #[test]
    fn desktop_away_fixture_round_trips_without_shape_translation() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/project-away-v22.json"
        ))
        .unwrap();
        for command in fixture["commands"].as_array().unwrap() {
            let decoded: Request = serde_json::from_value(command.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *command);
        }
        let snapshot: Snapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), fixture["snapshot"]);
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub command: Command,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Command {
    Inspect {},
    Status {},
    Start {
        run_id: String,
        cwd: String,
        options: Option<Options>,
    },
    Stop {
        run_id: String,
    },
    Save {
        cwd: String,
        options: Options,
    },
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum State {
    Running,
    Stopping,
    Completed,
    Stopped,
    Expired,
    Blocked,
    Unknown,
}
impl State {
    #[must_use]
    pub const fn active(self) -> bool {
        matches!(self, Self::Running | Self::Stopping)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Work,
    Review,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Turn {
    pub iteration: u8,
    pub role: Role,
    pub session_key: Option<String>,
    pub state: String,
    pub report: String,
    /// A work turn's ordinal in its run's thread (protocol 45): the turn the
    /// thread recorded for its prompt. Absent for reviews, for turns the
    /// thread did not record and for loops from before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_turn: Option<u32>,
}
/// The thread a loop runs as (protocol 45): its work session, in the loop's
/// own worktree of the project. Reviews run in fresh sessions in the same
/// worktree and stay out of Threads.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunThread {
    /// The work session's key, which is the thread's.
    pub key: String,
    /// The worktree the loop works in.
    pub worktree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub cwd: String,
    pub options: Options,
    pub started_at_ms: u64,
    pub deadline_at_ms: u64,
    pub state: State,
    pub detail: String,
    pub turns: Vec<Turn>,
    /// Absent until the loop's thread exists, and for loops from before
    /// protocol 45 (which worked in the project's own checkout).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<RunThread>,
}

/// A reviewer's or worker's verdict, from its `AWAY_RESULT` line.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Continue,
    Complete,
    Revise,
    Blocked,
}
/// Where one of a loop's rounds stands.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RoundState {
    Started,
    Completed,
    Blocked,
    Unknown,
    Cancelled,
}
impl RoundState {
    /// The store's word for a turn's state; `None` for a word it never writes.
    #[must_use]
    pub fn parse(state: &str) -> Option<Self> {
        Some(match state {
            "started" => Self::Started,
            "completed" => Self::Completed,
            "blocked" => Self::Blocked,
            "unknown" => Self::Unknown,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}
/// One round of a loop as its thread shows it (protocol 45).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadAwayRound {
    pub iteration: u8,
    pub role: Role,
    pub state: RoundState,
    /// A work round: the thread turn it ran as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    /// A review round: its own session, kept out of Threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// From the round's `AWAY_RESULT`; absent while it runs or when it ended
    /// without a well-formed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    /// The result's summary, cut for display (the session keeps it whole).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The checks the result names, cut for display.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
}
/// The Away loop a thread runs as (protocol 45): Rust's projection of the
/// run, for the thread's round cards and its report.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadAway {
    pub run_id: String,
    pub state: State,
    /// What the loop is doing, or how it ended.
    pub detail: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider whose sessions review each round (protocol 47); absent
    /// from views before it, where the worker's provider reviewed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    pub objective: String,
    pub started_at: u64,
    pub deadline_at: u64,
    pub max_iterations: u8,
    pub rounds: Vec<ThreadAwayRound>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preset {
    pub cwd: String,
    pub options: Options,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub version: u8,
    pub revision: u64,
    pub defaults: Options,
    pub presets: Vec<Preset>,
    pub runs: Vec<Run>,
    pub storage_uncertain: bool,
    pub cancelled_starts: Vec<String>,
}

/// Small polling response. Run identities reconcile lost start replies without
/// transferring objectives, settings, turns or report bodies.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Status {
    pub version: u8,
    pub revision: u64,
    pub storage_uncertain: bool,
    pub active_runs: Vec<ActiveRun>,
    pub run_ids: Vec<String>,
    pub cancelled_starts: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveRun {
    pub id: String,
    pub cwd: String,
}
