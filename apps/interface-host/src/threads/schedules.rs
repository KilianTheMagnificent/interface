//! Scheduled prompts and pull-request watch (T3 section 7, protocol 62).
//!
//! An adapter over `interface_settings::thread_schedules`, which holds every
//! rule (due times, the missed-run policy, the delivery gate, fingerprints,
//! the prompts' wording) and `thread-schedules.v2.json`. This module drives
//! them: the thread worker wakes it at the next due time (at least once a
//! minute while anything is scheduled, so a Mac that slept catches up), a
//! pass turns due schedules into firings that wait for their thread, and
//! each waiting firing is delivered through the same gate as the phone's
//! send: an idle or failed thread that no Away loop or Code pane holds and
//! that is not settled or archived. A busy thread is waited for until its
//! run ends.
//!
//! Fail closed, as every provider effect: a firing is recorded as an
//! attempt (outcome unknown) BEFORE its send, through the guarded runtime
//! as a host prompt (shown folded in the pane, journalled as the host's);
//! then its outcome is recorded. A send whose outcome is unknown stays
//! unknown and is never tried again, and a restart never offers an
//! attempted occurrence again. `gh` reads for the watch run off the worker,
//! on the pull-request pass, through `interface_review`'s `Gh` (its
//! environment is credential-filtered).

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use interface_common::sync::lock;
use interface_contracts::{
    ipc::CHANNELS,
    threads::{
        CheckBucket, PullRequestState, ScheduleCalendar, ScheduleKind, ScheduleOutcome,
        ScheduleRun, ScheduleSetBy, ScheduleWhen, ThreadCommand, ThreadControlResponse,
        ThreadsView, WorkspaceMode,
    },
};
use interface_review::thread_git::ship::{GhAuth, GhPullRequest, checks_summary};
use interface_settings::thread_schedules::{
    self, Gate, GateFacts, NewSchedule, ScheduleError, ScheduleRefusal, ThreadScheduleStore,
};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{THREADS_NO_PULL_REQUEST_CODE, ThreadHost, worktree_missing};
use crate::dispatcher::DispatchError;

/// The schedules store did not open this launch. Definitely unsent.
pub const THREADS_SCHEDULES_UNAVAILABLE_CODE: &str = "threads-schedules-unavailable";
/// A schedule change could not be saved.
const THREADS_SCHEDULES_FAILED_CODE: &str = "threads-schedules-failed";
/// While anything is scheduled the worker looks at least this often, so a
/// due time passed while the Mac slept (when the worker's timer stops) is
/// noticed within a minute of waking.
const SCHEDULE_TICK: Duration = Duration::from_secs(60);

pub(crate) struct ScheduleParts {
    store: Arc<Mutex<Result<ThreadScheduleStore, String>>>,
    /// A pass is running.
    pass: AtomicBool,
    /// Something asked for a pass while one ran: it runs again.
    again: AtomicBool,
}

/// What one look at a watched pull request read.
enum WatchRead {
    NeedsGh,
    Failed(String),
    Closed(GhPullRequest),
    Open {
        gh: interface_review::thread_git::ship::Gh,
        dir: String,
        pull_request: GhPullRequest,
        checks: Vec<interface_review::thread_git::ship::GhCheck>,
        comments: Vec<interface_contracts::threads::ReviewComment>,
    },
}

fn schedule_error(error: ScheduleError) -> DispatchError {
    match error {
        ScheduleError::Refused(refusal) => {
            DispatchError::new(refusal.code, refusal.message, false, true)
        }
        ScheduleError::Storage(error) => {
            let unsent = matches!(
                error,
                interface_settings::SettingsError::InvalidAgent(_)
                    | interface_settings::SettingsError::Storage(
                        interface_storage::StorageError::WriteFailed { .. }
                            | interface_storage::StorageError::BoundsExceeded { .. }
                    )
            );
            tracing::error!(%error, "scheduled prompts could not be saved");
            DispatchError::new(
                THREADS_SCHEDULES_FAILED_CODE,
                format!("Scheduled prompts could not be saved: {error}. Try again."),
                false,
                unsent,
            )
        }
    }
}

fn refusal(refusal: ScheduleRefusal) -> DispatchError {
    DispatchError::new(refusal.code, refusal.message, false, true)
}

fn unavailable(reason: &str) -> DispatchError {
    DispatchError::new(
        THREADS_SCHEDULES_UNAVAILABLE_CODE,
        format!(
            "Scheduled prompts could not be opened ({reason}). Existing data was kept; restart Interface to try again."
        ),
        false,
        true,
    )
}

/// A new schedule id: `s-` and twelve hex digits.
pub(super) fn new_id() -> String {
    format!("s-{}", &uuid::Uuid::new_v4().simple().to_string()[..12])
}

impl ThreadHost {
    /// Opens `thread-schedules.v2.json` in the private data root (carrying a
    /// `thread-schedules.v1.json` over). A file that does not decode is
    /// quarantined and the store starts empty.
    ///
    /// # Errors
    /// Why the store could not open (every schedule command then refuses).
    pub fn open_schedule_store(data_root: &Path) -> Result<ThreadScheduleStore, String> {
        ThreadScheduleStore::open(&data_root.join(thread_schedules::THREAD_SCHEDULES_FILE_NAME))
            .map_err(|error| {
                tracing::error!(%error, "scheduled prompts did not open; none will run");
                error.to_string()
            })
    }

    /// Attaches the schedules store. Attaching twice keeps the first.
    pub fn attach_schedules(&self, store: Result<ThreadScheduleStore, String>) {
        let _ = self.inner.schedules.set(ScheduleParts {
            store: Arc::new(Mutex::new(store)),
            pass: AtomicBool::new(false),
            again: AtomicBool::new(false),
        });
    }

    fn schedule_parts(&self) -> Result<&ScheduleParts, DispatchError> {
        self.inner
            .schedules
            .get()
            .ok_or_else(|| unavailable("not attached"))
    }

    /// Reads the store in memory (no I/O).
    pub(super) fn schedules_peek<T>(
        &self,
        read: impl FnOnce(&ThreadScheduleStore) -> T,
    ) -> Option<T> {
        let parts = self.inner.schedules.get()?;
        let store = lock(&parts.store);
        store.as_ref().ok().map(read)
    }

    /// Runs `work` on the store on the blocking pool (a commit fsyncs).
    pub(super) async fn schedules_write<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut ThreadScheduleStore) -> Result<T, ScheduleError> + Send + 'static,
    ) -> Result<T, DispatchError> {
        let store = Arc::clone(&self.schedule_parts()?.store);
        tokio::task::spawn_blocking(move || {
            let mut store = lock(&store);
            match store.as_mut() {
                Ok(store) => work(store).map_err(schedule_error),
                Err(reason) => Err(unavailable(reason)),
            }
        })
        .await
        .map_err(|_| super::internal("The scheduled prompts task stopped unexpectedly."))?
    }

    /// Puts each thread's schedules and watch on `view` (paused while it is
    /// settled).
    pub(super) fn with_schedules(&self, view: &mut ThreadsView) {
        let _ = self.schedules_peek(|store| {
            for thread in &mut view.threads {
                thread.schedules = store.view(&thread.key, thread.settled_at.is_some());
            }
        });
    }

    /// Pushes the current view after a schedules change, holding the thread
    /// store's lock so pushes leave in order, and wakes the worker. Answers
    /// the view.
    pub(super) async fn push_schedules(&self) -> Result<ThreadsView, DispatchError> {
        let guard = self.inner.store.lock().await;
        let store = guard
            .as_ref()
            .map_err(|reason| super::store_unavailable(reason))?;
        let now = self.now();
        let document = store.document().clone();
        let mut view = tokio::task::spawn_blocking(move || document.view(now, worktree_missing))
            .await
            .map_err(|_| super::internal("The thread store task stopped unexpectedly."))?;
        self.with_away(&mut view);
        self.with_hidden(&mut view);
        self.with_accounts(&mut view);
        self.with_schedules(&mut view);
        self.with_outside(&mut view);
        self.with_limits(&mut view);
        self.with_secrets(&mut view);
        self.with_sides(&mut view);
        self.with_folder_trust(&mut view);
        self.push_view(&view).await;
        drop(guard);
        self.inner.timers.notify_one();
        Ok(view)
    }

    pub(super) async fn pushed_quietly(&self) {
        if let Err(error) = self.push_schedules().await {
            tracing::warn!(code = %error.code, "a scheduled prompt's change was not pushed");
        }
    }

    // --- Commands ---------------------------------------------------------------

    pub(super) async fn schedule_command(
        &self,
        command: ThreadCommand,
    ) -> Result<ThreadControlResponse, DispatchError> {
        match command {
            ThreadCommand::ScheduleSet {
                key,
                id,
                name,
                prompt,
                when,
            } => {
                let key = self.listed_thread(&key).await?;
                self.set_schedule(key, id, name, prompt, when, ScheduleSetBy::Owner)
                    .await?;
            }
            ThreadCommand::ScheduleDelete { id } => {
                self.schedules_write(move |store| store.delete(&id)).await?;
            }
            ThreadCommand::ScheduleEnable { id, enabled } => {
                let now = self.now();
                self.schedules_write(move |store| store.set_enabled(&id, enabled, now))
                    .await?;
            }
            ThreadCommand::WatchSet { key, enabled } => {
                let key = self.listed_thread(&key).await?;
                if enabled {
                    let target = key.clone();
                    let open = self
                        .peek(move |document, _| {
                            document
                                .record(&target)
                                .and_then(|record| record.pull_request.as_ref())
                                .is_some_and(|pr| pr.state == PullRequestState::Open)
                        })
                        .await?;
                    if !open {
                        return Err(DispatchError::new(
                            THREADS_NO_PULL_REQUEST_CODE,
                            "This thread has no open pull request to watch.",
                            false,
                            true,
                        ));
                    }
                }
                let target = key.clone();
                self.schedules_write(move |store| store.set_watch(&target, enabled))
                    .await?;
                if enabled {
                    // The first look now: a check failing right now is sent once.
                    let host = self.clone();
                    self.spawn_tracked(async move { host.watch_one(&key).await });
                }
            }
            other => {
                return Err(super::internal(&format!(
                    "The thread command {} is not a schedule command.",
                    other.kind()
                )));
            }
        }
        Ok(self.push_schedules().await?.into())
    }

    /// `key`'s thread (its head when `key` is an earlier segment), which must
    /// be listed.
    pub(super) async fn listed_thread(&self, key: &str) -> Result<String, DispatchError> {
        let head = self.head_key(key).await;
        let target = head.clone();
        let listed = self
            .peek(move |document, _| {
                document
                    .record(&target)
                    .is_some_and(interface_settings::threads::ThreadRecord::listed)
            })
            .await?;
        if !listed {
            return Err(super::refused(
                interface_settings::threads::ThreadRefusal::unknown(),
            ));
        }
        Ok(head)
    }

    /// Adds a schedule to `key` (or changes schedule `id` of it). Answers
    /// its id and first due time.
    async fn set_schedule(
        &self,
        key: String,
        id: Option<String>,
        name: Option<String>,
        prompt: String,
        when: ScheduleWhen,
        set_by: ScheduleSetBy,
    ) -> Result<(String, u64), DispatchError> {
        let now = self.now();
        let name = name.filter(|name| !name.trim().is_empty());
        let first =
            thread_schedules::validate(&prompt, name.as_deref(), &when, now).map_err(refusal)?;
        let answer = self
            .schedules_write(move |store| {
                if let Some(id) = id {
                    return store
                        .update(&id, &key, name, prompt, when, first)
                        .map(|()| id);
                }
                let id = new_id();
                store
                    .add(
                        NewSchedule {
                            id: id.clone(),
                            thread_key: key,
                            name,
                            prompt,
                            when,
                            set_by,
                            kind: ScheduleKind::Prompt,
                            receipt: None,
                        },
                        first,
                        now,
                    )
                    .map(|()| id)
            })
            .await?;
        Ok((answer, first))
    }

    // --- The driver ---------------------------------------------------------------

    /// When the worker should look at the schedules next: the soonest due
    /// time, and at least every minute while anything is scheduled or waits.
    pub(super) fn next_schedule_wake(&self) -> Option<Instant> {
        let (next, waiting) =
            self.schedules_peek(|store| (store.next_wake(), store.has_waiting()))?;
        let now = self.now();
        let tick = Instant::now() + SCHEDULE_TICK;
        match next {
            Some(at) => {
                Some((Instant::now() + Duration::from_millis(at.saturating_sub(now))).min(tick))
            }
            None => waiting.then_some(tick),
        }
    }

    /// Whether a firing waits for a thread (the worker then runs a pass when
    /// a run ends).
    pub(super) fn schedules_waiting(&self) -> bool {
        self.schedules_peek(ThreadScheduleStore::has_waiting)
            .unwrap_or(false)
    }

    /// Runs a pass off the worker; one at a time, and once more when asked
    /// while one runs.
    pub(crate) fn spawn_schedule_pass(&self) {
        let Ok(parts) = self.schedule_parts() else {
            return;
        };
        if parts.pass.swap(true, Ordering::AcqRel) {
            parts.again.store(true, Ordering::Release);
            return;
        }
        let host = self.clone();
        self.spawn_tracked(async move {
            loop {
                host.run_schedules().await;
                let Ok(parts) = host.schedule_parts() else {
                    return;
                };
                if !parts.again.swap(false, Ordering::AcqRel) {
                    parts.pass.store(false, Ordering::Release);
                    // A request between the swap and the store would be lost.
                    if parts.again.swap(false, Ordering::AcqRel)
                        && !parts.pass.swap(true, Ordering::AcqRel)
                    {
                        continue;
                    }
                    return;
                }
            }
        });
    }

    /// One pass: due schedules fire (or are skipped and recorded), then each
    /// waiting firing and watch prompt is offered to its thread.
    pub(crate) async fn run_schedules(&self) {
        let settled = self
            .peek(|document, _| {
                document
                    .records
                    .iter()
                    .filter(|record| record.settled_at.is_some())
                    .map(|record| record.key.clone())
                    .collect::<std::collections::HashSet<_>>()
            })
            .await
            .unwrap_or_default();
        let now = self.now();
        let advanced = self
            .schedules_write(move |store| store.advance(now, |key| settled.contains(key)))
            .await;
        match advanced {
            Ok(advanced) if !advanced.is_empty() => self.pushed_quietly().await,
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(code = %error.code, "due scheduled prompts could not be recorded");
            }
        }
        let (schedules, watches) = self
            .schedules_peek(|store| {
                (
                    store
                        .schedules()
                        .iter()
                        .filter(|schedule| schedule.pending.is_some())
                        .map(|schedule| schedule.id.clone())
                        .collect::<Vec<_>>(),
                    store
                        .watches()
                        .iter()
                        .filter(|watch| watch.pending.is_some())
                        .map(|watch| watch.thread_key.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .unwrap_or_default();
        for id in schedules {
            self.deliver_schedule(&id).await;
        }
        for key in watches {
            self.deliver_watch(&key).await;
        }
    }

    /// What the gate needs to know about `key`.
    async fn gate_facts(&self, key: &str, waited_ms: u64) -> GateFacts {
        let owned = key.to_owned();
        let record = self
            .peek(move |document, _| document.record(&owned).cloned())
            .await
            .ok()
            .flatten();
        let summary = self.inner.ports.sessions.session(key).ok().flatten();
        GateFacts {
            known: record
                .as_ref()
                .is_some_and(interface_settings::threads::ThreadRecord::listed)
                && summary.is_some(),
            archived: summary.as_ref().is_some_and(|summary| summary.archived),
            settled: record
                .as_ref()
                .is_some_and(|record| record.settled_at.is_some()),
            away: lock(&self.inner.away_holds).contains_key(key),
            pane: self.inner.ports.claims.claim(key).is_some(),
            busy: self.busy(key)
                || record
                    .as_ref()
                    .is_some_and(interface_settings::threads::ThreadRecord::running)
                || self.run_waiting(key).is_some(),
            no_runtime: self.guarded_runtime().is_none(),
            waited_ms,
        }
    }

    /// Sends `text` to `key` through the guarded runtime as a host prompt
    /// (with `recovery`, a prompt-recovery Retry that consumes its receipt).
    /// Answers how it ended: sent, refused (definitely not delivered, with
    /// why) or unknown.
    async fn send_host_prompt(
        &self,
        key: &str,
        text: &str,
        recovery: Option<Value>,
    ) -> (ScheduleOutcome, Option<String>) {
        let Some(runtime) = self.guarded_runtime() else {
            return (
                ScheduleOutcome::Refused,
                Some("Interface could not send prompts yet.".into()),
            );
        };
        self.permit_host_prompt(key, text);
        let mut payload = json!({"sessionKey": key, "text": text});
        if let Some(recovery) = recovery {
            payload["recovery"] = recovery;
        }
        match runtime.dispatch(CHANNELS.send, payload).await {
            Ok(_) => (ScheduleOutcome::Sent, None),
            Err(error) if error.replay_safe => {
                self.take_host_prompt(key, text);
                (ScheduleOutcome::Refused, Some(error.message))
            }
            Err(error) => {
                tracing::warn!(code = %error.code, "a scheduled prompt's send has no known outcome");
                (ScheduleOutcome::Unknown, Some(error.message))
            }
        }
    }

    /// Offers schedule `id`'s waiting firing to its thread (see the module
    /// docs). Answers its recorded run, or `None` while it waits.
    pub(crate) async fn deliver_schedule(&self, id: &str) -> Option<ScheduleRun> {
        let (key, pending) = self.schedules_peek(|store| {
            let schedule = store.schedule(id)?;
            Some((schedule.thread_key.clone(), schedule.pending.clone()?))
        })??;
        // A schedule belongs to the thread: its current head.
        let head = self.head_key(&key).await;
        let now = self.now();
        let facts = self
            .gate_facts(&head, now.saturating_sub(pending.since))
            .await;
        let (outcome, detail) = match thread_schedules::gate(facts) {
            Gate::Wait => return None,
            Gate::Drop { outcome, detail } => (outcome, Some(detail)),
            Gate::Deliver => {
                let owned = id.to_owned();
                let occurrence = pending.occurrence;
                // Recorded before the send: never offered again.
                let schedule = match self
                    .schedules_write(move |store| store.begin_attempt(&owned, occurrence, now))
                    .await
                {
                    Ok(Some(schedule)) => schedule,
                    Ok(None) => return None,
                    Err(error) => {
                        tracing::warn!(code = %error.code, "a scheduled prompt's attempt could not be recorded, so it was not sent");
                        return None;
                    }
                };
                tracing::info!(
                    occurrence = %thread_schedules::occurrence_id(id, occurrence),
                    "sending a scheduled prompt"
                );
                let (text, recovery) = match schedule.kind {
                    ScheduleKind::Prompt => (
                        thread_schedules::scheduled_prompt(
                            schedule.name.as_deref(),
                            &schedule.prompt,
                        ),
                        None,
                    ),
                    // Resume at reset (protocol 66): a prompt the limit
                    // refused goes as a Retry of its receipt, which the
                    // send consumes, so it never goes twice.
                    ScheduleKind::Resume => (
                        thread_schedules::resume_prompt(&schedule.prompt),
                        schedule.receipt.as_ref().map(|id| {
                            json!({
                                "text": schedule.prompt,
                                "context": [],
                                "source": {"id": id, "mode": "retry"},
                            })
                        }),
                    ),
                };
                self.send_host_prompt(&head, &text, recovery).await
            }
        };
        let (owned, occurrence, at) = (id.to_owned(), pending.occurrence, self.now());
        let recorded = detail.clone();
        if let Err(error) = self
            .schedules_write(move |store| {
                store.record(&owned, occurrence, outcome, recorded.as_deref(), at)
            })
            .await
        {
            tracing::warn!(code = %error.code, "a scheduled prompt's outcome could not be recorded");
        }
        self.pushed_quietly().await;
        Some(ScheduleRun {
            occurrence,
            at,
            outcome,
            detail: detail.map(|text| thread_schedules::detail(&text)),
        })
    }

    /// Offers thread `key`'s waiting watch prompt to it, through the same
    /// gate and path as a scheduled prompt.
    pub(crate) async fn deliver_watch(&self, key: &str) {
        let Some(Some(pending)) = self.schedules_peek(|store| {
            store
                .watch(key)
                .filter(|watch| watch.enabled)
                .and_then(|watch| watch.pending.clone())
        }) else {
            return;
        };
        let head = self.head_key(key).await;
        let now = self.now();
        let facts = self
            .gate_facts(&head, now.saturating_sub(pending.since))
            .await;
        let (outcome, detail) = match thread_schedules::gate(facts) {
            Gate::Wait => return,
            Gate::Drop { outcome, detail } => (outcome, Some(detail)),
            Gate::Deliver => {
                let owned = key.to_owned();
                let prompt = match self
                    .schedules_write(move |store| store.begin_watch_attempt(&owned, now))
                    .await
                {
                    Ok(Some(pending)) => pending.prompt,
                    Ok(None) => return,
                    Err(error) => {
                        tracing::warn!(code = %error.code, "a watch prompt's attempt could not be recorded, so it was not sent");
                        return;
                    }
                };
                self.send_host_prompt(&head, &prompt, None).await
            }
        };
        let (owned, occurrence, at) = (key.to_owned(), pending.occurrence, self.now());
        if let Err(error) = self
            .schedules_write(move |store| {
                store.record_watch(&owned, occurrence, outcome, detail.as_deref(), at)
            })
            .await
        {
            tracing::warn!(code = %error.code, "a watch prompt's outcome could not be recorded");
        }
        self.pushed_quietly().await;
    }

    // --- Pull-request watch ---------------------------------------------------------

    /// Looks at every watched pull request once (the pull-request pass runs
    /// this after its lookups).
    pub(crate) async fn watch_pass(&self) {
        let keys = self
            .schedules_peek(|store| {
                store
                    .watches()
                    .iter()
                    .filter(|watch| watch.enabled)
                    .map(|watch| watch.thread_key.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut stop = self.inner.stop.subscribe();
        for key in keys {
            if *stop.borrow_and_update() {
                break;
            }
            self.watch_one(&key).await;
        }
    }

    /// One look at thread `key`'s pull request: its checks and unresolved
    /// review threads against what the watch saw; anything new waits for the
    /// thread as one prompt. A merged or closed pull request turns the
    /// watch off; `gh` missing or signed out sends nothing and says so.
    pub(crate) async fn watch_one(&self, key: &str) {
        let owned = key.to_owned();
        let Ok(Some((pull_request, dir))) = self
            .peek(move |document, _| {
                let record = document.record(&owned).filter(|record| record.listed())?;
                let dir = record
                    .workspace_dir()
                    .or_else(|| {
                        record
                            .workspace
                            .as_ref()
                            .filter(|workspace| workspace.mode != WorkspaceMode::None)
                            .map(|workspace| workspace.project_root.as_str())
                    })?
                    .to_owned();
                Some((record.pull_request.clone(), dir))
            })
            .await
        else {
            return;
        };
        let now = self.now();
        let target = key.to_owned();
        let Some(pull_request) = pull_request else {
            return;
        };
        if pull_request.state != PullRequestState::Open {
            let why = closed_reason(pull_request.number, pull_request.state);
            self.watch_turned_off(target, why, now).await;
            return;
        }
        let tools = Arc::clone(&self.inner.ports.ship);
        let number = pull_request.number;
        let read = tokio::task::spawn_blocking(move || {
            let folder = dir.clone();
            let dir = Path::new(&folder);
            let Some(gh) = tools.gh() else {
                return WatchRead::NeedsGh;
            };
            if gh.auth(dir) != GhAuth::Ready {
                return WatchRead::NeedsGh;
            }
            let current = match gh.pull_request(dir, number) {
                Ok(current) => current,
                Err(error) => return WatchRead::Failed(error.message),
            };
            if current.state != PullRequestState::Open {
                return WatchRead::Closed(current);
            }
            let checks = match gh.checks(dir, number) {
                Ok(checks) => checks,
                Err(error) => return WatchRead::Failed(error.message),
            };
            // A watch prompts with nobody reading first, so it hears only
            // the project's own people; the owner still sees every comment
            // under Bring in review comments.
            let comments = match gh.project_review_comments(dir, &current.url) {
                Ok(comments) => comments,
                Err(error) => return WatchRead::Failed(error.message),
            };
            WatchRead::Open {
                gh,
                dir: folder.clone(),
                pull_request: current,
                checks,
                comments,
            }
        })
        .await
        .unwrap_or_else(|_| WatchRead::Failed("the gh task stopped".into()));
        match read {
            WatchRead::NeedsGh => {
                let changed = self
                    .schedules_peek(|store| store.watch(key).is_some_and(|watch| !watch.needs_gh))
                    .unwrap_or(false);
                if changed {
                    let _ = self
                        .schedules_write(move |store| store.set_needs_gh(&target, true))
                        .await;
                    self.pushed_quietly().await;
                }
            }
            WatchRead::Failed(error) => {
                tracing::info!(%error, "a watched pull request could not be read this time");
            }
            WatchRead::Closed(current) => {
                let why = closed_reason(current.number, current.state);
                self.watch_turned_off(target, why, now).await;
            }
            WatchRead::Open {
                gh,
                dir,
                pull_request,
                checks,
                comments,
            } => {
                self.watch_observed(target, (gh, dir), pull_request, checks, comments)
                    .await;
            }
        }
    }

    async fn watch_turned_off(&self, key: String, why: String, now: u64) {
        let on = self
            .schedules_peek(|store| store.watch(&key).is_some_and(|watch| watch.enabled))
            .unwrap_or(false);
        if !on {
            return;
        }
        if let Err(error) = self
            .schedules_write(move |store| store.watch_off(&key, &why, now))
            .await
        {
            tracing::warn!(code = %error.code, "a finished pull request's watch could not be turned off");
        }
        self.pushed_quietly().await;
    }

    async fn watch_observed(
        &self,
        key: String,
        (gh, dir): (interface_review::thread_git::ship::Gh, String),
        pull_request: GhPullRequest,
        checks: Vec<interface_review::thread_git::ship::GhCheck>,
        comments: Vec<interface_contracts::threads::ReviewComment>,
    ) {
        let Some(Some(previous)) = self.schedules_peek(|store| {
            store
                .watch(&key)
                .filter(|watch| watch.enabled && watch.pending.is_none())
                .map(|watch| watch.fingerprint.clone())
        }) else {
            return;
        };
        let failing = checks
            .iter()
            .filter(|check| check.bucket == CheckBucket::Fail)
            .map(|check| {
                thread_schedules::check_id(
                    check.workflow.as_deref(),
                    &check.name,
                    check.link.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        let observation = thread_schedules::observe(
            previous.as_ref(),
            failing,
            comments
                .iter()
                .map(|comment| comment.url.clone())
                .filter(|url| !url.is_empty()),
        );
        let prompt = if observation.prompts() {
            let new_failure = observation.new_failure;
            let fresh = comments
                .into_iter()
                .filter(|comment| observation.new_reviews.contains(&comment.url))
                .collect::<Vec<_>>();
            let built = tokio::task::spawn_blocking(move || {
                let checks_text = new_failure.then(|| {
                    let summary = checks_summary(&checks);
                    super::ship::checks_prompt(&gh, Path::new(&dir), &pull_request, &summary)
                });
                let review_text =
                    (!fresh.is_empty()).then(|| thread_schedules::review_comments_text(&fresh));
                thread_schedules::watch_prompt(
                    pull_request.number,
                    &pull_request.url,
                    checks_text.as_deref(),
                    review_text.as_deref(),
                )
            })
            .await;
            match built {
                Ok(prompt) => Some(prompt),
                Err(_) => return,
            }
        } else {
            None
        };
        let now = self.now();
        let target = key.clone();
        let fingerprint = observation.fingerprint;
        match self
            .schedules_write(move |store| store.observed(&target, fingerprint, prompt, now))
            .await
        {
            Ok(waiting) => {
                self.pushed_quietly().await;
                if waiting {
                    self.deliver_watch(&key).await;
                }
            }
            Err(error) => {
                tracing::warn!(code = %error.code, "a watched pull request's look could not be recorded");
            }
        }
    }

    // --- Following the thread -----------------------------------------------------------

    /// A deleted, forgotten or archived thread's schedules and watch go.
    pub(crate) fn schedules_removed(&self, key: &str) {
        let has = self
            .schedules_peek(|store| {
                store
                    .schedules()
                    .iter()
                    .any(|schedule| schedule.thread_key == key)
                    || store.watch(key).is_some()
            })
            .unwrap_or(false);
        if !has {
            return;
        }
        let host = self.clone();
        let owned = key.to_owned();
        self.spawn_tracked(async move {
            if let Err(error) = host
                .schedules_write(move |store| store.remove_thread(&owned))
                .await
            {
                tracing::warn!(code = %error.code, "a removed thread's scheduled prompts stayed");
            }
            host.pushed_quietly().await;
        });
    }

    /// A reconcile saw these sessions archived: their threads' schedules go.
    pub(super) fn schedules_archived(&self, archived: &[String]) {
        for key in archived {
            self.schedules_removed(key);
        }
    }

    /// A switch moved the thread from `from` to `to`: its schedules and
    /// watch follow the head. A failed write is logged; firings reach the
    /// head either way (they resolve it when they go out).
    pub(super) async fn schedules_follow(&self, from: &str, to: &str) {
        if self.inner.schedules.get().is_none() {
            return;
        }
        let (from, to) = (from.to_owned(), to.to_owned());
        if let Err(error) = self
            .schedules_write(move |store| store.follow(&from, &to))
            .await
        {
            tracing::warn!(code = %error.code, "a switched thread's scheduled prompts did not follow it");
        }
    }

    // --- `iface trigger` -----------------------------------------------------------------

    /// Runs the schedule named (or numbered) `target` now: it fires outside
    /// its times and is offered to its thread at once. Answers the schedule
    /// and what became of the firing (`waiting` while the thread is busy).
    ///
    /// # Errors
    /// No such schedule, one turned off or already waiting, the store.
    pub async fn trigger_schedule(&self, target: &str) -> Result<Value, DispatchError> {
        let found = self
            .schedules_peek(|store| {
                store.find(target).map(|schedule| {
                    (
                        schedule.id.clone(),
                        schedule.name.clone(),
                        schedule.thread_key.clone(),
                    )
                })
            })
            .ok_or_else(|| unavailable("not attached"))?
            .map_err(refusal)?;
        let (id, name, key) = found;
        let now = self.now();
        let owned = id.clone();
        let occurrence = self
            .schedules_write(move |store| store.trigger(&owned, now))
            .await?;
        let run = self.deliver_schedule(&id).await;
        if run.is_none() {
            self.pushed_quietly().await;
        }
        let mut answer = json!({
            "id": id,
            "thread": key,
            "occurrence": thread_schedules::occurrence_id(&id, occurrence),
        });
        if let Some(name) = name {
            answer["name"] = name.into();
        }
        match run {
            Some(run) => {
                answer["outcome"] = serde_json::to_value(run.outcome).unwrap_or(Value::Null);
                if let Some(detail) = run.detail {
                    answer["detail"] = detail.into();
                }
            }
            None => answer["outcome"] = "waiting".into(),
        }
        Ok(answer)
    }

    // --- Team tools ------------------------------------------------------------------

    /// `schedule_prompt`: a schedule on the calling thread, set by its agent.
    pub(super) async fn team_schedule_prompt(
        &self,
        caller: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let when = team_when(input.get("when").unwrap_or(&Value::Null), self.now())?;
        let prompt = input
            .get("prompt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let name = input.get("name").and_then(Value::as_str).map(str::to_owned);
        let (id, first) = self
            .set_schedule(
                caller.to_owned(),
                None,
                name.clone(),
                prompt,
                when.clone(),
                ScheduleSetBy::Agent,
            )
            .await
            .map_err(|error| error.message)?;
        self.pushed_quietly().await;
        let mut answer = json!({
            "id": id,
            "when": when,
            "nextAt": first,
            "nextAtIso": iso(first),
        });
        if let Some(name) = name {
            answer["name"] = name.into();
        }
        Ok(answer)
    }

    /// `list_schedules`: the calling thread's schedules.
    pub(super) fn team_list_schedules(&self, caller: &str) -> Value {
        let schedules = self
            .schedules_peek(|store| store.view(caller, false))
            .flatten()
            .map(|view| view.schedules)
            .unwrap_or_default();
        json!({
            "schedules": schedules.into_iter().map(|schedule| {
                let mut item = json!({
                    "id": schedule.id,
                    "prompt": thread_schedules::detail(&schedule.prompt),
                    "when": schedule.when,
                    "enabled": schedule.enabled,
                    "setBy": schedule.set_by,
                });
                if let Some(name) = schedule.name {
                    item["name"] = name.into();
                }
                if let Some(next) = schedule.next_at {
                    item["nextAt"] = next.into();
                    item["nextAtIso"] = iso(next).into();
                }
                if let Some(run) = schedule.last_run {
                    item["lastRun"] = serde_json::to_value(run).unwrap_or(Value::Null);
                }
                item
            }).collect::<Vec<_>>(),
        })
    }

    /// `cancel_schedule`: deletes one of the calling thread's schedules.
    pub(super) async fn team_cancel_schedule(
        &self,
        caller: &str,
        id: &str,
    ) -> Result<Value, String> {
        let mine = self
            .schedules_peek(|store| {
                store
                    .schedule(id)
                    .is_some_and(|schedule| schedule.thread_key == caller)
            })
            .unwrap_or(false);
        if !mine {
            return Err(format!("This thread has no scheduled prompt {id}."));
        }
        let owned = id.to_owned();
        self.schedules_write(move |store| store.delete(&owned))
            .await
            .map_err(|error| error.message)?;
        self.pushed_quietly().await;
        Ok(json!({"id": id, "cancelled": true}))
    }
}

fn closed_reason(number: u64, state: PullRequestState) -> String {
    match state {
        PullRequestState::Merged => format!("#{number} was merged; the watch stopped."),
        _ => format!("#{number} was closed; the watch stopped."),
    }
}

fn iso(ms: u64) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// `schedule_prompt`'s `when`, as the model writes it: `{kind: "once", at}`
/// (ms or an ISO 8601 time), `{kind: "once", inMinutes}`, `{kind:
/// "interval", minutes}` or `{kind: "calendar", calendar: {timezone,
/// weekdays, hour, minute}}`.
fn team_when(value: &Value, now: u64) -> Result<ScheduleWhen, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "`when` must be an object with a `kind`.".to_owned())?;
    let only = |allowed: &[&str]| -> Result<(), String> {
        match object.keys().find(|key| !allowed.contains(&key.as_str())) {
            Some(key) => Err(format!("`when` takes no `{key}` here.")),
            None => Ok(()),
        }
    };
    match object.get("kind").and_then(Value::as_str) {
        Some("once") => {
            if let Some(minutes) = object.get("inMinutes") {
                only(&["kind", "inMinutes"])?;
                let minutes = minutes
                    .as_u64()
                    .filter(|minutes| {
                        (1..=u64::from(thread_schedules::MAX_INTERVAL_MINUTES)).contains(minutes)
                    })
                    .ok_or_else(|| {
                        "`inMinutes` is a whole number of minutes, at most 30 days.".to_owned()
                    })?;
                return Ok(ScheduleWhen::Once {
                    at: now + minutes * 60_000,
                });
            }
            only(&["kind", "at"])?;
            let at = match object.get("at") {
                Some(Value::Number(number)) => number.as_u64(),
                Some(Value::String(text)) => chrono::DateTime::parse_from_rfc3339(text)
                    .ok()
                    .and_then(|at| u64::try_from(at.timestamp_millis()).ok()),
                _ => None,
            }
            .ok_or_else(|| {
                "`at` is an ISO 8601 time with its offset, or ms since the epoch.".to_owned()
            })?;
            Ok(ScheduleWhen::Once { at })
        }
        Some("interval") => {
            only(&["kind", "minutes"])?;
            let minutes = object
                .get("minutes")
                .and_then(Value::as_u64)
                .and_then(|minutes| u32::try_from(minutes).ok())
                .ok_or_else(|| "`minutes` is a whole number.".to_owned())?;
            Ok(ScheduleWhen::Interval { minutes })
        }
        Some("calendar") => {
            only(&["kind", "calendar"])?;
            let calendar: ScheduleCalendar =
                serde_json::from_value(object.get("calendar").cloned().unwrap_or(Value::Null))
                    .map_err(|_| {
                        "`calendar` is {timezone, weekdays: [1-7], hour, minute}.".to_owned()
                    })?;
            Ok(ScheduleWhen::Calendar { calendar })
        }
        _ => Err("`when.kind` is once, interval or calendar.".to_owned()),
    }
}

#[async_trait::async_trait]
impl crate::orchestration_transport::ScheduleTrigger for ThreadHost {
    async fn trigger(&self, target: &str) -> Result<Value, String> {
        self.trigger_schedule(target)
            .await
            .map_err(|error| error.message)
    }
}

#[cfg(test)]
pub(crate) fn when_for_tests(value: &Value, now: u64) -> Result<ScheduleWhen, String> {
    team_when(value, now)
}
