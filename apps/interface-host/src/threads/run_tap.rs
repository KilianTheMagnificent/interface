//! The run tap (which engine events start and end a thread's run), the
//! registry signal, and the send hook that checkpoints and records a turn
//! before the runtime dispatcher forwards a send — or, for a send steered
//! into a running turn, records nothing unless the provider later makes it a
//! turn of its own.

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    ipc::parse_key,
    thread_journal::TurnOrigin,
    threads::{RunOutcome, WorkspaceMode},
};
use interface_review::thread_git::CaptureOutcome;
use interface_settings::threads::{BegunTurn, turn_ref};
use tokio::sync::{Notify, OwnedMutexGuard};

use super::{THREADS_WORKTREE_UNAVAILABLE_CODE, ThreadHost};
use crate::dispatcher::{
    DispatchError, RegistryRoundObserver, SendHooks, SendSettlement, SendTicket,
    SessionEventObserver,
};

/// A send's start checkpoint never holds it back longer than this.
const START_CAPTURE_LIMIT: Duration = Duration::from_secs(10);

/// Engines whose provider folds a send made while a turn runs into that turn
/// (the ones whose adapters declare `EngineCapabilities.steer`): Codex steers
/// it (`turn/steer`); Claude Code queues it and hands it to the running turn
/// at its next tool call. Either starts a new turn with it instead when the
/// running one ends first (Codex's `turn/start` fallback, Claude's queue).
const STEERING_ENGINES: [&str; 2] = ["codex", "claude"];

/// How soon after the steered run ends the provider's own next turn must
/// start to be that steer made into a turn. The provider starts it at once;
/// a later run is something else (a background task finishing, say).
pub(crate) const STEER_RESTART_WINDOW_MS: u64 = 30_000;

/// A run's own edits name at most this many files; past it, which files the
/// run touched is unknown.
const MAX_RUN_PATHS: usize = 1_000;

/// What one event did to a session's run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RunChange {
    Started,
    Ended(RunOutcome),
}

/// One run-tap observation for the worker.
pub(crate) struct RunOp {
    pub(crate) key: String,
    pub(crate) at: u64,
    pub(crate) change: RunChange,
    /// The session's run generation when the event was seen. A send this
    /// host records opens the next one, so the worker drops an op that
    /// reaches it after a newer send's turn began (a stale end would
    /// otherwise finish that turn).
    pub(crate) generation: u64,
    /// For an end: the files the run's own edits named (`None`: unknown).
    pub(crate) touched: Option<BTreeSet<String>>,
}

/// The files a session's own edits named, for its open run and the run it
/// last ended: each diff its edits produced, and each edit tool's own file
/// (`MultiEdit` and `NotebookEdit` produce no diff). A subagent's edits are
/// the session's own. `None` is unknown: a run whose start the tap never saw,
/// or one whose edits named more than [`MAX_RUN_PATHS`] files.
#[derive(Debug, Default)]
pub(crate) struct RunPaths {
    open: Option<BTreeSet<String>>,
    ended: Option<BTreeSet<String>>,
}

impl RunPaths {
    fn start(&mut self) {
        self.open = Some(BTreeSet::new());
    }

    fn note(&mut self, path: &str) {
        if let Some(paths) = &mut self.open
            && !paths.contains(path)
        {
            if paths.len() >= MAX_RUN_PATHS {
                self.open = None;
            } else {
                paths.insert(path.to_owned());
            }
        }
    }

    /// The run ended: its files become the last ended run's, and answered.
    fn end(&mut self) -> Option<BTreeSet<String>> {
        self.ended = self.open.take();
        self.ended.clone()
    }
}

/// The file `event` says the session's own edit touched: a produced diff's,
/// or an edit tool's.
fn edited_path(event: &EngineEventKind) -> Option<&str> {
    let path = match event {
        EngineEventKind::DiffProduced { path, .. } => path.as_str(),
        EngineEventKind::ToolCall { tool, input, .. } => {
            let field = match tool.as_str() {
                "Edit" | "MultiEdit" | "Write" => "file_path",
                "NotebookEdit" => "notebook_path",
                _ => return None,
            };
            input.get(field)?.as_str()?
        }
        _ => return None,
    };
    (!path.is_empty()).then_some(path)
}

/// A send steered into a running turn. It opened no turn; if the provider
/// starts a new turn with it after the run ends, the tap records that turn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingSteer {
    prompt: String,
    /// When the run it was steered into ended.
    ended_at: Option<u64>,
}

/// A session's run as the tap sees it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RunTrack {
    open: bool,
    /// The provider did something since the run started.
    activity: bool,
    /// The owner asked to stop it.
    pub(crate) interrupt: bool,
    /// Which send or launch this host last opened a run for. A run the
    /// provider starts on its own keeps it; runs ending keep it too.
    generation: u64,
    /// The generation in which the provider last answered (text, thinking,
    /// a tool, an approval): a send refused after its provider answered in
    /// its generation had run.
    answered: u64,
    /// When the run the tap last saw end ended, and how, while no run has
    /// started since. A launch run can end before its bind records turn 0
    /// (a provider that fails within milliseconds); the bind finishes the
    /// turn with it.
    ended: Option<(u64, RunOutcome)>,
    /// The run stopped on an approval (`false`) or a question (`true`) and
    /// nothing has happened since (team tools' `needsYou`).
    waiting: Option<bool>,
    /// Ends of this generation's runs the tap has queued for the worker and
    /// the worker has not yet recorded. While one is on its way the record
    /// still reads as running though nothing runs, and a reconcile must not
    /// take that for a run cut off (it would file it as stopped).
    ending: u32,
}

impl RunTrack {
    /// A run this host started (a send or a launch): open, nothing seen yet.
    pub(crate) const fn opened(generation: u64) -> Self {
        Self {
            open: true,
            activity: false,
            interrupt: false,
            generation,
            answered: 0,
            ended: None,
            waiting: None,
            ending: 0,
        }
    }

    pub(crate) const fn is_open(self) -> bool {
        self.open
    }

    /// Closes the run, keeping its generation, when it last answered and
    /// the ends still on their way.
    fn close(&mut self) {
        *self = Self {
            generation: self.generation,
            answered: self.answered,
            ending: self.ending,
            ..Self::default()
        };
    }

    fn start(&mut self) -> Option<RunChange> {
        if self.open {
            return None;
        }
        self.open = true;
        self.ended = None;
        Some(RunChange::Started)
    }

    fn end(&mut self, outcome: RunOutcome) -> RunChange {
        let outcome = if self.interrupt {
            RunOutcome::Interrupted
        } else {
            outcome
        };
        self.close();
        RunChange::Ended(outcome)
    }

    /// The run change `event` makes. Working, waiting, a user message or an
    /// approval starts a run; output and tool activity starts one too and
    /// marks it active (thinking that stops only marks an open run active). Idle ends a run only once it was active or asked to
    /// stop: a freshly resumed session may announce its idle state after a
    /// send already started the run. Failure ends it as failed, a close as
    /// interrupted, and an interrupt the owner asked for wins over both.
    pub(crate) fn observe(&mut self, event: &EngineEventKind) -> Option<RunChange> {
        match event {
            EngineEventKind::ApprovalRequested { tool, .. } => {
                self.waiting = Some(tool == "AskUserQuestion");
            }
            EngineEventKind::StatusChanged { .. }
            | EngineEventKind::ToolResult { .. }
            | EngineEventKind::MessageDelta { .. }
            | EngineEventKind::SessionClosed => self.waiting = None,
            _ => {}
        }
        match event {
            EngineEventKind::ApprovalRequested { .. } => {
                self.answered = self.generation;
                self.start()
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Working | SessionStatus::Waiting,
            }
            | EngineEventKind::UserMessage { .. } => self.start(),
            EngineEventKind::MessageDelta { .. }
            | EngineEventKind::ThinkingSignal { active: true, .. }
            | EngineEventKind::ToolCall { .. }
            | EngineEventKind::ToolResult { .. }
            | EngineEventKind::DiffProduced { .. }
            | EngineEventKind::PeerResult { .. } => {
                let change = self.start();
                self.activity = true;
                self.answered = self.generation;
                change
            }
            // Thinking that stops is a run's activity, but never the start
            // of one: every Codex turn ends with an inactive signal after its
            // idle, which would otherwise open a run nothing ends.
            EngineEventKind::ThinkingSignal { active: false, .. } if !self.open => None,
            EngineEventKind::ThinkingSignal { .. }
            | EngineEventKind::Notice { .. }
            | EngineEventKind::TurnRetry { .. }
            | EngineEventKind::Error {
                retryable: true, ..
            } => {
                let change = self.start();
                self.activity = true;
                change
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } => (self.open && (self.activity || self.interrupt))
                .then(|| self.end(RunOutcome::Completed)),
            EngineEventKind::StatusChanged {
                status: SessionStatus::Failed,
            }
            | EngineEventKind::Error {
                retryable: false, ..
            } => self.open.then(|| self.end(RunOutcome::Failed)),
            EngineEventKind::SessionClosed => self.open.then(|| self.end(RunOutcome::Interrupted)),
            _ => None,
        }
    }
}

/// Rung by the shared registry coalescer after each round, so the thread
/// host reconciles against every registry change the desktop is told about.
#[derive(Default)]
pub struct ThreadRegistrySignal(Notify);

impl ThreadRegistrySignal {
    pub(crate) async fn changed(&self) {
        self.0.notified().await;
    }
}

impl RegistryRoundObserver for ThreadRegistrySignal {
    fn registry_round(&self) {
        self.0.notify_one();
    }
}

/// Watches every live session's events in the pump's observer chain: keeps
/// each run's state inline and hands starts and ends to the thread worker,
/// so the pump never waits on the store or git.
pub struct ThreadRunTap {
    host: ThreadHost,
    next: Arc<dyn SessionEventObserver>,
}

impl ThreadRunTap {
    pub(crate) fn new(host: ThreadHost, next: Arc<dyn SessionEventObserver>) -> Self {
        Self { host, next }
    }
}

impl SessionEventObserver for ThreadRunTap {
    fn observe_session_event(&self, session_key: &str, event: &EngineEvent) {
        self.next.observe_session_event(session_key, event);
        let change = self.host.observe_run(session_key, &event.event);
        // A rate-limit notice records the thread's limit; a completed run
        // clears it (protocol 66).
        self.host.observe_limit(session_key, &event.event, change);
        // The journal sees each event with the run change it made, in pump
        // order, over the same one channel.
        self.host.journal_event(session_key, &event.event, change);
    }
}

impl ThreadHost {
    /// Feeds one event to the run tap; a start or end goes to the worker,
    /// and is answered for the journal.
    pub(crate) fn observe_run(&self, key: &str, event: &EngineEventKind) -> Option<RunChange> {
        // Inline on every event (text deltas included): no allocation unless
        // a session's run state is new.
        let change = {
            let mut runs = lock(&self.inner.runs);
            let (change, generation) = if let Some(track) = runs.get_mut(key) {
                (track.observe(event), track.generation)
            } else {
                let mut track = RunTrack::default();
                let change = track.observe(event);
                if track != RunTrack::default() {
                    runs.insert(key.to_owned(), track);
                }
                (change, 0)
            };
            if matches!(event, EngineEventKind::SessionClosed) {
                // The track stays (closed) for its generation.
                lock(&self.inner.steers).remove(key);
            }
            // Under `runs`, so the files a run's edits named are its own: a
            // start clears them before the event that started it is noted,
            // and an end takes them with the run.
            let edited = edited_path(event);
            let mut touched = None;
            if change.is_some() || edited.is_some() {
                let mut paths = lock(&self.inner.run_paths);
                let paths = paths.entry(key.to_owned()).or_default();
                if change == Some(RunChange::Started) {
                    paths.start();
                }
                if let Some(path) = edited {
                    paths.note(path);
                }
                if matches!(change, Some(RunChange::Ended(_))) {
                    touched = paths.end();
                }
            }
            let at = change.map(|_| self.now());
            if let (Some(RunChange::Ended(outcome)), Some(at)) = (change, at)
                && let Some(track) = runs.get_mut(key)
            {
                track.ended = Some((at, outcome));
                // Under `runs`, in the same step that closed the run: no
                // reconcile sees it closed and its end not on its way.
                track.ending = track.ending.saturating_add(1);
            }
            if let (Some(RunChange::Ended(_)), Some(at)) = (change, at)
                && let Some(steer) = lock(&self.inner.steers).get_mut(key)
            {
                // Under `runs`, so a steer is either recorded before this end
                // (and stamped here) or finds the run already over.
                steer.ended_at.get_or_insert(at);
            }
            change
                .zip(at)
                .map(|(change, at)| (change, at, generation, touched))
        };
        if change.is_some()
            || matches!(
                event,
                EngineEventKind::ApprovalRequested { .. } | EngineEventKind::StatusChanged { .. }
            )
        {
            self.team_changed();
        }
        // Protocol 67: which run asked for a secret, and a run's end
        // withdrawing the one it asked for.
        self.secret_observe(
            key,
            event,
            matches!(change, Some((RunChange::Ended(_), ..))),
        );
        let (change, at, generation, touched) = change?;
        let sent = self.inner.ops.send(RunOp {
            key: key.to_owned(),
            at,
            change,
            generation,
            touched,
        });
        if sent.is_err() && matches!(change, RunChange::Ended(_)) {
            // No worker will record it (shutdown): nothing is on its way.
            self.end_recorded(key, generation);
        }
        Some(change)
    }

    /// The worker recorded (or dropped as stale) an end the tap queued for
    /// `key`'s run of `generation`. An end of an older generation counts
    /// for nothing: a newer send's run replaced the one it ended.
    pub(crate) fn end_recorded(&self, key: &str, generation: u64) {
        if let Some(track) = lock(&self.inner.runs)
            .get_mut(key)
            .filter(|track| track.generation == generation)
        {
            track.ending = track.ending.saturating_sub(1);
        }
    }

    /// The files `key`'s last ended run's own edits named (`None`: unknown).
    pub(crate) fn ended_run_paths(&self, key: &str) -> Option<BTreeSet<String>> {
        lock(&self.inner.run_paths).get(key)?.ended.clone()
    }

    /// Records `prompt` as steered into `key`'s running turn when its engine
    /// folds such a send into the turn and the turn is running by both the
    /// tap and the registry. Answers whether it did; a send it does not
    /// record opens a turn of its own.
    fn steer_into_running_turn(&self, key: &str, prompt: &str) -> bool {
        let folds =
            parse_key(key).is_ok_and(|(engine, _)| STEERING_ENGINES.contains(&engine.as_str()));
        let working = folds
            && self
                .inner
                .ports
                .sessions
                .session(key)
                .ok()
                .flatten()
                .is_some_and(|summary| {
                    matches!(
                        summary.status,
                        SessionStatus::Working | SessionStatus::Waiting
                    )
                });
        if !working {
            return false;
        }
        let runs = lock(&self.inner.runs);
        if !runs.get(key).is_some_and(|track| track.is_open()) {
            return false;
        }
        // The first steer of a run is the one a restart would carry.
        lock(&self.inner.steers)
            .entry(key.to_owned())
            .or_insert_with(|| PendingSteer {
                prompt: prompt.to_owned(),
                ended_at: None,
            });
        true
    }

    /// The steered prompt a run starting at `at` is, when the run it was
    /// steered into ended within [`STEER_RESTART_WINDOW_MS`]. A steer whose
    /// run ended longer ago was folded into it, and is dropped.
    pub(crate) fn take_restarted_steer(&self, key: &str, at: u64) -> Option<String> {
        let mut steers = lock(&self.inner.steers);
        let ended = steers.get(key)?.ended_at?;
        let steer = steers.remove(key)?;
        (at.saturating_sub(ended) <= STEER_RESTART_WINDOW_MS).then_some(steer.prompt)
    }

    fn drop_steer(&self, key: &str) {
        lock(&self.inner.steers).remove(key);
    }

    /// Marks `key`'s run as started by this host, in a new generation, and
    /// answers it. Called under the store lock in the same edit that records
    /// the run, so a reconcile never sees the record running while the tap
    /// does not, and the worker never applies an older op after it.
    pub(crate) fn open_run(&self, key: &str) -> u64 {
        let generation = self.inner.run_generations.fetch_add(1, Ordering::Relaxed) + 1;
        let mut runs = lock(&self.inner.runs);
        runs.insert(key.to_owned(), RunTrack::opened(generation));
        lock(&self.inner.run_paths)
            .entry(key.to_owned())
            .or_default()
            .start();
        generation
    }

    pub(crate) fn close_run(&self, key: &str) {
        if let Some(track) = lock(&self.inner.runs).get_mut(key) {
            track.close();
        }
    }

    /// Closes `key`'s run when it is still the one `generation` opened.
    pub(super) fn close_run_of(&self, key: &str, generation: u64) {
        if let Some(track) = lock(&self.inner.runs)
            .get_mut(key)
            .filter(|track| track.generation == generation)
        {
            track.close();
        }
    }

    /// The generation of `key`'s last run this host opened (0 for none).
    pub(crate) fn run_generation(&self, key: &str) -> u64 {
        lock(&self.inner.runs)
            .get(key)
            .map_or(0, |track| track.generation)
    }

    /// Whether the provider answered in the run `generation` opened.
    fn answered_in(&self, key: &str, generation: u64) -> bool {
        lock(&self.inner.runs)
            .get(key)
            .is_some_and(|track| track.answered == generation)
    }

    /// When and how `key`'s last run ended, while the tap holds no open
    /// run for it and none started since.
    pub(crate) fn run_ended_untracked(&self, key: &str) -> Option<(u64, RunOutcome)> {
        lock(&self.inner.runs)
            .get(key)
            .filter(|track| !track.is_open())
            .and_then(|track| track.ended)
    }

    /// Whether `key`'s open run stopped on a question (`Some(true)`) or an
    /// approval (`Some(false)`) and nothing has happened since.
    pub(crate) fn run_waiting(&self, key: &str) -> Option<bool> {
        lock(&self.inner.runs)
            .get(key)
            .filter(|track| track.is_open())
            .and_then(|track| track.waiting)
    }

    /// Whether the tap holds an open run for `key`.
    pub(crate) fn run_tracked(&self, key: &str) -> bool {
        lock(&self.inner.runs)
            .get(key)
            .is_some_and(|track| track.is_open())
    }

    /// Whether the host still accounts for `key`'s run: the tap holds it
    /// open, or the tap saw it end and the worker has not recorded that end
    /// yet. Reconcile closes as interrupted only a running record the host
    /// does not account for (a run cut off by a restart or a vanished
    /// session).
    pub(crate) fn run_accounted(&self, key: &str) -> bool {
        lock(&self.inner.runs)
            .get(key)
            .is_some_and(|track| track.is_open() || track.ending > 0)
    }

    pub(super) fn send_gate(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = lock(&self.inner.send_gates);
        gates.retain(|_, gate| Arc::strong_count(gate) > 1);
        Arc::clone(gates.entry(key.to_owned()).or_default())
    }

    /// Recreates a missing worktree from its branch.
    async fn ensure_worktree(
        &self,
        git_root: &str,
        path: &str,
        branch: Option<&str>,
    ) -> Result<(), DispatchError> {
        let unavailable = |detail: String| {
            DispatchError::new(
                THREADS_WORKTREE_UNAVAILABLE_CODE,
                format!(
                    "This thread's worktree at {path} is missing and could not be recreated: {detail} Nothing was sent."
                ),
                false,
                true,
            )
        };
        let Some(branch) = branch.map(str::to_owned) else {
            return Err(unavailable("it has no branch to recreate it from.".into()));
        };
        let git = Arc::clone(&self.inner.ports.git);
        let (root, worktree) = (git_root.to_owned(), path.to_owned());
        tokio::task::spawn_blocking(move || {
            git.ensure_worktree(Path::new(&root), Path::new(&worktree), &branch)
        })
        .await
        .map_err(|_| unavailable("the git task stopped.".into()))?
        .map(|_| ())
        .map_err(|error| unavailable(error.message))
    }

    /// Captures `refname` in `dir` within [`START_CAPTURE_LIMIT`]. Any skip,
    /// failure or overrun answers `None` and the send goes on without it.
    pub(super) async fn capture_start(&self, dir: String, refname: String) -> Option<String> {
        let git = Arc::clone(&self.inner.ports.git);
        let capture = tokio::task::spawn_blocking(move || {
            git.capture_checkpoint(Path::new(&dir), &refname)
                .map(|outcome| (outcome, refname))
        });
        match tokio::time::timeout(START_CAPTURE_LIMIT, capture).await {
            Ok(Ok(Ok((CaptureOutcome::Captured { .. }, refname)))) => Some(refname),
            Ok(Ok(Ok((CaptureOutcome::Skipped { reason }, _)))) => {
                tracing::info!(%reason, "a thread turn starts without a checkpoint");
                None
            }
            Ok(Ok(Err(error))) => {
                tracing::warn!(%error, "a thread turn's start checkpoint failed");
                None
            }
            Ok(Err(_)) | Err(_) => {
                tracing::warn!("a thread turn's start checkpoint did not finish in time");
                None
            }
        }
    }
}

/// What `before_send` needs from a thread's record.
struct SendPlan {
    ordinal: u32,
    dir: Option<String>,
    git_root: Option<String>,
    missing_worktree: Option<(String, Option<String>)>,
}

#[async_trait]
impl SendHooks for ThreadHost {
    /// For a thread: recreate its missing worktree (refusing the send when
    /// that fails), checkpoint the turn's start, and record the turn and the
    /// run's start. A send its provider folds into the running turn records
    /// nothing and keeps that turn running. Sessions without a thread record
    /// and an unavailable store pass through untouched: thread bookkeeping
    /// never blocks a send.
    async fn before_send(
        &self,
        session_key: &str,
        text: &str,
    ) -> Result<Option<Box<dyn SendTicket>>, DispatchError> {
        let gate = self.send_gate(session_key).lock_owned().await;
        // A prompt Interface sends itself (team tools: a started thread's
        // task, a message to it, a finished thread's notice) is shown in the
        // pane and journalled as the host's.
        let host_prompt = self.take_host_prompt(session_key, text);
        // A thread an Away loop holds takes only the loop's own prompt.
        let away = self.away_send(session_key, text)?;
        let key = session_key.to_owned();
        let plan = self
            .peek(move |document, _| {
                let record = document.record(&key)?;
                let workspace = record.workspace.as_ref();
                let missing_worktree = workspace
                    .filter(|workspace| workspace.mode == WorkspaceMode::Worktree)
                    .and_then(|workspace| workspace.worktree_path.clone())
                    .filter(|path| super::worktree_missing(path))
                    .map(|path| (path, workspace.and_then(|w| w.branch.clone())));
                Some(SendPlan {
                    ordinal: record
                        .turns
                        .last()
                        .map_or(0, |turn| turn.ordinal.saturating_add(1)),
                    dir: record.workspace_dir().map(str::to_owned),
                    git_root: record.git_root.clone(),
                    missing_worktree,
                })
            })
            .await;
        let plan = match plan {
            Ok(Some(plan)) => plan,
            Ok(None) => return Ok(None),
            Err(error) => {
                tracing::warn!(code = %error.code, "a send went out without thread bookkeeping");
                return Ok(None);
            }
        };
        if self.steer_into_running_turn(session_key, text) {
            return Ok(Some(Box::new(SteerTicket {
                host: self.clone(),
                key: session_key.to_owned(),
                prompt: text.to_owned(),
                _gate: gate,
            })));
        }
        // A prompt that opens its own turn supersedes a steer still pending.
        self.drop_steer(session_key);
        let mut start_ref = None;
        if let (Some(git_root), Some(dir)) = (&plan.git_root, &plan.dir) {
            if let Some((path, branch)) = &plan.missing_worktree {
                self.ensure_worktree(git_root, path, branch.as_deref())
                    .await?;
            }
            start_ref = self
                .capture_start(dir.clone(), turn_ref(session_key, plan.ordinal, false))
                .await;
        }
        let key = session_key.to_owned();
        let prompt = text.to_owned();
        let host = self.clone();
        let begun = self
            .edit_quiet(move |document, now| {
                let begun = document.begin_turn(&key, &prompt, start_ref, now)?;
                Ok((begun, now, host.open_run(&key)))
            })
            .await;
        match begun {
            Ok((begun, started_at, generation)) => {
                if away {
                    self.away_turn_begun(session_key, begun.ordinal);
                }
                // Before the send goes out, so the prompt precedes its answer.
                let origin = if away || host_prompt {
                    TurnOrigin::Host
                } else {
                    TurnOrigin::Owner
                };
                self.journal_prompt(session_key, origin, text);
                Ok(Some(Box::new(TurnTicket {
                    host: self.clone(),
                    key: session_key.to_owned(),
                    begun,
                    started_at,
                    generation,
                    gate,
                    echo: away || host_prompt,
                })))
            }
            Err(error) => {
                // The record did not change, so neither does the tap.
                self.close_run(session_key);
                tracing::warn!(code = %error.code, "a send went out without its thread turn");
                Ok(None)
            }
        }
    }

    fn interrupt_requested(&self, session_key: &str) {
        if let Some(track) = lock(&self.inner.runs).get_mut(session_key) {
            track.interrupt = track.open;
        }
        // A stop takes a steer down with the turn (Claude cancels its queue).
        self.drop_steer(session_key);
    }
}

/// A send steered into a running turn, waiting on its delivery. A steer that
/// never reached the provider is forgotten; one it took is journalled as a
/// turn of origin `steer` (the running turn's entries before it stay where
/// they are). The thread's send gate is held until then.
struct SteerTicket {
    host: ThreadHost,
    key: String,
    prompt: String,
    _gate: OwnedMutexGuard<()>,
}

impl SendTicket for SteerTicket {
    fn settle(self: Box<Self>, settlement: SendSettlement) {
        if settlement == SendSettlement::Delivered {
            self.host
                .journal_prompt(&self.key, TurnOrigin::Steer, &self.prompt);
        } else {
            self.host.drop_steer(&self.key);
        }
    }
}

/// A recorded turn waiting on its send's delivery. A send that never reached
/// the provider takes the turn back, and so does one the provider refused
/// without answering; a refused send the provider had already answered (an
/// ACP prompt fails only when its turn ends) ran, and keeps its turn. The
/// thread's send gate is held until then.
struct TurnTicket {
    host: ThreadHost,
    key: String,
    begun: BegunTurn,
    started_at: u64,
    generation: u64,
    gate: OwnedMutexGuard<()>,
    /// An Away loop's own prompt, or another Interface sent itself: no
    /// window echoes it.
    echo: bool,
}

impl SendTicket for TurnTicket {
    fn echoes_prompt(&self) -> bool {
        self.echo
    }

    fn settle(self: Box<Self>, settlement: SendSettlement) {
        let Self {
            host,
            key,
            begun,
            started_at,
            generation,
            gate,
            echo: _,
        } = *self;
        let take_back = match settlement {
            SendSettlement::Delivered => false,
            SendSettlement::Unsent => true,
            SendSettlement::Rejected => !host.answered_in(&key, generation),
        };
        if !take_back {
            // A thread created with no prompt (one team tools or an outside
            // agent started): its first prompt is out, so it is named now.
            if begun.ordinal == 0 {
                host.title_at_first_prompt(&key);
            }
            return;
        }
        // The journal drops the turn too, in order with its events.
        host.journal_retract(&key);
        let task = host.clone();
        host.spawn_tracked(async move {
            let undo = key.clone();
            let tap = task.clone();
            if let Err(error) = task
                .edit_quiet(move |document, _| {
                    // Together, under the store lock: a reconcile never sees
                    // the turn running with its run closed.
                    tap.close_run_of(&undo, generation);
                    document.cancel_turn(&undo, &begun, started_at);
                    Ok(())
                })
                .await
            {
                tracing::warn!(code = %error.code, "an unsent thread turn could not be taken back");
            }
            drop(gate);
        });
    }
}
