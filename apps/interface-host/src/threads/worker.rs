//! The thread worker: registry reconcile (seeding and drops), run starts and
//! ends from the tap, end checkpoints, auto-settle every ten minutes, snooze
//! wakes, pull request lookups, stale prepared workspaces and worktree
//! releases. Started once by [`ThreadHost::start`], stopped by
//! [`ThreadHost::shutdown`].

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use interface_common::sync::lock;
use interface_contracts::{
    engine::{SessionKind, SessionOrigin, SessionStatus, SessionSummary, session_key},
    threads::ThreadsView,
};
use interface_review::thread_git::{CaptureOutcome, PreparedGitWorkspace, RemoveOutcome};
use interface_settings::threads::{
    AUTO_SETTLE_INTERVAL_MS, FinishedTurn, ObservedSession, PreparedRecord, PullRequestLookupBy,
    ReleaseOutcome, SeedOrigin, SessionUse, TurnEnd, TurnMeasure,
};
use tokio::time::Instant;

use super::{
    ThreadHost,
    ports::PullRequestLookup,
    run_tap::{RunChange, RunOp},
};
use crate::dispatcher::DispatchError;

impl ThreadHost {
    /// Starts the worker: discards stale prepared workspaces, reconciles the
    /// registry, settles and looks up pull requests once, then serves run
    /// changes, registry rounds and timers until shutdown. Starting twice
    /// does nothing.
    pub fn start(&self) {
        let Some(mut ops) = lock(&self.inner.ops_receiver).take() else {
            return;
        };
        self.start_journal();
        let host = self.clone();
        let mut stop = self.inner.stop.subscribe();
        let handle = tokio::spawn(async move {
            host.discard_stale_prepared().await;
            host.reconcile().await;
            host.auto_settle().await;
            host.spawn_pull_request_pass();
            // Firings due while Interface was closed (T3 section 7).
            host.spawn_schedule_pass();
            let interval = Duration::from_millis(AUTO_SETTLE_INTERVAL_MS);
            let mut next_settle = Instant::now() + interval;
            loop {
                let wake = host.next_snooze_wake().await;
                let scheduled = host.next_schedule_wake();
                let deadline = [wake, scheduled]
                    .into_iter()
                    .flatten()
                    .fold(next_settle, Instant::min);
                tokio::select! {
                    biased;
                    _ = stop.changed() => break,
                    op = ops.recv() => match op {
                        Some(op) => host.apply_run(op).await,
                        None => break,
                    },
                    () = host.inner.registry.changed() => {
                        host.reconcile_round().await;
                        // A thread a firing waits for may have stopped working.
                        if host.schedules_waiting() {
                            host.spawn_schedule_pass();
                        }
                    }
                    () = host.inner.timers.notified() => {}
                    () = tokio::time::sleep_until(deadline) => {
                        if Instant::now() >= next_settle {
                            next_settle = Instant::now() + interval;
                            host.auto_settle().await;
                            host.spawn_pull_request_pass();
                        }
                        if wake.is_some_and(|wake| wake <= Instant::now()) {
                            host.wake_snoozed().await;
                        }
                        if scheduled.is_some_and(|scheduled| scheduled <= Instant::now()) {
                            host.spawn_schedule_pass();
                        }
                    }
                }
            }
        });
        *lock(&self.inner.worker) = Some(handle);
    }

    /// Stops the worker, then waits for end checkpoints, releases and
    /// lookups already running; none is cancelled midway.
    pub async fn shutdown(&self) {
        let _ = self.inner.stop.send(true);
        let worker = lock(&self.inner.worker).take();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
        loop {
            let tasks = std::mem::take(&mut *lock(&self.inner.tasks));
            if tasks.is_empty() {
                return;
            }
            for task in tasks {
                let _ = task.await;
            }
        }
    }

    /// The next snooze to wake, as a timer deadline.
    async fn next_snooze_wake(&self) -> Option<Instant> {
        let wake = self
            .peek(|document, now| {
                document
                    .next_snooze_wake(now)
                    .map(|until| until.saturating_sub(now))
            })
            .await
            .ok()?;
        wake.map(|after| Instant::now() + Duration::from_millis(after))
    }

    /// A snooze ran out: the next revision carries the thread back.
    pub(crate) async fn wake_snoozed(&self) {
        if let Err(error) = self
            .with_store_quiet(interface_settings::ThreadStore::bump)
            .await
        {
            tracing::warn!(code = %error.code, "a snoozed thread could not be woken");
        }
    }

    pub(crate) async fn apply_run(&self, op: RunOp) {
        let (key, generation, ended) = (
            op.key.clone(),
            op.generation,
            matches!(op.change, RunChange::Ended(_)),
        );
        self.apply_run_op(op).await;
        if ended {
            // Recorded (or dropped): reconcile may judge the record alone.
            self.end_recorded(&key, generation);
        }
    }

    async fn apply_run_op(&self, op: RunOp) {
        let RunOp {
            key,
            at,
            change,
            generation,
            touched,
        } = op;
        if !self.knows(&key).await && !self.seed_session(&key).await {
            return;
        }
        let target = key.clone();
        let host = self.clone();
        // Checked under the store lock, which a send's turn is recorded and
        // its generation opened under: an op older than that send's turn is
        // about a run that turn replaced.
        let stale = move |key: &str| host.run_generation(key) > generation;
        let result = match change {
            // A steer the provider made into a turn of its own (the run it
            // was steered into ended first) is that turn's prompt.
            RunChange::Started => match self.take_restarted_steer(&key, at) {
                Some(prompt) => self
                    .edit_quiet(move |document, _| {
                        if stale(&target) {
                            return Ok(());
                        }
                        document.begin_turn(&target, &prompt, None, at).map(|_| ())
                    })
                    .await
                    .map(|()| None),
                None => self
                    .edit_quiet(move |document, _| {
                        Ok(!stale(&target) && document.run_started(&target, at))
                    })
                    .await
                    .map(|_| None),
            },
            RunChange::Ended(outcome) => self
                .edit_quiet(move |document, _| {
                    if stale(&target) {
                        return Ok((None, false));
                    }
                    let finished = document.run_ended(&target, at, outcome);
                    Ok((finished, document.wants_generated_title(&target)))
                })
                .await
                .map(|(finished, title)| {
                    if title {
                        self.title_after_first_run(&key);
                    }
                    // A thread team tools started: its caller's notice.
                    self.team_run_ended(&key);
                    // A scheduled prompt waiting for this run to end.
                    if self.schedules_waiting() {
                        self.spawn_schedule_pass();
                    }
                    finished
                }),
        };
        match result {
            Ok(Some(finished)) => self.capture_end(finished, touched),
            Ok(None) => {}
            Err(error) => tracing::warn!(code = %error.code, "a thread run could not be recorded"),
        }
    }

    async fn knows(&self, key: &str) -> bool {
        let key = key.to_owned();
        self.peek(move |document, _| document.record(&key).is_some())
            .await
            .unwrap_or(false)
    }

    /// Seeds `key` now when it is a Threads-surface session; answers whether
    /// it has a record afterwards.
    async fn seed_session(&self, key: &str) -> bool {
        let agent = self
            .inner
            .ports
            .sessions
            .session(key)
            .ok()
            .flatten()
            .is_none_or(|summary| summary.kind == Some(SessionKind::Agent));
        if agent {
            return false;
        }
        self.reconcile().await;
        self.knows(key).await
    }

    /// Records a finished git turn's end, off the worker: its end checkpoint
    /// and numstat counts (when it has a start checkpoint), the files its
    /// run's own edits named (`touched`, kept relative to the repository
    /// root) and whether others shared its checkout. Where others work in the
    /// same checkout the counts cover only those files, and stay unknown
    /// without them
    /// ([`ThreadsDocument::turn_measure`](interface_settings::ThreadsDocument::turn_measure));
    /// its diff is scoped the same way.
    pub(crate) fn capture_end(&self, finished: FinishedTurn, touched: Option<BTreeSet<String>>) {
        let host = self.clone();
        self.spawn_tracked(async move {
            let git = Arc::clone(&host.inner.ports.git);
            let measure = host.turn_measure(&finished.key, touched.clone()).await;
            let FinishedTurn {
                key,
                ordinal,
                dir,
                start_ref,
                end_ref,
            } = finished;
            let end = tokio::task::spawn_blocking(move || {
                let dir = Path::new(&dir);
                if !dir.is_dir() {
                    return None;
                }
                let shared = !matches!(measure, TurnMeasure::Checkout);
                let touched = touched.and_then(|paths| {
                    let paths = paths.into_iter().collect::<Vec<_>>();
                    git.repo_relative_paths(dir, &paths)
                        .inspect_err(|error| {
                            tracing::warn!(%error, "a thread turn's files could not be named");
                        })
                        .ok()
                });
                let Some(start_ref) = start_ref else {
                    return Some(TurnEnd {
                        touched,
                        shared,
                        ..TurnEnd::default()
                    });
                };
                match git.capture_checkpoint(dir, &end_ref) {
                    Ok(CaptureOutcome::Captured { .. }) => {
                        let counts = match measure {
                            TurnMeasure::Checkout => {
                                git.numstat(dir, &start_ref, Some(&end_ref)).ok()
                            }
                            TurnMeasure::Files(paths) => git
                                .numstat_paths(dir, &start_ref, Some(&end_ref), &paths)
                                .ok(),
                            TurnMeasure::Unknown => None,
                        };
                        Some(TurnEnd {
                            end_ref: Some(end_ref),
                            counts,
                            touched,
                            shared,
                        })
                    }
                    Ok(CaptureOutcome::Skipped { reason }) => {
                        tracing::info!(%reason, "a thread turn ended without a checkpoint");
                        Some(TurnEnd {
                            touched,
                            shared,
                            ..TurnEnd::default()
                        })
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a thread turn's end checkpoint failed");
                        Some(TurnEnd {
                            touched,
                            shared,
                            ..TurnEnd::default()
                        })
                    }
                }
            })
            .await
            .ok()
            .flatten();
            let Some(end) = end else {
                return;
            };
            if let Err(error) = host
                .edit_quiet(move |document, _| {
                    document.record_turn_end(&key, ordinal, end);
                    Ok(())
                })
                .await
            {
                tracing::warn!(code = %error.code, "a thread turn's end could not be recorded");
            }
        });
    }

    /// What `key`'s finished turn is measured over, by who else works in its
    /// checkout now. An unreadable registry counts as shared; an unreadable
    /// store measures nothing.
    async fn turn_measure(&self, key: &str, touched: Option<BTreeSet<String>>) -> TurnMeasure {
        let uses = self.session_uses().ok();
        self.peek(|document, _| document.turn_measure(key, uses.as_deref(), touched.as_ref()))
            .await
            .unwrap_or(TurnMeasure::Unknown)
    }

    /// Brings thread records in line with the registry and releases the
    /// owned worktrees of archived threads. A failed registry read, or an
    /// empty registry while threads exist, changes nothing.
    pub(crate) async fn reconcile(&self) {
        self.reconcile_if(false).await;
    }

    /// A registry round: reconciles only when something reconcile reads
    /// (membership, status, archive, kind, origin, hidden workers) changed
    /// since the last round. Timestamp-only rounds arrive every few seconds
    /// while an agent streams.
    pub(crate) async fn reconcile_round(&self) {
        self.reconcile_if(true).await;
    }

    async fn reconcile_if(&self, only_changed: bool) {
        let sessions = match self.inner.ports.sessions.sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                tracing::warn!(%error, "threads could not read the session registry");
                return;
            }
        };
        let mut hidden = self.inner.ports.hidden.hidden_keys().await;
        // Protocol 67: side questions' side threads are never listed.
        hidden.extend(self.side_keys());
        // An empty registry read changes nothing, the hidden sessions included.
        let hidden_changed = !sessions.is_empty() && {
            let listed = hidden_listed(&sessions, &hidden);
            let mut current = lock(&self.inner.hidden_listed);
            let changed = *current != listed;
            *current = listed;
            changed
        };
        let observed = observe(&sessions, &hidden);
        let fingerprint = fingerprint(&observed);
        let previous = lock(&self.inner.registry_seen).replace(fingerprint);
        if only_changed && previous == Some(fingerprint) && !hidden_changed {
            return;
        }
        let complete = self.inner.ports.sessions.registry_complete();
        let host = self.clone();
        let reconciled = self
            .with_store_quiet(move |store, now| {
                let (reconciled, committed) = store.apply(now, |document| {
                    if observed.is_empty() && !document.records.is_empty() {
                        return Ok(None);
                    }
                    Ok(Some(document.reconcile(
                        &observed,
                        |key| host.run_accounted(key),
                        complete,
                        now,
                    )))
                })?;
                // Which sessions are hidden reaches Chat as a new revision,
                // even when no record changed.
                if hidden_changed && !committed {
                    store.bump(now)?;
                }
                Ok(reconciled)
            })
            .await;
        let reconciled = match reconciled {
            Ok(Some(reconciled)) => reconciled,
            Ok(None) => {
                tracing::warn!("the session registry is empty; thread records were kept");
                return;
            }
            Err(error) => {
                tracing::warn!(code = %error.code, "threads could not reconcile the registry");
                return;
            }
        };
        // A dropped record's worktree stays: its session went without an
        // archive, delete or forget (a provider store moved, a registry
        // rebuilt), so nothing says the owner is done with it.
        if !reconciled.dropped.is_empty() {
            tracing::info!(
                dropped = reconciled.dropped.len(),
                "thread records whose sessions stayed missing were dropped; their worktrees were kept"
            );
        }
        for key in reconciled.to_release {
            self.release_owned(key);
        }
        // An archived thread's scheduled prompts and watch go (T3 section 7).
        let archived = sessions
            .iter()
            .filter(|summary| summary.archived)
            .map(|summary| session_key(&summary.engine, &summary.session_id))
            .collect::<Vec<_>>();
        self.schedules_archived(&archived);
        self.segments_follow_archive(&sessions).await;
    }

    /// A switched thread's earlier segments are archived and restored with
    /// their head (T3 section 5), so the chain stays one thread.
    async fn segments_follow_archive(&self, sessions: &[SessionSummary]) {
        let archived = sessions
            .iter()
            .map(|summary| {
                (
                    session_key(&summary.engine, &summary.session_id),
                    summary.archived,
                )
            })
            .collect::<HashMap<_, _>>();
        let Ok(changes) = self
            .peek(move |document, _| {
                document
                    .records
                    .iter()
                    .filter_map(|record| {
                        let head = record.segment_of.as_deref()?;
                        let wanted = *archived.get(head)?;
                        (archived.get(&record.key) == Some(&!wanted))
                            .then(|| (record.key.clone(), wanted))
                    })
                    .collect::<Vec<_>>()
            })
            .await
        else {
            return;
        };
        if changes.is_empty() {
            return;
        }
        let host = self.clone();
        self.spawn_tracked(async move {
            for (segment, archived) in changes {
                if let Err(error) = host.inner.ports.sessions.set_archived(&segment, archived).await {
                    tracing::warn!(%error, "an earlier segment of a thread did not follow its archive");
                }
            }
            host.schedule_registry_push();
        });
    }

    /// The registry's sessions as the store's isolation checks see them.
    pub(crate) fn session_uses(&self) -> Result<Vec<SessionUse>, DispatchError> {
        let sessions = self
            .inner
            .ports
            .sessions
            .sessions()
            .map_err(|error| super::registry_unavailable(&error))?;
        let live = self
            .inner
            .ports
            .sessions
            .live_keys()
            .map_err(|error| super::registry_unavailable(&error))?;
        Ok(sessions
            .iter()
            .map(|summary| {
                let key = session_key(&summary.engine, &summary.session_id);
                SessionUse {
                    live: live.contains(&key),
                    cwd: summary.cwd.clone(),
                    archived: summary.archived,
                    title: summary.title.clone(),
                    key,
                }
            })
            .collect())
    }

    /// The owner archived, deleted or forgot `key`'s session: its owned
    /// worktree goes when nothing else uses it. The outcome is recorded so
    /// it is tried once per archive.
    pub(crate) fn release_owned(&self, key: String) {
        if !lock(&self.inner.releasing).insert(key.clone()) {
            return;
        }
        let host = self.clone();
        self.spawn_tracked(async move {
            let outcome = host.release_owned_now(&key).await;
            let target = key.clone();
            if let Err(error) = host
                .edit_quiet(move |document, now| {
                    document.record_release(&target, outcome, now);
                    Ok(())
                })
                .await
            {
                tracing::warn!(code = %error.code, "a worktree release could not be recorded");
            }
            lock(&host.inner.releasing).remove(&key);
        });
    }

    /// [`Self::release_owned`] for a thread that owns a worktree.
    async fn release_if_owned(&self, key: String) {
        let target = key.clone();
        let owns = self
            .peek(move |document, _| {
                document
                    .record(&target)
                    .is_some_and(|record| record.owned_worktree().is_some())
            })
            .await
            .unwrap_or(false);
        if owns {
            self.release_owned(key);
        }
    }

    async fn release_owned_now(&self, key: &str) -> ReleaseOutcome {
        let Ok(uses) = self.session_uses() else {
            return ReleaseOutcome::Failed;
        };
        let own_live = uses
            .iter()
            .any(|session| session.key == key && session.live);
        let target = key.to_owned();
        let plan = self
            .peek(move |document, _| document.release_plan(&target, own_live, &uses))
            .await;
        match plan {
            Ok(Ok(plan)) => {
                self.remove_worktree(plan.git_root, plan.worktree_path)
                    .await
            }
            Ok(Err(_)) => ReleaseOutcome::InUse,
            Err(_) => ReleaseOutcome::Failed,
        }
    }

    /// `thread_git::remove_worktree`: clean worktrees go, dirty ones stay,
    /// branches always stay.
    pub(crate) async fn remove_worktree(&self, git_root: String, path: String) -> ReleaseOutcome {
        let git = Arc::clone(&self.inner.ports.git);
        let removed = tokio::task::spawn_blocking(move || {
            git.remove_worktree(Path::new(&git_root), Path::new(&path))
        })
        .await;
        match removed {
            Ok(Ok(RemoveOutcome::Removed)) => ReleaseOutcome::Removed,
            Ok(Ok(RemoveOutcome::KeptDirty)) => ReleaseOutcome::KeptDirty,
            Ok(Ok(RemoveOutcome::KeptIgnored)) => ReleaseOutcome::KeptIgnored,
            Ok(Ok(RemoveOutcome::Missing)) => ReleaseOutcome::Missing,
            Ok(Ok(RemoveOutcome::InUse)) => ReleaseOutcome::InUse,
            Ok(Err(error)) => {
                tracing::warn!(%error, "a thread worktree could not be released");
                ReleaseOutcome::Failed
            }
            Err(_) => ReleaseOutcome::Failed,
        }
    }

    /// Settles every thread auto-settle allows, by the registry's status.
    pub(crate) async fn auto_settle(&self) {
        let busy = match self.inner.ports.sessions.sessions() {
            Ok(sessions) => sessions
                .iter()
                .map(|summary| {
                    (
                        session_key(&summary.engine, &summary.session_id),
                        matches!(
                            summary.status,
                            SessionStatus::Working | SessionStatus::Waiting
                        ),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>(),
            Err(error) => {
                tracing::warn!(%error, "auto-settle could not read the session registry");
                return;
            }
        };
        let host = self.clone();
        if let Err(error) = self
            .edit_quiet(move |document, now| {
                Ok(document.auto_settle(now, |key| {
                    busy.get(key).map(|busy| *busy || host.run_accounted(key))
                }))
            })
            .await
        {
            tracing::warn!(code = %error.code, "auto-settle could not save");
        }
    }

    /// Looks up the pull requests that are due, one thread after another,
    /// unless a pass is already running.
    pub(crate) fn spawn_pull_request_pass(&self) {
        if self.inner.pull_request_pass.swap(true, Ordering::AcqRel) {
            return;
        }
        let host = self.clone();
        self.spawn_tracked(async move {
            let due = host
                .peek(|document, now| document.pull_requests_due(now, None))
                .await
                .unwrap_or_default();
            let mut stop = host.inner.stop.subscribe();
            for check in due {
                if *stop.borrow_and_update() {
                    break;
                }
                host.look_up_pull_request(check).await;
            }
            // Watched pull requests (T3 section 7), after their lookups.
            host.watch_pass().await;
            host.inner.pull_request_pass.store(false, Ordering::Release);
        });
    }

    /// One lookup, recorded unless `gh` was unavailable.
    pub(crate) async fn look_up_pull_request(
        &self,
        check: interface_settings::threads::PullRequestCheck,
    ) -> PullRequestLookup {
        let lookups = Arc::clone(&self.inner.ports.pull_requests);
        let (root, by) = (PathBuf::from(&check.project_root), check.by.clone());
        let found = tokio::task::spawn_blocking(move || match by {
            PullRequestLookupBy::Branch(branch) => lookups.lookup(&root, &branch),
            PullRequestLookupBy::Number(number) => lookups.view(&root, number),
        })
        .await
        .unwrap_or(PullRequestLookup::Unavailable);
        let record = match &found {
            PullRequestLookup::Found(found) => Some(Some(found.clone())),
            PullRequestLookup::None => Some(None),
            PullRequestLookup::Unavailable => None,
        };
        if let Some(record) = record {
            let key = check.key;
            if let Err(error) = self
                .edit_quiet(move |document, now| {
                    document.record_pull_request(&key, record, now);
                    Ok(())
                })
                .await
            {
                tracing::warn!(code = %error.code, "a pull request lookup could not be recorded");
            }
        }
        found
    }

    /// Prepared workspaces never bound within the hour: their worktree (if
    /// this prepare made it) and baseline go, unless a session or thread
    /// works in it (a launch whose bind never arrived); such an entry stays
    /// for a later sweep.
    pub(crate) async fn discard_stale_prepared(&self) {
        let stale = match self
            .peek(interface_settings::ThreadsDocument::stale_prepared)
            .await
        {
            Ok(stale) => stale,
            Err(error) => {
                tracing::warn!(code = %error.code, "stale prepared workspaces could not be read");
                return;
            }
        };
        for token in stale {
            if let Err(error) = self.discard_unused_prepared(token).await {
                tracing::warn!(code = %error.code, "a stale prepared workspace was kept");
            }
        }
    }

    /// Forgets the prepared workspace `token` and discards what it made,
    /// unless something uses its worktree: then both stay. Answers whether
    /// it was discarded.
    pub(crate) async fn discard_unused_prepared(
        &self,
        token: String,
    ) -> Result<bool, DispatchError> {
        let target = token.clone();
        let Some(dir) = self
            .peek(move |document, _| {
                document
                    .prepared(&target)
                    .map(|prepared| prepared.workspace.worktree_path.clone())
            })
            .await?
        else {
            return Ok(false);
        };
        if let Some(dir) = dir
            && let Some(user) = self.worktree_user(&dir).await?
        {
            tracing::info!(%user, "a prepared worktree is in use, so it was kept");
            return Ok(false);
        }
        let discarded = self
            .edit_quiet(move |document, _| Ok(document.take_prepared(&token)))
            .await?;
        if let Some(prepared) = discarded {
            self.discard_prepared(prepared).await?;
        }
        Ok(true)
    }

    /// Who works in `dir`: an unarchived registry session working inside it
    /// (a launch whose bind never arrived is one), or what a release checks
    /// (a live session inside or around it, an unarchived thread).
    pub(super) async fn worktree_user(&self, dir: &str) -> Result<Option<String>, DispatchError> {
        let uses = self.session_uses()?;
        if let Some(session) = uses.iter().find(|session| {
            !session.archived
                && Path::new(dir).is_absolute()
                && Path::new(&session.cwd).starts_with(dir)
        }) {
            return Ok(Some(session.key.clone()));
        }
        let dir = dir.to_owned();
        self.peek(move |document, _| {
            document
                .shared_with("", &dir, &uses, true)
                .map(|user| user.key().to_owned())
        })
        .await
    }

    /// Deletes a prepared workspace's baseline and removes the worktree and
    /// branch it created (a worktree with changes is kept).
    pub(crate) async fn discard_prepared(
        &self,
        prepared: PreparedRecord,
    ) -> Result<(), DispatchError> {
        let git = Arc::clone(&self.inner.ports.git);
        let outcome = tokio::task::spawn_blocking(move || {
            if let (Some(baseline), Some(root)) = (&prepared.baseline_ref, &prepared.git_root)
                && let Err(error) = git.delete_refs(Path::new(root), baseline)
            {
                tracing::warn!(%error, "a prepared workspace's baseline was kept");
            }
            if !prepared.created {
                return Ok(None);
            }
            git.discard_workspace(&PreparedGitWorkspace {
                cwd: PathBuf::from(&prepared.cwd),
                repo_root: prepared.git_root.as_ref().map(PathBuf::from),
                worktree_path: prepared.workspace.worktree_path.as_ref().map(PathBuf::from),
                branch: prepared.workspace.branch.clone(),
                base_ref: prepared.workspace.base_ref.clone(),
                created: true,
            })
            .map(Some)
        })
        .await
        .map_err(|_| super::internal("The git task stopped unexpectedly."))?;
        match outcome {
            Ok(outcome) => {
                if let Some(outcome) = outcome {
                    tracing::info!(?outcome, "discarded a prepared thread workspace");
                }
                Ok(())
            }
            Err(error) => Err(DispatchError::new(
                super::THREADS_GIT_FAILED_CODE,
                format!(
                    "The prepared workspace could not be removed: {} Remove the worktree by hand if you no longer need it.",
                    error.message
                ),
                false,
                false,
            )),
        }
    }
}

/// The registry sessions `hidden` leaves out of Threads and Chat (agent-kind
/// sessions are never listed anyway), sorted.
fn hidden_listed(sessions: &[SessionSummary], hidden: &BTreeSet<String>) -> Vec<String> {
    let mut keys = sessions
        .iter()
        .filter(|summary| summary.kind != Some(SessionKind::Agent))
        .map(|summary| session_key(&summary.engine, &summary.session_id))
        .filter(|key| hidden.contains(key))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys
}

impl ThreadHost {
    /// Installs the host's provider accounts (protocol 60) once: views then
    /// name each thread's account, and team tools start threads on it.
    pub fn attach_accounts(&self, accounts: Arc<crate::provider_accounts::ProviderAccounts>) {
        let _ = self.inner.accounts.set(accounts);
    }

    /// The installed provider accounts, when the host has them.
    pub(super) fn accounts(&self) -> Option<&Arc<crate::provider_accounts::ProviderAccounts>> {
        self.inner.accounts.get()
    }

    /// Puts each thread's added provider account on `view` (protocol 60).
    pub(super) fn with_accounts(&self, view: &mut ThreadsView) {
        let Some(accounts) = self.accounts() else {
            return;
        };
        for thread in &mut view.threads {
            thread.account = accounts.account_of_key(&thread.key);
        }
    }

    /// Puts the sessions the last reconcile hid on `view` (protocol 58).
    pub(super) fn with_hidden(&self, view: &mut ThreadsView) {
        view.hidden_keys
            .clone_from(&lock(&self.inner.hidden_listed));
    }
}

/// The registry rows as reconcile sees them. A Threads-surface session is
/// one Chat lists: not agent-kind, not an Away or orchestration worker.
pub(crate) fn observe(
    sessions: &[SessionSummary],
    hidden: &BTreeSet<String>,
) -> Vec<ObservedSession> {
    sessions
        .iter()
        .map(|summary| {
            let key = session_key(&summary.engine, &summary.session_id);
            ObservedSession {
                listed: summary.kind != Some(SessionKind::Agent) && !hidden.contains(&key),
                origin: if summary.origin == Some(SessionOrigin::App) {
                    SeedOrigin::App
                } else {
                    SeedOrigin::Store
                },
                first_seen: iso_ms(&summary.first_seen),
                last_activity: iso_ms(&summary.last_activity),
                busy: matches!(
                    summary.status,
                    SessionStatus::Working | SessionStatus::Waiting
                ),
                archived: summary.archived,
                key,
            }
        })
        .collect()
}

/// What reconcile reads from each session, hashed.
fn fingerprint(observed: &[ObservedSession]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for session in observed {
        (
            &session.key,
            session.origin == SeedOrigin::App,
            session.busy,
            session.archived,
            session.listed,
        )
            .hash(&mut hasher);
    }
    hasher.finish()
}

fn iso_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
}

impl crate::session_management::RemovalHooks for ThreadHost {
    /// A deleted or forgotten thread's owned worktree goes as an archived
    /// one's does, and its journal goes. Its record stays until the session has been missing for
    /// a day, so the release still knows the worktree.
    fn session_removed(&self, session_key: &str) {
        // Deleting or forgetting a session removes its journal; archiving
        // keeps it.
        self.journal_removed(session_key);
        self.team_removed(session_key);
        self.stash_removed(session_key);
        self.schedules_removed(session_key);
        self.secrets_removed(session_key);
        self.sides_removed(session_key);
        let host = self.clone();
        let key = session_key.to_owned();
        self.spawn_tracked(async move { host.release_if_owned(key).await });
    }

    /// A switched thread's head was deleted or forgotten: its earlier
    /// segments' sessions and journals go the same way (T3 section 5).
    fn session_removed_as(&self, session_key: &str, deleted: bool) {
        self.session_removed(session_key);
        let host = self.clone();
        let key = session_key.to_owned();
        self.spawn_tracked(async move {
            for segment in host.segments_of(&key).await {
                if let Err(error) = host.inner.ports.sessions.remove(&segment, deleted).await {
                    tracing::warn!(%error, "an earlier segment of a removed thread stayed");
                    continue;
                }
                host.journal_removed(&segment);
                host.team_removed(&segment);
                host.stash_removed(&segment);
            }
        });
    }
}
