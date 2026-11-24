//! Side questions (protocol 67, T3 section 12): ask a thread something
//! without adding to it. Rust forks the thread at its latest turn (the fork
//! keeps every recorded turn) into a side thread, records it in
//! `thread-sides.v1.json`, hides it from Threads and Chat (it is a hidden
//! session, never a thread record), gives it the read-only rung where its
//! provider has one (Claude's `plan`; Codex keeps the thread's own mode)
//! and no team tools (it resumes one hop down), and sends it the question
//! through the guarded runtime. The renderer draws the side thread's events
//! in the right column. Closing it (after Bring back or not) removes the
//! side thread; removing the thread removes it too. One per thread.

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};

use interface_common::sync::lock;
use interface_contracts::{
    ipc::{CHANNELS, parse_key},
    threads::{ThreadControlResponse, ThreadsView},
};
use interface_settings::{
    thread_sides::{SideQuestion, THREAD_SIDES_FILE_NAME, ThreadSideStore, question_ok},
    thread_team::{self, Rung},
};
use serde_json::json;

use super::{ThreadHost, internal, refused as thread_refused};
use crate::dispatcher::{DispatchError, runtime_dispatch_error};

/// A side question was refused before anything happened. Definitely unsent.
pub const THREADS_SIDE_REFUSED_CODE: &str = "threads-side-refused";
/// The side questions' store did not open, or team tools (the guarded
/// runtime) are not attached. Definitely unsent.
pub const THREADS_SIDE_UNAVAILABLE_CODE: &str = "threads-side-unavailable";

pub(crate) type SharedSides = Arc<Mutex<Result<ThreadSideStore, String>>>;

fn refused(message: impl Into<String>) -> DispatchError {
    DispatchError::new(THREADS_SIDE_REFUSED_CODE, message, false, true)
}

fn unavailable(message: impl Into<String>) -> DispatchError {
    DispatchError::new(THREADS_SIDE_UNAVAILABLE_CODE, message, false, true)
}

impl ThreadHost {
    /// Opens `thread-sides.v1.json` in the private data root.
    ///
    /// # Errors
    /// Why the store could not open (side questions are then refused).
    pub fn open_side_store(data_root: &Path) -> Result<ThreadSideStore, String> {
        ThreadSideStore::open(&data_root.join(THREAD_SIDES_FILE_NAME)).map_err(|error| {
            tracing::error!(%error, "the side questions store did not open");
            error.to_string()
        })
    }

    /// Attaches the side questions' store. Attaching twice keeps the first.
    pub fn attach_sides(&self, store: Result<ThreadSideStore, String>) {
        if let Ok(store) = &store {
            if store.quarantined_artifact().is_some() {
                tracing::warn!("the side questions store was unreadable and was set aside");
            }
            *lock(&self.inner.side_keys) =
                store.sides().iter().map(|side| side.side.clone()).collect();
        }
        let _ = self.inner.sides.set(Arc::new(Mutex::new(store)));
    }

    /// Whether `key` is a side question's side thread.
    pub(crate) fn is_side_thread(&self, key: &str) -> bool {
        lock(&self.inner.side_keys).contains(key)
    }

    /// Every side thread, for the sessions Threads and Chat leave out.
    pub(super) fn side_keys(&self) -> BTreeSet<String> {
        lock(&self.inner.side_keys).clone()
    }

    async fn side_store<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut ThreadSideStore) -> Result<T, String> + Send + 'static,
    ) -> Result<T, DispatchError> {
        let store =
            Arc::clone(self.inner.sides.get().ok_or_else(|| {
                unavailable("Side questions are not available in this Interface.")
            })?);
        tokio::task::spawn_blocking(move || {
            let mut store = lock(&store);
            match store.as_mut() {
                Ok(store) => work(store),
                Err(reason) => Err(format!(
                    "the side questions store could not be opened ({reason}); restart Interface"
                )),
            }
        })
        .await
        .map_err(|_| internal("The side questions task stopped unexpectedly."))?
        .map_err(refused)
    }

    /// Puts each thread's open side question on `view`.
    pub(super) fn with_sides(&self, view: &mut ThreadsView) {
        let Some(store) = self.inner.sides.get() else {
            return;
        };
        let store = lock(store);
        let Ok(store) = store.as_ref() else {
            return;
        };
        for thread in &mut view.threads {
            thread.side = store.of_parent(&thread.key).map(SideQuestion::view);
        }
    }

    /// `SideAsk`: forks thread `key` (its head) at its latest turn into a
    /// hidden side thread and sends it `question`.
    pub(super) async fn side_ask(
        &self,
        key: String,
        question: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if !question_ok(&question) {
            return Err(refused("Write the side question (at most 16 KiB)."));
        }
        let head = self.head_key(&key).await;
        let (engine, _) = parse_key(&head).map_err(|_| refused("This is not a thread."))?;
        let engine = engine.as_str().to_owned();
        if engine != "claude" && engine != "codex" {
            return Err(refused(
                "Side questions work in Claude and Codex threads; this provider cannot fork a thread.",
            ));
        }
        let runtime = self
            .guarded_runtime()
            .ok_or_else(|| unavailable("Side questions are not available in this Interface."))?;
        let target = head.clone();
        let open = self
            .side_store(move |store| Ok(store.of_parent(&target).is_some()))
            .await?;
        if open {
            return Err(refused(
                "This thread already has a side question open. Close it first.",
            ));
        }
        let target = head.clone();
        let turns = self
            .peek(move |document, _| document.record(&target).map(|record| record.turns.len()))
            .await?
            .ok_or_else(|| thread_refused(interface_settings::threads::ThreadRefusal::unknown()))?;
        let keep_turns = u32::try_from(turns).unwrap_or(u32::MAX);
        if keep_turns == 0 {
            return Err(refused(
                "Ask a side question once the thread has an answer.",
            ));
        }
        let child = {
            // Between turns only: a fork keeps whole turns.
            let _gate = self.hold_send_gate(&head).map_err(|_| side_busy())?;
            if self.busy(&head) {
                return Err(side_busy());
            }
            let boundary = self.fork_boundary(&head, keep_turns).await?;
            self.inner
                .ports
                .sessions
                .fork_at(&head, boundary)
                .await
                .map_err(runtime_dispatch_error)?
        };
        // Hidden before anything else can list it.
        lock(&self.inner.side_keys).insert(child.clone());
        let side = SideQuestion {
            parent: head.clone(),
            side: child.clone(),
            question: question.clone(),
            asked_at: self.now(),
        };
        if let Err(error) = self
            .side_store(move |store| store.open_side(side).map_err(|error| error.to_string()))
            .await
        {
            self.remove_side_thread(&child).await;
            return Err(error);
        }
        let seeded = child.clone();
        self.edit_quiet(move |document, _| {
            document.drop_record(&seeded);
            Ok(())
        })
        .await?;
        // Read-only where the provider has it, else the thread's own mode.
        let mode = thread_team::child_mode(&engine, Rung::Plan)
            .map(|(_, mode)| mode.to_owned())
            .or_else(|| self.access_mode(&head));
        self.remember_side_mode(&child, mode).await;
        self.schedule_registry_push();
        let sent = runtime
            .dispatch(
                CHANNELS.send,
                json!({"sessionKey": child, "text": question}),
            )
            .await;
        match sent {
            Ok(_) => {}
            // Definitely not sent: nothing was asked, so nothing stays.
            Err(error) if error.replay_safe => {
                self.close_side(&head).await;
                return Err(error);
            }
            // Unknown: the side question stays open (its panel shows what
            // arrives) and is never sent again.
            Err(mut error) => {
                self.push_view_now().await;
                error.message = format!(
                    "The side question may or may not have reached the provider ({}). Close it to ask again.",
                    error.message
                );
                return Err(error);
            }
        }
        Ok(ThreadControlResponse {
            key: Some(child),
            ..self.view_response().await?
        })
    }

    async fn remember_side_mode(&self, child: &str, mode: Option<String>) {
        let Some(access) = self.side_access() else {
            return;
        };
        let child = child.to_owned();
        let _ = tokio::task::spawn_blocking(move || access.remember(&child, mode.as_deref())).await;
    }

    /// `SideClose`: closes thread `key`'s side question and removes its side
    /// thread.
    pub(super) async fn side_close(
        &self,
        key: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let head = self.head_key(&key).await;
        self.close_side(&head).await;
        self.view_response().await
    }

    /// Removes `parent`'s side question, if any: the record, then the side
    /// thread's session (closed first) and journal.
    pub(crate) async fn close_side(&self, parent: &str) {
        let owned = parent.to_owned();
        let closed = self
            .side_store(move |store| store.close(&owned).map_err(|error| error.to_string()))
            .await;
        match closed {
            Ok(Some(side)) => self.remove_side_thread(&side.side).await,
            Ok(None) => {}
            Err(error) => tracing::warn!(code = %error.code, "a side question stayed open"),
        }
        self.push_view_now().await;
    }

    async fn remove_side_thread(&self, side: &str) {
        if let Err(error) = self.inner.ports.sessions.remove(side, true).await {
            tracing::warn!(%error, "a side thread's session stayed");
        }
        self.journal_removed(side);
        self.remember_side_mode(side, None).await;
        lock(&self.inner.side_keys).remove(side);
        self.schedule_registry_push();
    }

    /// A removed thread's side question goes with it.
    pub(crate) fn sides_removed(&self, key: &str) {
        if self.is_side_thread(key) {
            return;
        }
        let host = self.clone();
        let key = key.to_owned();
        self.spawn_tracked(async move { host.close_side(&key).await });
    }

    async fn push_view_now(&self) {
        if let Ok(((), view)) = self.read(|_, _| ()).await {
            self.push_view(&view).await;
        }
    }
}

fn side_busy() -> DispatchError {
    refused("The thread is working. Ask the side question when this turn ends.")
}
