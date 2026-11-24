//! The thread journal's host side (protocol 56; contract:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 1): the
//! tap that hands every relevant engine event and run change to one journal
//! worker in pump order, the prompts the send hook and `Bind` capture, the
//! worker that writes `interface_settings::thread_journal`, the shutdown
//! drain, the one-time import, the history fallback and the
//! `iface:threadJournal` owner.
//!
//! The observer does no I/O and never awaits: it sends one op over an
//! unbounded channel. The worker owns the one [`ThreadJournal`] writer; the
//! journal's rules (what an event becomes, bounds, retention) are the
//! settings crate's. What is decided here is when entries are written:
//!
//! - Only sessions with a thread record are journalled. A session the
//!   journal has not seen a prompt for is held in memory (at most
//!   [`HOLD_CAP`] entries, [`HOLD_MS`]) until its launch prompt arrives from
//!   `Bind`, which puts the prompt before the entries it started; past the
//!   hold, at the session's close or at shutdown the worker asks the store
//!   whether it has a record, then writes the held entries or drops them.
//! - A turn's end fsyncs that thread; other appends are fsynced at most
//!   [`SYNC_MS`] later and in the drain.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::{
    engine::{EngineEvent, EngineEventKind},
    ipc::{HistoryAvailability, HistoryPage},
    thread_journal::{
        DEFAULT_JOURNAL_PAGE_ENTRIES, JournalEntry, ThreadJournalCommand, ThreadJournalResponse,
        TurnOrigin,
    },
};
use interface_settings::{
    thread_journal::{self, JOURNAL_DIR, JournalFold, ThreadJournal, import_entries_from},
    thread_visuals::{self, SavedVisual, ThreadVisuals},
    threads::prompt_digest,
};
use serde_json::Value;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{ThreadHost, run_tap::RunChange};
use crate::{
    diagnostics::{SetAsideStore, StoreNotices},
    dispatcher::{DispatchError, HistoryFallback, INVALID_PAYLOAD_CODE},
};

/// Entries held for a session with no prompt yet are written or dropped
/// after this long...
pub(crate) const HOLD_MS: u64 = 30_000;
/// ...or once this many are held.
pub(crate) const HOLD_CAP: usize = 2_000;
/// An append is fsynced at most this long after it was written.
pub(crate) const SYNC_MS: u64 = 2_000;
/// Threads imported from their provider's history in one launch.
pub(crate) const IMPORT_LIMIT: usize = 200;
/// A search answers with what it found by then.
const SEARCH_DEADLINE: Duration = Duration::from_secs(2);
/// Ops the worker takes in one batch.
const BATCH: usize = 512;

/// What the worker is told, in the order it happened.
#[derive(Debug)]
pub(crate) enum JournalOp {
    /// An engine event, with the run change the tap decided for it.
    Event {
        key: String,
        at: u64,
        event: EngineEventKind,
        run: Option<RunChange>,
    },
    /// A prompt: the launch prompt from `Bind`, a send that opened its own
    /// turn, or a steer the provider took.
    Prompt {
        key: String,
        at: u64,
        origin: TurnOrigin,
        prompt: String,
    },
    /// The send `Prompt` recorded never reached the provider.
    Retract { key: String, at: u64 },
    /// A page an agent showed (T3 section 8): kept in the thread's visual
    /// store, then journalled as `visual` in order with the events around
    /// it. `reply` hears whether it was kept.
    Visual {
        key: String,
        at: u64,
        id: String,
        title: String,
        html: String,
        reply: oneshot::Sender<Result<SavedVisual, String>>,
    },
    /// The session was deleted or forgotten: its journal and its visuals go.
    Removed { key: String },
    /// A provider's history for a thread with no journal; `older` when the
    /// page still left the oldest part out.
    Import {
        key: String,
        events: Vec<EngineEvent>,
        older: bool,
    },
    /// A provider's whole history for a thread whose journal holds only an
    /// import made before imports read the whole history: it replaces that
    /// journal when it holds more and loses nothing.
    Rebuild {
        key: String,
        events: Vec<EngineEvent>,
        older: bool,
    },
    /// Write or drop what is held, fsync everything, then stop.
    Drain(oneshot::Sender<()>),
}

/// Where the journal lives and who hears about set-aside segments.
pub struct ThreadJournalConfig {
    pub data_root: PathBuf,
    pub notices: Option<Arc<StoreNotices>>,
}

/// The journal's channel and worker, held by [`ThreadHost`].
pub(crate) struct JournalHandle {
    ops: mpsc::UnboundedSender<JournalOp>,
    pub(crate) receiver: Mutex<Option<mpsc::UnboundedReceiver<JournalOp>>>,
    data_root: PathBuf,
    notices: Option<Arc<StoreNotices>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Counts searches: a search whose number is no longer the newest stops
    /// (the sidebar asks again as typing rests and draws only the newest).
    searches: Arc<AtomicU64>,
}

/// One search's place in line: cancelled once a newer one starts.
pub(crate) struct SearchTicket {
    searches: Arc<AtomicU64>,
    number: u64,
}

impl SearchTicket {
    pub(crate) fn cancelled(&self) -> bool {
        self.searches.load(Ordering::Acquire) != self.number
    }
}

impl JournalHandle {
    pub(crate) fn new(config: ThreadJournalConfig) -> Self {
        let (ops, receiver) = mpsc::unbounded_channel();
        Self {
            ops,
            receiver: Mutex::new(Some(receiver)),
            data_root: config.data_root,
            notices: config.notices,
            worker: Mutex::new(None),
            searches: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Starts a search, cancelling every earlier one still running.
    pub(crate) fn begin_search(&self) -> SearchTicket {
        SearchTicket {
            searches: Arc::clone(&self.searches),
            number: self.searches.fetch_add(1, Ordering::AcqRel) + 1,
        }
    }

    fn root(&self) -> PathBuf {
        self.data_root.join(JOURNAL_DIR)
    }

    /// The journal's directory, for team tools' reads.
    pub(crate) fn journal_root(&self) -> PathBuf {
        self.root()
    }

    fn send(&self, op: JournalOp) {
        // After the drain the worker is gone; there is nothing to tell.
        let _ = self.ops.send(op);
    }
}

/// Whether the journal needs to see `event` (or its run change).
fn relevant(event: &EngineEventKind, run: Option<RunChange>) -> bool {
    run.is_some()
        || matches!(
            event,
            EngineEventKind::MessageDelta { .. }
                | EngineEventKind::ThinkingSignal { .. }
                | EngineEventKind::SessionClosed
        )
        || thread_journal::is_journalled(event)
}

impl ThreadHost {
    /// The tap: hands `event` and its run change to the journal worker.
    /// No I/O, no waiting.
    pub(crate) fn journal_event(&self, key: &str, event: &EngineEventKind, run: Option<RunChange>) {
        let Some(journal) = &self.inner.journal else {
            return;
        };
        if !relevant(event, run) {
            return;
        }
        journal.send(JournalOp::Event {
            key: key.to_owned(),
            at: self.now(),
            event: event.clone(),
            run,
        });
    }

    pub(crate) fn journal_prompt(&self, key: &str, origin: TurnOrigin, prompt: &str) {
        if let Some(journal) = &self.inner.journal {
            journal.send(JournalOp::Prompt {
                key: key.to_owned(),
                at: self.now(),
                origin,
                prompt: prompt.to_owned(),
            });
        }
    }

    pub(crate) fn journal_retract(&self, key: &str) {
        if let Some(journal) = &self.inner.journal {
            journal.send(JournalOp::Retract {
                key: key.to_owned(),
                at: self.now(),
            });
        }
    }

    /// Keeps a visual's page and journals it, in order with the events the
    /// tap already handed over. Answers once the page is on disk (the
    /// journal entry is written in the same batch).
    ///
    /// # Errors
    /// Why it was not kept: no journal this launch, the store, or the write.
    pub(crate) async fn journal_visual(
        &self,
        key: &str,
        id: String,
        title: String,
        html: String,
    ) -> Result<SavedVisual, String> {
        let Some(journal) = &self.inner.journal else {
            return Err("Visuals are not available in this Interface.".into());
        };
        let (reply, answer) = oneshot::channel();
        journal.send(JournalOp::Visual {
            key: key.to_owned(),
            at: self.now(),
            id,
            title,
            html,
            reply,
        });
        answer
            .await
            .unwrap_or_else(|_| Err("The thread journal is not running.".into()))
    }

    pub(crate) fn journal_removed(&self, key: &str) {
        if let Some(journal) = &self.inner.journal {
            journal.send(JournalOp::Removed {
                key: key.to_owned(),
            });
        }
    }

    /// Starts the journal worker once: opens the journal (marking turns a
    /// restart cut off), starts the import, then writes ops until drained.
    pub(crate) fn start_journal(&self) {
        let Some(journal) = &self.inner.journal else {
            return;
        };
        let Some(ops) = lock(&journal.receiver).take() else {
            return;
        };
        let host = self.clone();
        let handle = tokio::spawn(async move { host.run_journal(ops).await });
        *lock(&journal.worker) = Some(handle);
    }

    /// The shutdown drain: everything sent before it is written, what is
    /// held is written or dropped, every thread is fsynced. Never aborted.
    pub async fn drain_journal(&self) {
        let Some(journal) = &self.inner.journal else {
            return;
        };
        let (done, drained) = oneshot::channel();
        journal.send(JournalOp::Drain(done));
        let worker = lock(&journal.worker).take();
        if let Some(worker) = worker {
            let _ = drained.await;
            let _ = worker.await;
        }
    }

    async fn run_journal(&self, mut ops: mpsc::UnboundedReceiver<JournalOp>) {
        let Some(mut core) = self.open_journal().await else {
            return discard(ops).await;
        };
        let import = self.start_import(&core).await;
        loop {
            let wait = core
                .next_deadline()
                .map(|at| Duration::from_millis(at.saturating_sub(self.now())));
            let (batch, closed) = next_batch(&mut ops, wait).await;
            let now = self.now();
            let Ok((next, outcome)) = tokio::task::spawn_blocking(move || {
                let mut outcome = core.apply_batch(batch, now);
                outcome.closed = closed;
                (core, outcome)
            })
            .await
            else {
                tracing::error!(
                    "the thread journal worker stopped; threads are no longer journalled"
                );
                import.abort();
                return discard(ops).await;
            };
            core = next;
            // A wait or a notice may be watching what this batch wrote.
            self.team_changed();
            let BatchOutcome {
                mut undecided,
                drain,
                closed,
            } = outcome;
            let stop = drain.is_some() || closed;
            if stop {
                undecided.extend(core.held_keys());
            }
            if undecided.is_empty() && !stop {
                continue;
            }
            let recorded = if undecided.is_empty() {
                HashSet::new()
            } else {
                self.recorded_keys().await
            };
            let now = self.now();
            let Ok(next) = tokio::task::spawn_blocking(move || {
                for key in undecided {
                    let has = recorded.contains(&key);
                    core.decide(&key, has, now);
                }
                if stop {
                    core.sync_all();
                }
                core
            })
            .await
            else {
                import.abort();
                return discard(ops).await;
            };
            core = next;
            if stop {
                import.abort();
                drop(core);
                if let Some(done) = drain {
                    let _ = done.send(());
                }
                return;
            }
        }
    }

    /// Opens the journal on the blocking pool, closes the turns a restart
    /// cut off and reports set-aside segments.
    async fn open_journal(&self) -> Option<JournalCore> {
        let journal = self.inner.journal.as_ref()?;
        let data_root = journal.data_root.clone();
        let now = self.now();
        let opened = tokio::task::spawn_blocking(move || {
            let mut journal = ThreadJournal::open(&data_root)?;
            let marked = journal.close_open_turns(now);
            Ok::<_, interface_settings::SettingsError>((journal, marked))
        })
        .await;
        let (mut opened, marked) = match opened {
            Ok(Ok(opened)) => opened,
            Ok(Err(error)) => {
                tracing::error!(%error, "the thread journal did not open; threads are not journalled this launch");
                return None;
            }
            Err(_) => {
                tracing::error!("the thread journal's open task stopped");
                return None;
            }
        };
        match marked {
            Ok(keys) if !keys.is_empty() => {
                tracing::info!(
                    count = keys.len(),
                    "thread journal turns a restart cut off were closed"
                );
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(%error, "a thread journal restart marker was not written");
            }
        }
        for artifact in opened.take_quarantined() {
            if let Some(notices) = &journal.notices {
                notices.record(SetAsideStore::ThreadJournal, &artifact);
            }
        }
        Some(JournalCore::new(opened))
    }

    /// Every key the Threads store has a record for (none when it is
    /// unavailable).
    async fn recorded_keys(&self) -> HashSet<String> {
        self.peek(|document, _| {
            document
                .records
                .iter()
                .map(|record| record.key.clone())
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// Imports, one at a time, the newest [`IMPORT_LIMIT`] threads with a
    /// record and no journal from their providers' whole history, and asks
    /// again for those whose journal holds only an import made when imports
    /// read the newest 500 items (rebuilt once when the history is longer).
    async fn start_import(&self, core: &JournalCore) -> JoinHandle<()> {
        let mut records = self
            .peek(|document, _| {
                document
                    .records
                    .iter()
                    .map(|record| {
                        let newest = record.last_run.as_ref().map_or(record.created_at, |run| {
                            run.started_at.max(record.created_at)
                        });
                        (newest, record.key.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
        records.sort_by(|left, right| right.cmp(left));
        let short = core
            .journal
            .short_import_candidates()
            .into_iter()
            .collect::<HashSet<_>>();
        let keys = records
            .into_iter()
            .map(|(_, key)| key)
            .filter_map(|key| {
                if core.journal.has_journal(&key) {
                    short.contains(&key).then_some((key, true))
                } else {
                    Some((key, false))
                }
            })
            .take(IMPORT_LIMIT)
            .collect::<Vec<_>>();
        let host = self.clone();
        tokio::spawn(async move {
            for (key, rebuild) in keys {
                let page = match host.inner.ports.sessions.import_history(&key).await {
                    Ok(page) if page.availability != Some(HistoryAvailability::Unavailable) => page,
                    // No history to import, or none readable now (the next
                    // launch tries again).
                    _ => continue,
                };
                let Some(journal) = &host.inner.journal else {
                    return;
                };
                let older = page.older_messages > 0;
                journal.send(if rebuild {
                    JournalOp::Rebuild {
                        key,
                        events: page.events,
                        older,
                    }
                } else {
                    JournalOp::Import {
                        key,
                        events: page.events,
                        older,
                    }
                });
            }
        })
    }

    /// Serves `iface:threadJournal`: reads only.
    pub(crate) async fn journal_command(&self, payload: Value) -> Result<Value, DispatchError> {
        let invalid = || {
            DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "The thread journal request does not match the protocol 56 schema.",
                false,
                true,
            )
        };
        let command =
            crate::payload::decode_object::<ThreadJournalCommand, _>(payload, |_| invalid())?;
        command.validate().map_err(|_| invalid())?;
        if let ThreadJournalCommand::Visual { key, id } = &command {
            return self.visual_answer(key, id).await;
        }
        let root = self.inner.journal.as_ref().map(JournalHandle::root);
        let ticket = matches!(command, ThreadJournalCommand::Search { .. })
            .then(|| self.inner.journal.as_ref().map(JournalHandle::begin_search))
            .flatten();
        let mut response =
            tokio::task::spawn_blocking(move || answer(root, command, ticket.as_ref()))
                .await
                .map_err(|_| {
                    DispatchError::new(
                        "thread-journal-internal",
                        "The thread journal read stopped unexpectedly.",
                        false,
                        true,
                    )
                })?;
        // A hit in an earlier segment of a switched thread is the thread's
        // own (T3 section 5): reported under its head, the row it lists as.
        if let Some(hits) = response.hits.as_mut()
            && let Ok(heads) = self
                .peek(|document, _| {
                    hits.iter()
                        .map(|hit| document.head_of(&hit.key).map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .await
        {
            for (hit, head) in hits.iter_mut().zip(heads) {
                if let Some(head) = head {
                    hit.key = head;
                }
            }
        }
        serde_json::to_value(response).map_err(|error| {
            DispatchError::new(
                "thread-journal-encode-failed",
                format!("The thread journal answer could not be encoded: {error}"),
                false,
                true,
            )
        })
    }
}

impl ThreadHost {
    /// `visual {key, id}`: the page, found in any session of `key`'s thread
    /// (its head and earlier segments), or no `visual` when none keeps it.
    async fn visual_answer(&self, key: &str, id: &str) -> Result<Value, DispatchError> {
        let mut response = ThreadJournalResponse::default();
        if let Some(journal) = &self.inner.journal {
            let data_root = journal.data_root.clone();
            let head = self.head_key(key).await;
            let mut keys = vec![head.clone()];
            keys.extend(self.segments_of(&head).await);
            if !keys.iter().any(|member| member == key) {
                keys.push(key.to_owned());
            }
            let id = id.to_owned();
            response.visual = tokio::task::spawn_blocking(move || {
                keys.iter().find_map(|member| {
                    thread_visuals::read_visual(&data_root, member, &id).map(|html| {
                        interface_contracts::thread_journal::ThreadVisual {
                            id: id.clone(),
                            html,
                        }
                    })
                })
            })
            .await
            .ok()
            .flatten();
        }
        serde_json::to_value(response).map_err(|error| {
            DispatchError::new(
                "thread-journal-encode-failed",
                format!("The thread journal answer could not be encoded: {error}"),
                false,
                true,
            )
        })
    }
}

/// The answer to one read command; with no journal, an empty one. A search
/// stops early once `ticket` is cancelled (its answer is no longer drawn).
fn answer(
    root: Option<PathBuf>,
    command: ThreadJournalCommand,
    ticket: Option<&SearchTicket>,
) -> ThreadJournalResponse {
    let empty_page = |key: String| interface_contracts::thread_journal::JournalPage {
        key,
        entries: Vec::new(),
        first_seq: None,
        last_seq: 0,
        floor_seq: 0,
        has_more: false,
    };
    match (command, root) {
        (
            ThreadJournalCommand::Read {
                key,
                after_seq,
                limit,
            },
            Some(root),
        ) => ThreadJournalResponse {
            page: Some(thread_journal::read(
                &root,
                &key,
                after_seq,
                limit.unwrap_or(DEFAULT_JOURNAL_PAGE_ENTRIES),
            )),
            hits: None,
            visual: None,
        },
        (ThreadJournalCommand::Tail { key, limit }, Some(root)) => ThreadJournalResponse {
            page: Some(thread_journal::tail(
                &root,
                &key,
                limit.unwrap_or(DEFAULT_JOURNAL_PAGE_ENTRIES),
            )),
            hits: None,
            visual: None,
        },
        (ThreadJournalCommand::Search { query }, Some(root)) => ThreadJournalResponse {
            page: None,
            hits: Some(
                thread_journal::search_with(
                    &root,
                    &query,
                    Instant::now() + SEARCH_DEADLINE,
                    &|| ticket.is_some_and(SearchTicket::cancelled),
                )
                .hits,
            ),
            visual: None,
        },
        (ThreadJournalCommand::Read { key, .. } | ThreadJournalCommand::Tail { key, .. }, None) => {
            ThreadJournalResponse {
                page: Some(empty_page(key)),
                hits: None,
                visual: None,
            }
        }
        (ThreadJournalCommand::Search { .. }, None) => ThreadJournalResponse {
            page: None,
            hits: Some(Vec::new()),
            visual: None,
        },
        // Answered before reaching here (it needs the thread's chain).
        (ThreadJournalCommand::Visual { .. }, _) => ThreadJournalResponse::default(),
    }
}

#[async_trait]
impl HistoryFallback for ThreadHost {
    /// A thread whose provider keeps no history shows its journal.
    async fn history(&self, session_key: &str, limit: u64) -> Option<HistoryPage> {
        let root = self.inner.journal.as_ref()?.root();
        let key = session_key.to_owned();
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        tokio::task::spawn_blocking(move || thread_journal::replay_history(&root, &key, limit))
            .await
            .ok()
            .flatten()
    }

    /// The provider's page with this session's visuals placed by time.
    async fn with_visuals(&self, session_key: &str, mut page: HistoryPage) -> HistoryPage {
        let Some(journal) = &self.inner.journal else {
            return page;
        };
        let root = journal.root();
        let key = session_key.to_owned();
        let since = (page.older_messages > 0)
            .then(|| {
                page.events
                    .iter()
                    .find_map(|event| event.at.as_deref().and_then(iso_ms))
            })
            .flatten();
        let visuals =
            tokio::task::spawn_blocking(move || thread_journal::visual_events(&root, &key, since))
                .await
                .unwrap_or_default();
        if !visuals.is_empty() {
            thread_journal::merge_visuals(&mut page, visuals);
        }
        page
    }

    /// A switched thread's head reads as one conversation: every segment's
    /// journal merged by time, "Switched to <provider>" at each change.
    async fn chain_history(&self, session_key: &str, limit: u64) -> Option<HistoryPage> {
        let root = self.inner.journal.as_ref()?.root();
        let parts = self.chain_history_parts(session_key).await?;
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        tokio::task::spawn_blocking(move || {
            let labels = parts
                .iter()
                .map(|part| {
                    interface_contracts::ipc::parse_key(&part.key)
                        .map(|(engine, _)| {
                            interface_collaboration::thread_handoff::provider_word(engine.as_str())
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            let segments = parts
                .iter()
                .zip(&labels)
                .map(|(part, label)| thread_journal::ChainSegment {
                    key: &part.key,
                    label,
                })
                .collect::<Vec<_>>();
            thread_journal::replay_chain_history(&root, &segments, limit)
        })
        .await
        .ok()
        .flatten()
    }
}

/// The next ops, as many as are ready (at most [`BATCH`]), or none when
/// `wait` passes first; and whether the channel closed.
async fn next_batch(
    ops: &mut mpsc::UnboundedReceiver<JournalOp>,
    wait: Option<Duration>,
) -> (Vec<JournalOp>, bool) {
    tokio::select! {
        biased;
        op = ops.recv() => match op {
            Some(op) => {
                let mut batch = vec![op];
                while batch.len() < BATCH {
                    match ops.try_recv() {
                        Ok(op) => batch.push(op),
                        Err(_) => break,
                    }
                }
                (batch, false)
            }
            None => (Vec::new(), true),
        },
        () = sleep(wait) => (Vec::new(), false),
    }
}

async fn sleep(wait: Option<Duration>) {
    match wait {
        Some(wait) => tokio::time::sleep(wait).await,
        None => std::future::pending().await,
    }
}

/// With no writer, ops are read and dropped; a drain is still answered.
async fn discard(mut ops: mpsc::UnboundedReceiver<JournalOp>) {
    while let Some(op) = ops.recv().await {
        if let JournalOp::Drain(done) = op {
            let _ = done.send(());
            return;
        }
    }
}

/// Whether a session's entries go straight to its journal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    /// Not known to have a thread record yet: entries are held.
    #[default]
    Unknown,
    /// It has a record (a prompt or a journal says so).
    Journalled,
    /// The store had no record for it: nothing is kept until its next run.
    Ignored,
}

#[derive(Debug, Default)]
struct KeyState {
    fold: JournalFold,
    mode: Mode,
    held: Vec<(u64, JournalEntry)>,
    held_since: Option<u64>,
}

/// What one batch asks of the async side.
#[derive(Default)]
pub(crate) struct BatchOutcome {
    /// Sessions whose held entries wait on whether they have a record.
    pub(crate) undecided: Vec<String>,
    pub(crate) drain: Option<oneshot::Sender<()>>,
    /// The channel closed.
    pub(crate) closed: bool,
}

/// The worker's state and rules, without the async parts: tests drive it
/// with their own clock.
pub(crate) struct JournalCore {
    pub(crate) journal: ThreadJournal,
    keys: HashMap<String, KeyState>,
    unsynced_since: Option<u64>,
    /// The visual store, opened at the first visual or removal; why it did
    /// not open, once it failed (tried again next launch).
    visuals: Option<Result<ThreadVisuals, String>>,
}

impl JournalCore {
    pub(crate) fn new(journal: ThreadJournal) -> Self {
        Self {
            journal,
            keys: HashMap::new(),
            unsynced_since: None,
            visuals: None,
        }
    }

    /// The visual store beside the journal, in the same data root.
    fn visuals(&mut self) -> Result<&mut ThreadVisuals, String> {
        if self.visuals.is_none() {
            let data_root = self
                .journal
                .root()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            self.visuals = Some(ThreadVisuals::open(&data_root).map_err(|error| {
                tracing::error!(%error, "the thread visual store did not open; no visual is kept this launch");
                error.to_string()
            }));
        }
        match self.visuals.as_mut() {
            Some(Ok(visuals)) => Ok(visuals),
            Some(Err(reason)) => Err(format!("The visual store could not be opened ({reason}).")),
            None => Err("The visual store could not be opened.".into()),
        }
    }

    /// Writes a visual's page under `key` (the rules are checked again).
    fn keep_visual(&mut self, key: &str, id: &str, html: &str) -> Result<SavedVisual, String> {
        if let Err(refusal) = thread_visuals::check_visual("page", html) {
            return Err(refusal.message());
        }
        self.visuals()?
            .save(key, id, html)
            .map_err(|error| format!("The visual could not be saved ({error})."))
    }

    /// Applies `batch` in order (stopping at a drain), then the holds and
    /// fsyncs that are due at `now`.
    pub(crate) fn apply_batch(&mut self, batch: Vec<JournalOp>, now: u64) -> BatchOutcome {
        let mut outcome = BatchOutcome::default();
        for op in batch {
            if let JournalOp::Drain(done) = op {
                outcome.drain = Some(done);
                break;
            }
            outcome.undecided.extend(self.apply(op, now));
        }
        outcome.undecided.extend(self.tick(now));
        self.sync_if_due(now);
        outcome.undecided.sort();
        outcome.undecided.dedup();
        outcome
    }

    /// Applies one op; answers the sessions now waiting on a record check.
    pub(crate) fn apply(&mut self, op: JournalOp, now: u64) -> Vec<String> {
        match op {
            JournalOp::Event {
                key,
                at,
                event,
                run,
            } => self.event(&key, at, &event, run),
            JournalOp::Prompt {
                key,
                at,
                origin,
                prompt,
            } => {
                self.prompt(&key, at, origin, prompt);
                Vec::new()
            }
            JournalOp::Retract { key, at } => {
                self.retract(&key, at);
                Vec::new()
            }
            JournalOp::Visual {
                key,
                at,
                id,
                title,
                html,
                reply,
            } => match self.keep_visual(&key, &id, &html) {
                Ok(saved) => {
                    let bytes = saved.bytes;
                    let undecided = self.event(
                        &key,
                        at,
                        &EngineEventKind::VisualShown { id, title, bytes },
                        None,
                    );
                    // Answered once its entry is written (or held, for a
                    // session the journal has no prompt for yet).
                    let _ = reply.send(Ok(saved));
                    undecided
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                    Vec::new()
                }
            },
            JournalOp::Removed { key } => {
                self.keys.remove(&key);
                if let Err(error) = self.journal.remove(&key) {
                    tracing::warn!(%error, "a removed session's journal could not be deleted");
                }
                if let Ok(visuals) = self.visuals()
                    && let Err(error) = visuals.remove_thread(&key)
                {
                    tracing::warn!(%error, "a removed session's visuals could not be deleted");
                }
                Vec::new()
            }
            JournalOp::Import { key, events, older } => {
                self.import(&key, &events, older, now);
                Vec::new()
            }
            JournalOp::Rebuild { key, events, older } => {
                self.rebuild(&key, &events, older, now);
                Vec::new()
            }
            JournalOp::Drain(done) => {
                let _ = done.send(());
                Vec::new()
            }
        }
    }

    fn state(&mut self, key: &str) -> &mut KeyState {
        let journalled = self.journal.has_journal(key);
        self.keys.entry(key.to_owned()).or_insert_with(|| KeyState {
            mode: if journalled {
                Mode::Journalled
            } else {
                Mode::Unknown
            },
            ..KeyState::default()
        })
    }

    fn event(
        &mut self,
        key: &str,
        at: u64,
        event: &EngineEventKind,
        run: Option<RunChange>,
    ) -> Vec<String> {
        let state = self.state(key);
        if state.mode == Mode::Ignored {
            if run != Some(RunChange::Started) {
                return Vec::new();
            }
            // A new run: look again once it has something to say.
            state.mode = Mode::Unknown;
        }
        let closed = matches!(event, EngineEventKind::SessionClosed);
        let end = match run {
            Some(RunChange::Ended(outcome)) => Some(outcome),
            _ => None,
        };
        let mut entries = if closed {
            state.fold.flush_all()
        } else {
            state.fold.event(event)
        };
        if let Some(outcome) = end {
            entries.extend(state.fold.flush_all());
            entries.push(JournalEntry::TurnEnded {
                outcome,
                reason: None,
            });
        }
        if closed {
            entries.push(JournalEntry::SessionClosed {});
        }
        let mut undecided = self.write(key, at, entries);
        if end.is_some() {
            self.sync(key);
        }
        if closed
            && self
                .keys
                .get(key)
                .is_some_and(|state| !state.held.is_empty())
        {
            undecided.extend(self.release(key));
        }
        undecided
    }

    /// Writes `entries`, or holds them while `key` has no prompt yet.
    fn write(&mut self, key: &str, at: u64, entries: Vec<JournalEntry>) -> Vec<String> {
        if entries.is_empty() {
            return Vec::new();
        }
        let holding = {
            let has_turn = self.journal.has_turn(key);
            let state = self.state(key);
            state.mode != Mode::Journalled || !has_turn
        };
        if holding {
            let state = self.state(key);
            state.held_since.get_or_insert(at);
            state
                .held
                .extend(entries.into_iter().map(|entry| (at, entry)));
            if state.held.len() >= HOLD_CAP {
                return self.release(key);
            }
            return Vec::new();
        }
        for entry in entries {
            self.append(key, at, entry);
        }
        Vec::new()
    }

    fn append(&mut self, key: &str, at: u64, entry: JournalEntry) {
        // A turn's end with no open turn (a run the provider started on its
        // own after the last one ended) adds nothing.
        if matches!(entry, JournalEntry::TurnEnded { .. }) && !self.journal.turn_open(key) {
            return;
        }
        match self.journal.append(key, at, entry) {
            Ok(_) => {
                self.unsynced_since.get_or_insert(at);
            }
            Err(error) => tracing::warn!(%error, "a thread journal entry was not written"),
        }
    }

    /// The held entries of `key` past their hold: written when it is known
    /// to have a record, else asked about.
    fn release(&mut self, key: &str) -> Vec<String> {
        if self
            .keys
            .get(key)
            .is_some_and(|state| state.mode == Mode::Journalled)
        {
            self.flush_held(key);
            Vec::new()
        } else {
            vec![key.to_owned()]
        }
    }

    fn flush_held(&mut self, key: &str) {
        let held = self.keys.get_mut(key).map_or_else(Vec::new, |state| {
            state.held_since = None;
            std::mem::take(&mut state.held)
        });
        for (at, entry) in held {
            self.append(key, at, entry);
        }
    }

    /// The store said whether `key` has a thread record.
    pub(crate) fn decide(&mut self, key: &str, recorded: bool, _now: u64) {
        let Some(state) = self.keys.get_mut(key) else {
            return;
        };
        if recorded {
            state.mode = Mode::Journalled;
            self.flush_held(key);
        } else if state.mode != Mode::Journalled {
            state.mode = Mode::Ignored;
            state.held.clear();
            state.held_since = None;
            state.fold = JournalFold::default();
        }
    }

    fn prompt(&mut self, key: &str, at: u64, origin: TurnOrigin, prompt: String) {
        let launch = origin == TurnOrigin::Launch;
        if launch && self.journal.has_turn(key) {
            // A bind again keeps the turns it has.
            self.state(key).mode = Mode::Journalled;
            return;
        }
        let state = self.state(key);
        state.mode = Mode::Journalled;
        // A launch prompt written before what it started takes its time too,
        // so `at` never runs backwards.
        let at = if launch {
            state.held_since.map_or(at, |since| since.min(at))
        } else {
            at
        };
        let earlier = if launch {
            Vec::new()
        } else {
            // What the agent said before this prompt belongs before it.
            state.fold.flush_all()
        };
        if !launch {
            self.flush_held(key);
            for entry in earlier {
                self.append(key, at, entry);
            }
        }
        let digest = prompt_digest(&prompt);
        self.append(
            key,
            at,
            JournalEntry::TurnStarted {
                origin,
                prompt,
                digest,
                cut: false,
            },
        );
        if launch {
            // The launch prompt started what was held.
            self.flush_held(key);
        }
    }

    fn retract(&mut self, key: &str, at: u64) {
        if !self.journal.turn_open(key) {
            return;
        }
        let earlier = self.state(key).fold.flush_all();
        for entry in earlier {
            self.append(key, at, entry);
        }
        self.append(key, at, JournalEntry::TurnRetracted {});
        self.sync(key);
    }

    fn import(&mut self, key: &str, events: &[EngineEvent], older: bool, now: u64) {
        // A live session got there first.
        if self.journal.has_journal(key) || self.live(key) {
            return;
        }
        let entries = import_entries_from(events, now, older);
        if entries.is_empty() {
            if let Err(error) = self.journal.begin(key) {
                tracing::warn!(%error, "an imported thread's empty journal was not started");
            }
        } else {
            for (at, entry) in entries {
                if let Err(error) = self.journal.append(key, at, entry) {
                    tracing::warn!(%error, "an imported thread journal entry was not written");
                }
            }
            self.sync(key);
        }
        // Read from the whole history: never asked again.
        if let Err(error) = self.journal.mark_whole_import(key) {
            tracing::warn!(%error, "an imported thread journal was not marked whole");
        }
    }

    /// Whether `key` has had live entries this launch (written or held).
    fn live(&self, key: &str) -> bool {
        self.keys
            .get(key)
            .is_some_and(|state| !state.held.is_empty() || state.mode == Mode::Journalled)
    }

    /// Replaces a journal that holds only an older, shorter import with the
    /// provider's whole history, when that history holds more lines and
    /// every prompt the journal kept, in order; otherwise marks it checked.
    /// A journal written live since, or one that changed meanwhile, is left
    /// alone; a failed replacement leaves the old journal and is tried again
    /// next launch.
    fn rebuild(&mut self, key: &str, events: &[EngineEvent], older: bool, now: u64) {
        if !self.journal.imported_only(key) || self.live(key) {
            return;
        }
        let entries = import_entries_from(events, now, older);
        let old = thread_journal::read::lines(self.journal.root(), key);
        if !keeps_everything(&old, &entries) {
            if let Err(error) = self.journal.mark_whole_import(key) {
                tracing::warn!(%error, "a checked thread journal was not marked");
            }
            return;
        }
        match self.journal.replace_with_import(key, entries) {
            Ok(()) => {
                tracing::info!(
                    "a thread journal imported short was rebuilt from its whole history"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "a short thread journal was not rebuilt; it is kept as it was");
            }
        }
    }

    /// Releases the holds that ran out by `now`.
    pub(crate) fn tick(&mut self, now: u64) -> Vec<String> {
        let mut due = self
            .keys
            .iter()
            .filter(|(_, state)| {
                state
                    .held_since
                    .is_some_and(|since| now.saturating_sub(since) >= HOLD_MS)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        due.sort();
        due.into_iter().flat_map(|key| self.release(&key)).collect()
    }

    /// Every session with entries held.
    pub(crate) fn held_keys(&self) -> Vec<String> {
        let mut keys = self
            .keys
            .iter()
            .filter(|(_, state)| !state.held.is_empty())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    fn sync(&mut self, key: &str) {
        if let Err(error) = self.journal.sync(key) {
            tracing::warn!(%error, "a thread journal was not synced");
        }
    }

    fn sync_if_due(&mut self, now: u64) {
        if self
            .unsynced_since
            .is_some_and(|since| now.saturating_sub(since) >= SYNC_MS)
        {
            self.sync_all();
        }
    }

    pub(crate) fn sync_all(&mut self) {
        if let Err(error) = self.journal.sync_all() {
            tracing::warn!(%error, "the thread journal was not synced");
        }
        self.unsynced_since = None;
    }

    /// When the worker must next wake: a hold running out or an fsync due.
    pub(crate) fn next_deadline(&self) -> Option<u64> {
        let hold = self
            .keys
            .values()
            .filter_map(|state| state.held_since)
            .min()
            .map(|since| since + HOLD_MS);
        let sync = self.unsynced_since.map(|since| since + SYNC_MS);
        hold.into_iter().chain(sync).min()
    }
}

#[cfg(test)]
#[path = "journal_tests.rs"]
mod tests;

/// Whether `new` (a whole-history import) holds more lines than `old` (the
/// journal's lines) and every prompt `old` holds, in the same order.
fn keeps_everything(
    old: &[interface_contracts::thread_journal::JournalLine],
    new: &[(u64, JournalEntry)],
) -> bool {
    let prompt = |entry: &JournalEntry| match entry {
        JournalEntry::TurnStarted { prompt, .. } => Some(prompt.clone()),
        _ => None,
    };
    let mut fresh = new.iter().filter_map(|(_, entry)| prompt(entry));
    new.len() > old.len()
        && old
            .iter()
            .filter_map(|line| prompt(&line.entry))
            .all(|kept| fresh.any(|candidate| candidate == kept))
}

/// An event time in milliseconds since the epoch.
fn iso_ms(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
}
