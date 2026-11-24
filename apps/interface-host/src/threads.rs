//! Host owner of `iface:threadControl` and the `iface:threads` push (Threads
//! surface v1, protocol 34; `docs/superpowers/specs/2026-10-06-threads-design.md`).
//!
//! An adapter. The thread rules (sections, seeding, run stamps, auto-settle,
//! restore isolation) live in `interface_settings::threads` and the git
//! mechanics in `interface_review::thread_git`. [`ThreadHost`] holds the one
//! `threads.v2.json` writer, routes every command, keeps the in-memory run
//! tap, hooks sends, runs the timers, and pushes the whole view after each
//! committed change. Store I/O and git run on the blocking pool; the store
//! lock is held across a commit and its push, so pushes leave in revision
//! order. Children: `ports.rs` (the seams and their production
//! implementations), `workspace.rs` (workspace, diff, restore, fork and
//! pull request commands), `ship.rs` (status, commit, branch, push and pull
//! request, checks, review comments and merge), `run_tap.rs` (the event
//! tap, the registry signal and the send hook), `titles.rs` (generated titles and the owner-rename
//! hook), `worker.rs` (registry reconcile, timers, releases and end
//! checkpoints), `journal.rs` (the thread journal's tap, prompts, worker,
//! drain, import, history fallback and `iface:threadJournal`), `away.rs` (an Away loop's thread: its worktree, its
//! hold on the session, its projection in the view), `opinion.rs`
//! (Second opinion and Continue with), `switch.rs` (switch provider
//! inside a thread, protocol 59), `schedules.rs` (scheduled prompts and
//! pull-request watch, protocol 62) and `resume.rs` (a thread's usage
//! limit and Resume at reset, protocol 66).

mod away;
mod folder_trust;
mod journal;
mod opinion;
mod outside;
mod ports;
mod reference;
mod resume;
mod run_tap;
mod schedules;
mod secrets;
mod ship;
mod side;
mod stash;
mod switch;
mod team;
mod terminal_bridge;
mod titles;
mod worker;
mod workflows;
mod workspace;

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64},
    },
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::{
    ipc::CHANNELS,
    threads::{SettledReason, ThreadCommand, ThreadControlResponse, ThreadsView},
};
use interface_settings::{
    SettingsError, ThreadStore, ThreadStoreError, ThreadsDocument,
    threads::{THREADS_FILE_NAME, ThreadRefusal},
};
use interface_storage::StorageError;
use serde_json::Value;
use tokio::{
    sync::{Notify, mpsc, watch},
    task::JoinHandle,
};

pub use away::THREADS_AWAY_RUNNING_CODE;
pub use folder_trust::{
    THREADS_FOLDER_TRUST_REFUSED_CODE, THREADS_FOLDER_TRUST_UNAVAILABLE_CODE, ThreadFolderTrustPort,
};
pub use journal::ThreadJournalConfig;
pub use outside::OUTSIDE_SETTING_FAILED_CODE;
pub use ports::{
    ClaudeThreadTitles, FolderAuthority, GhPullRequests, HiddenSessions, HostThreadConsults,
    ListDirAuthority, PullRequestLookup, PullRequests, RuntimeThreadSessions, ShipTools,
    SystemShipTools, SystemThreadGit, ThreadClock, ThreadConsults, ThreadGit, ThreadSessions,
    ThreadTitles, WorkerSessions,
};
pub use resume::PromptReceipts;
use run_tap::{PendingSteer, RunPaths, RunTrack};
pub use run_tap::{ThreadRegistrySignal, ThreadRunTap};
pub use schedules::THREADS_SCHEDULES_UNAVAILABLE_CODE;
pub use secrets::{
    SECRET_NOTE, SECRET_PENDING, SECRET_WAIT, THREAD_SECRET_FAILED_CODE,
    THREAD_SECRET_REFUSED_CODE, THREAD_SECRET_UNAVAILABLE_CODE,
};
pub use side::{THREADS_SIDE_REFUSED_CODE, THREADS_SIDE_UNAVAILABLE_CODE};
pub use stash::{DRAFT_STASH_FAILED_CODE, DRAFT_STASH_REFUSED_CODE, DRAFT_STASH_UNAVAILABLE_CODE};
pub use switch::{
    THREADS_SWITCH_MODE_CODE, THREADS_SWITCH_NOT_READY_CODE, THREADS_SWITCH_UNAVAILABLE_CODE,
};
pub use team::ThreadTeamConfig;

use crate::dispatcher::{
    DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE, OPERATION_NOT_MIGRATED_CODE,
    SharedRegistryPushes,
};
use crate::protocol::PushFrame;

/// The operations [`ThreadHost`] owns: its commands, (protocol 56) the
/// thread journal's reads, (protocol 63) each thread's stashed drafts and
/// (protocol 67) the secrets the owner gives its agents.
pub const THREAD_OPERATIONS: [&str; 4] = [
    CHANNELS.thread_control,
    CHANNELS.thread_journal,
    CHANNELS.draft_stash,
    CHANNELS.thread_secret,
];

/// The store did not open this launch. Definitely unsent.
pub const THREADS_STORE_UNAVAILABLE_CODE: &str = "threads-store-unavailable";
/// A commit failed; replay-safe only when nothing can have reached disk.
pub const THREADS_STORE_FAILED_CODE: &str = "threads-store-failed";
/// Claude is not installed, so no title can be generated. Definitely unsent.
pub const THREADS_TITLES_UNAVAILABLE_CODE: &str = "threads-titles-unavailable";
/// Claude answered no usable title; nothing was renamed. Definitely unsent.
pub const THREADS_TITLE_FAILED_CODE: &str = "threads-title-failed";
pub const THREADS_FOLDER_REFUSED_CODE: &str = "threads-folder-refused";
pub const THREADS_WORKSPACE_FAILED_CODE: &str = "threads-workspace-failed";
pub const THREADS_WORKTREE_UNAVAILABLE_CODE: &str = "threads-worktree-unavailable";
pub const THREADS_WORKTREE_DIRTY_CODE: &str = "threads-worktree-dirty";
pub const THREADS_GIT_FAILED_CODE: &str = "threads-git-failed";
pub const THREADS_NO_GIT_CODE: &str = "threads-no-git";
pub const THREADS_UNKNOWN_SESSION_CODE: &str = "threads-unknown-session";
pub const THREADS_FIRST_TURN_CODE: &str = "threads-first-turn";
pub const THREADS_PULL_REQUESTS_UNAVAILABLE_CODE: &str = "threads-pull-requests-unavailable";
pub const THREADS_REGISTRY_UNAVAILABLE_CODE: &str = "threads-registry-unavailable";
/// `gh` is missing, signed out or not answering; the message says how to fix
/// it. Definitely unsent.
pub const THREADS_GH_UNAVAILABLE_CODE: &str = "threads-gh-unavailable";
/// git or `gh` refused a ship effect, and a re-read agrees nothing changed.
pub const THREADS_SHIP_REFUSED_CODE: &str = "threads-ship-refused";
/// The files Commit was given are no longer the files it would stage.
pub const THREADS_SHIP_STALE_CODE: &str = "threads-ship-stale";
/// The checkout may not be pushed as it is (the default branch, no remote).
pub const THREADS_SHIP_BLOCKED_CODE: &str = "threads-ship-blocked";
pub const THREADS_NO_PULL_REQUEST_CODE: &str = "threads-no-pull-request";
pub const THREADS_CHECKS_NOT_FAILING_CODE: &str = "threads-checks-not-failing";
/// Write with AI (protocol 67): Claude is not installed or is signed out.
/// Definitely unsent.
pub const THREADS_SHIP_TEXT_UNAVAILABLE_CODE: &str = "threads-ship-text-unavailable";
/// Write with AI: Claude answered no usable text. Definitely unsent.
pub const THREADS_SHIP_TEXT_FAILED_CODE: &str = "threads-ship-text-failed";
const THREADS_INTERNAL_CODE: &str = "threads-internal";

/// What [`ThreadHost::new`] needs.
pub struct ThreadHostConfig {
    /// The opened store, or why it did not open (every command then refuses).
    pub store: Result<ThreadStore, String>,
    pub ports: ThreadPorts,
    /// Carries `iface:threads`.
    pub pushes: Option<mpsc::Sender<PushFrame>>,
    /// Scheduled after a fork registers a session outside session management.
    pub registry_pushes: Option<SharedRegistryPushes>,
    /// Rung after each shared registry round.
    pub registry: Arc<ThreadRegistrySignal>,
    /// The thread journal's place (none: threads are not journalled).
    pub journal: Option<ThreadJournalConfig>,
}

/// Everything outside the store a thread reaches.
#[derive(Clone)]
pub struct ThreadPorts {
    pub sessions: Arc<dyn ThreadSessions>,
    pub hidden: Arc<dyn HiddenSessions>,
    pub authority: Arc<dyn FolderAuthority>,
    pub git: Arc<dyn ThreadGit>,
    pub pull_requests: Arc<dyn PullRequests>,
    /// The GitHub CLI behind the ship commands.
    pub ship: Arc<dyn ShipTools>,
    pub titles: Arc<dyn ThreadTitles>,
    pub clock: ThreadClock,
    /// Who else drives a provider session: Open in terminal holds a session
    /// for its pane; Continue as thread waits for a pane to let go.
    pub claims: Arc<interface_runtime::SessionClaims>,
    /// The consult path Second opinion goes through (review B4).
    pub consults: Arc<dyn ThreadConsults>,
}

/// Serves `iface:threadControl` and emits the `iface:threads` push.
#[derive(Clone)]
pub struct ThreadHost {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<tokio::sync::Mutex<Result<ThreadStore, String>>>,
    ports: ThreadPorts,
    pushes: Option<mpsc::Sender<PushFrame>>,
    registry_pushes: Option<SharedRegistryPushes>,
    registry: Arc<ThreadRegistrySignal>,
    /// The run tap's per-session state, fed inline by the event pumps.
    runs: Mutex<HashMap<String, RunTrack>>,
    /// The last run generation handed out (see `RunTrack`).
    run_generations: AtomicU64,
    /// The files each session's own edits named in its open run and the run
    /// it last ended, by session; locked after `runs`, never with `steers`.
    run_paths: Mutex<HashMap<String, RunPaths>>,
    /// Sends steered into a running turn, by session; locked after `runs`
    /// when both are held.
    steers: Mutex<HashMap<String, PendingSteer>>,
    ops: mpsc::UnboundedSender<run_tap::RunOp>,
    ops_receiver: Mutex<Option<mpsc::UnboundedReceiver<run_tap::RunOp>>>,
    /// Wakes the worker to recompute its next timer after a commit.
    timers: Notify,
    /// One send at a time per thread, from its checkpoint to its delivery.
    send_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One title change at a time per session: an owner's rename, or a
    /// generated title from its check to its record.
    title_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// One commit, branch, push or merge at a time per thread.
    ship_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Where each thread's automatic title stands this launch: asked for as
    /// its first prompt goes out, retried once after its first completed
    /// run when that failed.
    titled: Mutex<HashMap<String, titles::AutoTitle>>,
    /// Worktree releases in flight, so a reconcile never starts one twice.
    releasing: Mutex<HashSet<String>>,
    /// A pull request pass is running.
    pull_request_pass: AtomicBool,
    /// Protocol 71: the owner's saved "Pane tools without asking" switch, as
    /// views show it. Launches read it from the launch catalog, which
    /// `provider_refresh` rebuilds when it changes.
    pane_tools: AtomicBool,
    /// Rebuilds the launch catalog after the pane tools switch changes;
    /// attached at startup.
    provider_refresh: std::sync::OnceLock<Arc<crate::provider_refresh::ProviderRefresh>>,
    /// Threads an Away loop holds while it runs, by key.
    away_holds: Mutex<HashMap<String, away::AwayHold>>,
    /// Each Away loop's projection, by its thread's key.
    away_threads: Mutex<HashMap<String, interface_contracts::away::ThreadAway>>,
    /// What the last reconcile read from the registry, hashed.
    registry_seen: Mutex<Option<u64>>,
    /// The registry sessions the last reconcile hid as workers, sorted: every
    /// view carries them for Chat's session list (protocol 58).
    hidden_listed: Mutex<Vec<String>>,
    /// Protocol 60: the provider accounts sessions are bound to; unset when
    /// the host has none (every thread then runs on its Default account).
    accounts: std::sync::OnceLock<Arc<crate::provider_accounts::ProviderAccounts>>,
    /// End checkpoints, releases and pull request passes; drained at shutdown.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    stop: watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
    journal: Option<journal::JournalHandle>,
    /// Team tools (protocol 57), attached once the guarded runtime exists.
    team: std::sync::OnceLock<team::TeamParts>,
    /// Bumped whenever something a team tools wait or notice watches may
    /// have changed: a run change, an approval, a commit, a journal batch.
    changes: watch::Sender<u64>,
    /// Prompts Interface sends a thread itself, by key (their digest): the
    /// send hook shows them and journals them as the host's.
    host_prompts: Mutex<HashMap<String, String>>,
    /// Tests: told when a team tools call starts waiting for its thread's
    /// bind.
    #[cfg(test)]
    bind_waits: Arc<Notify>,
    /// Protocol 63: each thread's stashed drafts, attached at startup.
    stash: std::sync::OnceLock<stash::SharedStash>,
    /// Sessions whose `iface_team` server is bound now: a thread reference
    /// they send mentions `read_thread`.
    team_bound: Mutex<HashSet<String>>,
    /// Scheduled prompts and pull-request watch (protocol 62), attached
    /// once the store is open.
    schedules: std::sync::OnceLock<schedules::ScheduleParts>,
    /// Each session's usage limit and the ports Resume at reset reads
    /// (protocol 66).
    resume: resume::ResumeParts,
    /// Protocol 67: the secrets agents ask for, attached once the data
    /// root is known.
    secrets: std::sync::OnceLock<secrets::SecretParts>,
    /// Protocol 67: each thread's open side question, attached at startup.
    sides: std::sync::OnceLock<side::SharedSides>,
    /// The side threads, hidden from Threads and Chat and resumed one hop
    /// down; kept beside the store so a reconcile never waits on it.
    side_keys: Mutex<std::collections::BTreeSet<String>>,
    /// Protocol 72: folder trust, attached at startup.
    folder_trust: std::sync::OnceLock<Arc<dyn folder_trust::ThreadFolderTrustPort>>,
}

impl ThreadHost {
    #[must_use]
    pub fn new(config: ThreadHostConfig) -> Self {
        let (ops, ops_receiver) = mpsc::unbounded_channel();
        let (stop, _) = watch::channel(false);
        Self {
            inner: Arc::new(Inner {
                pane_tools: AtomicBool::new(
                    config
                        .store
                        .as_ref()
                        .is_ok_and(ThreadStore::pane_tools_without_asking),
                ),
                provider_refresh: std::sync::OnceLock::new(),
                store: Arc::new(tokio::sync::Mutex::new(config.store)),
                ports: config.ports,
                pushes: config.pushes,
                registry_pushes: config.registry_pushes,
                registry: config.registry,
                runs: Mutex::new(HashMap::new()),
                run_generations: AtomicU64::new(0),
                run_paths: Mutex::new(HashMap::new()),
                steers: Mutex::new(HashMap::new()),
                ops,
                ops_receiver: Mutex::new(Some(ops_receiver)),
                timers: Notify::new(),
                send_gates: Mutex::new(HashMap::new()),
                title_gates: Mutex::new(HashMap::new()),
                ship_gates: Mutex::new(HashMap::new()),
                titled: Mutex::new(HashMap::new()),
                releasing: Mutex::new(HashSet::new()),
                pull_request_pass: AtomicBool::new(false),
                away_holds: Mutex::new(HashMap::new()),
                away_threads: Mutex::new(HashMap::new()),
                registry_seen: Mutex::new(None),
                hidden_listed: Mutex::new(Vec::new()),
                accounts: std::sync::OnceLock::new(),
                tasks: Mutex::new(Vec::new()),
                stop,
                worker: Mutex::new(None),
                journal: config.journal.map(journal::JournalHandle::new),
                team: std::sync::OnceLock::new(),
                changes: watch::channel(0).0,
                host_prompts: Mutex::new(HashMap::new()),
                #[cfg(test)]
                bind_waits: Arc::new(Notify::new()),
                stash: std::sync::OnceLock::new(),
                team_bound: Mutex::new(HashSet::new()),
                schedules: std::sync::OnceLock::new(),
                resume: resume::ResumeParts::default(),
                secrets: std::sync::OnceLock::new(),
                sides: std::sync::OnceLock::new(),
                side_keys: Mutex::new(std::collections::BTreeSet::new()),
                folder_trust: std::sync::OnceLock::new(),
            }),
        }
    }

    /// Opens `threads.v2.json` (carrying a `threads.v1.json` over) in the
    /// private data root, beside
    /// `harness.v1.json`. A corrupt or unknown file is quarantined and the
    /// store starts empty, its revision at the current time; any other
    /// failure is the reason every command will give. A store still holding
    /// the old default of generated titles (off) has them turned on once
    /// ([`ThreadStore::turn_generated_titles_on_once`]).
    ///
    /// # Errors
    /// Why the store could not open.
    pub fn open_store(data_root: &Path) -> Result<ThreadStore, String> {
        match ThreadStore::open(&data_root.join(THREADS_FILE_NAME), now_ms()) {
            Ok(mut store) => {
                if store.quarantined() {
                    tracing::warn!(
                        "thread state was unreadable; it was quarantined and threads start fresh"
                    );
                }
                // Generated titles became the default: a store holding the
                // old default (`false`) has them turned on, once.
                match store.turn_generated_titles_on_once(now_ms()) {
                    Ok(true) => tracing::info!("generated thread titles were turned on once"),
                    Ok(false) => {}
                    Err(error) => {
                        tracing::warn!(%error, "generated thread titles were left as they were");
                    }
                }
                Ok(store)
            }
            Err(error) => {
                tracing::error!(%error, "thread state did not open; threads are unavailable");
                Err(error.to_string())
            }
        }
    }

    /// Queues one `iface:threads` push carrying the whole `view`. Answers
    /// whether it was queued: false without a push queue, or once the queue
    /// has closed at shutdown.
    pub async fn push_view(&self, view: &ThreadsView) -> bool {
        let Some(pushes) = &self.inner.pushes else {
            return false;
        };
        let Some(frame) = threads_push(view) else {
            return false;
        };
        pushes.send(frame).await.is_ok()
    }

    /// Queues one session event push for `key`'s pane, as the event pump
    /// does. Answers whether it was queued.
    pub(crate) async fn push_session_event(
        &self,
        key: &str,
        event: &interface_contracts::engine::EngineEvent,
    ) -> bool {
        let Some(pushes) = &self.inner.pushes else {
            return false;
        };
        let Ok(frame) = crate::dispatcher::session_event_frame(key, event) else {
            return false;
        };
        pushes.send(frame).await.is_ok()
    }

    fn now(&self) -> u64 {
        (self.inner.ports.clock)()
    }

    /// Runs `work` on the store on the blocking pool, holding the store lock
    /// until the view of any commit it made has been pushed. Answers the
    /// work's value and the view after it.
    async fn with_store<T, F>(&self, work: F) -> Result<(T, ThreadsView), DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ThreadStore, u64) -> Result<T, ThreadStoreError> + Send + 'static,
    {
        let (value, view) = self.with_store_inner(true, work).await?;
        Ok((value, view.expect("requested view")))
    }

    /// Runs `work` on the store on the blocking pool, building and pushing a
    /// [`ThreadsView`] only when the document revision changed.
    async fn with_store_quiet<T, F>(&self, work: F) -> Result<T, DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ThreadStore, u64) -> Result<T, ThreadStoreError> + Send + 'static,
    {
        let (value, _) = self.with_store_inner(false, work).await?;
        Ok(value)
    }

    async fn with_store_inner<T, F>(
        &self,
        always_view: bool,
        work: F,
    ) -> Result<(T, Option<ThreadsView>), DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ThreadStore, u64) -> Result<T, ThreadStoreError> + Send + 'static,
    {
        let mut guard = Arc::clone(&self.inner.store).lock_owned().await;
        let clock = Arc::clone(&self.inner.ports.clock);
        let (guard, outcome) = tokio::task::spawn_blocking(move || {
            let now = clock();
            let outcome = match guard.as_mut() {
                Err(reason) => Err(store_unavailable(reason)),
                Ok(store) => {
                    let before = store.document().revision;
                    work(store, now).map_err(store_error).map(|value| {
                        let document = store.document();
                        let changed = document.revision != before;
                        let view =
                            (changed || always_view).then(|| document.view(now, worktree_missing));
                        (value, view, changed)
                    })
                }
            };
            (guard, outcome)
        })
        .await
        .map_err(|_| internal("The thread store task stopped unexpectedly."))?;
        let (value, mut view, changed) = outcome?;
        if let Some(view) = &mut view {
            self.with_away(view);
            self.with_hidden(view);
            self.with_accounts(view);
            self.with_schedules(view);
            self.with_outside(view);
            self.with_limits(view);
            self.with_secrets(view);
            self.with_sides(view);
            self.with_folder_trust(view);
            if changed {
                self.push_view(view).await;
                self.inner.timers.notify_one();
            }
        }
        if changed {
            self.team_changed();
        }
        drop(guard);
        Ok((value, view))
    }

    /// Applies one document edit and commits it when it changed anything.
    async fn edit<T, F>(&self, edit: F) -> Result<(T, ThreadsView), DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ThreadsDocument, u64) -> Result<T, ThreadRefusal> + Send + 'static,
    {
        self.with_store(move |store, now| {
            store
                .apply(now, |document| edit(document, now))
                .map(|(value, _)| value)
        })
        .await
    }

    /// Applies one document edit and commits it when it changed anything,
    /// skipping the [`ThreadsView`] projection when nothing changed.
    async fn edit_quiet<T, F>(&self, edit: F) -> Result<T, DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ThreadsDocument, u64) -> Result<T, ThreadRefusal> + Send + 'static,
    {
        self.with_store_quiet(move |store, now| {
            store
                .apply(now, |document| edit(document, now))
                .map(|(value, _)| value)
        })
        .await
    }

    /// Reads the document without changing it, releasing the store lock before
    /// checking worktree directories on the blocking pool.
    async fn read<T, F>(&self, read: F) -> Result<(T, ThreadsView), DispatchError>
    where
        T: Send + 'static,
        F: FnOnce(&ThreadsDocument, u64) -> T + Send + 'static,
    {
        let (value, document, now) = {
            let guard = self.inner.store.lock().await;
            let store = guard.as_ref().map_err(|reason| store_unavailable(reason))?;
            let now = self.now();
            let document = store.document();
            (read(document, now), document.clone(), now)
        };
        let mut view = tokio::task::spawn_blocking(move || document.view(now, worktree_missing))
            .await
            .map_err(|_| internal("The thread store task stopped unexpectedly."))?;
        self.with_away(&mut view);
        self.with_hidden(&mut view);
        self.with_accounts(&mut view);
        self.with_schedules(&mut view);
        self.with_outside(&mut view);
        self.with_limits(&mut view);
        self.with_secrets(&mut view);
        self.with_sides(&mut view);
        self.with_folder_trust(&mut view);
        Ok((value, view))
    }

    /// A metadata command on `key`. A session the registry has but the
    /// store has not seeded yet is reconciled in first.
    async fn metadata<F>(&self, key: &str, edit: F) -> Result<ThreadControlResponse, DispatchError>
    where
        F: FnOnce(&mut ThreadsDocument, u64) -> Result<(), ThreadRefusal> + Send + 'static,
    {
        let owned = key.to_owned();
        let known = self
            .peek(move |document, _| document.record(&owned).is_some())
            .await?;
        if !known {
            self.reconcile().await;
        }
        let ((), view) = self.edit(edit).await?;
        Ok(view.into())
    }

    async fn control(
        &self,
        command: ThreadCommand,
    ) -> Result<ThreadControlResponse, DispatchError> {
        match command {
            ThreadCommand::Read {} => Ok(self.read(|_, _| ()).await?.1.into()),
            ThreadCommand::Workflow { command } => self.workflow_command(command).await,
            ThreadCommand::Pin { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| document.pin(&target, now))
                    .await
            }
            ThreadCommand::Unpin { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| document.unpin(&target, now))
                    .await
            }
            ThreadCommand::Reorder {
                key,
                section,
                before_key,
            } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| {
                    document.reorder(&target, section, before_key.as_deref(), now)
                })
                .await
            }
            ThreadCommand::Settle { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| {
                    document.settle(&target, SettledReason::Manual, now)
                })
                .await
            }
            ThreadCommand::Unsettle { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| document.unsettle(&target, now))
                    .await
            }
            ThreadCommand::Snooze { key, until } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| {
                    document.snooze(&target, until, now)
                })
                .await
            }
            ThreadCommand::Unsnooze { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, _| document.unsnooze(&target))
                    .await
            }
            ThreadCommand::Visit { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, now| document.visit(&target, now))
                    .await
            }
            ThreadCommand::MarkUnread { key } => {
                let target = key.clone();
                self.metadata(&key, move |document, _| document.mark_unread(&target))
                    .await
            }
            ThreadCommand::SetAutoSettle { key, enabled } => {
                let target = key.clone();
                self.metadata(&key, move |document, _| {
                    document.set_auto_settle(&target, enabled)
                })
                .await
            }
            ThreadCommand::SetSettings { settings } => self.set_settings(settings).await,
            ThreadCommand::RegenerateTitle { key } => self.regenerate_title(key).await,
            ThreadCommand::OpenInTerminal { key } => {
                // Its loop keeps the session until it ends.
                self.away_unheld(&key)?;
                self.open_in_terminal(key).await
            }
            ThreadCommand::SecondOpinion {
                key,
                engine,
                request_id,
            } => {
                // A consult's reply would be sent into the loop's session.
                self.away_unheld(&key)?;
                self.second_opinion(key, engine, request_id).await
            }
            ThreadCommand::ContinueWith {
                key,
                engine,
                account,
            } => self.continue_with(key, engine, account).await,
            ThreadCommand::SwitchProvider { key, engine, model } => {
                self.switch_provider(key, engine, model).await
            }
            ThreadCommand::ContinueFromTerminal { key } => self.continue_from_terminal(key).await,
            // Protocol 67: side questions.
            ThreadCommand::SideAsk { key, question } => self.side_ask(key, question).await,
            ThreadCommand::SideClose { key } => self.side_close(key).await,
            // Protocol 72: folder trust.
            ThreadCommand::FolderTrust { folder } => self.folder_trust(folder).await,
            ThreadCommand::TrustFolder { folder, trust } => self.trust_folder(folder, trust).await,
            // Workspace, ship and (protocol 62) schedule commands.
            command => self.workspace_command(command).await,
        }
    }

    /// Settings → Threads. Protocol 65: the outside agents level lives in
    /// the team tools ledger, never in the Threads store. Protocol 71: pane
    /// tools without asking lives beside the store, never in it.
    async fn set_settings(
        &self,
        settings: interface_contracts::threads::ThreadSettings,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let level_changed = self.set_outside_agents(settings.outside_agents).await?;
        let pane_tools_changed = self
            .set_pane_tools(settings.pane_tools_without_asking)
            .await?;
        let ((), mut view) = self
            .edit(move |document, _| document.set_settings(settings))
            .await?;
        self.inner.timers.notify_one();
        if level_changed || pane_tools_changed {
            self.with_outside(&mut view);
            self.push_view(&view).await;
        }
        Ok(view.into())
    }

    /// Records the owner's "Pane tools without asking" switch beside the
    /// store. Answers whether it changed.
    async fn set_pane_tools(&self, on: bool) -> Result<bool, DispatchError> {
        if self
            .inner
            .pane_tools
            .load(std::sync::atomic::Ordering::SeqCst)
            == on
        {
            return Ok(false);
        }
        self.with_store_quiet(move |store, _| store.set_pane_tools_without_asking(on))
            .await?;
        self.inner
            .pane_tools
            .store(on, std::sync::atomic::Ordering::SeqCst);
        // Panes opened from now on follow it; running panes keep theirs.
        if let Some(refresh) = self.inner.provider_refresh.get().cloned() {
            self.spawn_tracked(async move {
                if let Err(error) = refresh.rebuild_after_setting_change().await {
                    tracing::warn!(
                        %error,
                        "the pane tools switch applies to panes after Interface restarts"
                    );
                }
            });
        }
        Ok(true)
    }

    /// The refresher that rebuilds the launch catalog when the pane tools
    /// switch changes.
    pub fn attach_provider_refresh(&self, refresh: Arc<crate::provider_refresh::ProviderRefresh>) {
        let _ = self.inner.provider_refresh.set(refresh);
    }

    /// The current Threads view, read without changing anything (the phone's
    /// Threads list and detail read it).
    ///
    /// # Errors
    /// Why the store cannot be read.
    pub async fn view(&self) -> Result<ThreadsView, DispatchError> {
        Ok(self.read(|_, _| ()).await?.1)
    }

    /// The registry's run tap for this host, ahead of `next`.
    #[must_use]
    pub fn run_tap(&self, next: Arc<dyn crate::dispatcher::SessionEventObserver>) -> ThreadRunTap {
        ThreadRunTap::new(self.clone(), next)
    }

    fn spawn_tracked(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let handle = tokio::spawn(task);
        let mut tasks = lock(&self.inner.tasks);
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }
}

#[async_trait]
impl HostDispatcher for ThreadHost {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation == CHANNELS.thread_journal {
            return self.journal_command(payload).await;
        }
        if operation == CHANNELS.draft_stash {
            return self.stash_command(payload).await;
        }
        if operation == CHANNELS.thread_secret {
            return self.secret_command(payload).await;
        }
        if operation != CHANNELS.thread_control {
            return Err(DispatchError::new(
                OPERATION_NOT_MIGRATED_CODE,
                "This Interface operation has not migrated to the Rust host.",
                false,
                true,
            ));
        }
        let command = crate::payload::decode_object::<ThreadCommand, _>(payload, |error| {
            if let Some(error) = error {
                tracing::warn!(%error, "rejected an invalid thread command");
            }
            DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "The thread command does not match the Threads v1 schema.",
                false,
                true,
            )
        })?;
        let response = self.control(command).await?;
        serde_json::to_value(response).map_err(|error| {
            DispatchError::new(
                "threads-encode-failed",
                format!("The threads view could not be encoded: {error}"),
                false,
                false,
            )
        })
    }
}

fn worktree_missing(path: &str) -> bool {
    !Path::new(path).is_dir()
}

fn refused(refusal: ThreadRefusal) -> DispatchError {
    DispatchError::new(refusal.code, refusal.message, false, true)
}

fn store_unavailable(reason: &str) -> DispatchError {
    DispatchError::new(
        THREADS_STORE_UNAVAILABLE_CODE,
        format!(
            "Thread state could not be opened ({reason}). Existing data was kept; restart Interface to try again."
        ),
        false,
        true,
    )
}

fn store_error(error: ThreadStoreError) -> DispatchError {
    match error {
        ThreadStoreError::Refused(refusal) => refused(refusal),
        ThreadStoreError::Storage(error) => {
            // Only these prove nothing reached disk: a latched store refuses
            // before writing, and a bounds or pre-rename failure wrote nothing.
            let unsent = matches!(
                error,
                SettingsError::InvalidAgent(_)
                    | SettingsError::Storage(
                        StorageError::WriteFailed { .. } | StorageError::BoundsExceeded { .. }
                    )
            );
            tracing::error!(%error, "thread state could not be saved");
            DispatchError::new(
                THREADS_STORE_FAILED_CODE,
                format!(
                    "Thread state could not be saved: {error}. Try again; restart Interface if it keeps failing."
                ),
                false,
                unsent,
            )
        }
    }
}

fn registry_unavailable(error: &str) -> DispatchError {
    DispatchError::new(
        THREADS_REGISTRY_UNAVAILABLE_CODE,
        format!("The session registry could not be read ({error}). Try again."),
        true,
        true,
    )
}

fn internal(message: &str) -> DispatchError {
    DispatchError::new(THREADS_INTERNAL_CODE, message, false, false)
}

/// The `iface:threads` frame for `view`, or `None` (logged) when it cannot be built.
fn threads_push(view: &ThreadsView) -> Option<PushFrame> {
    let payload = match serde_json::to_value(view) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::error!(%error, "the threads push could not be encoded");
            return None;
        }
    };
    match PushFrame::new(CHANNELS.threads, payload) {
        Ok(frame) => Some(frame),
        Err(error) => {
            tracing::error!(code = %error.code, "the threads push could not be framed");
            None
        }
    }
}

/// Milliseconds since the Unix epoch, the production [`ThreadClock`].
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
#[path = "threads_tests.rs"]
pub(crate) mod tests;
