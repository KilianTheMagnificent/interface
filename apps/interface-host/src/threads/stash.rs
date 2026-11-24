//! `iface:draftStash` (T3 section 9, protocol 63): a thread's stashed
//! drafts. The store and its bounds are `interface_settings::draft_stash`;
//! this owner resolves the thread (a segment's key means its head), refuses a
//! session that is not a thread, and runs each commit on the blocking pool.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use interface_common::sync::lock;
use interface_contracts::draft_stash::{DraftStashCommand, DraftStashView};
use interface_settings::SettingsError;
use interface_settings::draft_stash::{DRAFT_STASH_FILE_NAME, DraftStashStore};
use serde_json::Value;

use super::ThreadHost;
use crate::dispatcher::{DispatchError, INVALID_PAYLOAD_CODE};

/// The store did not open this launch. Definitely unsent.
pub const DRAFT_STASH_UNAVAILABLE_CODE: &str = "draft-stash-unavailable";
/// The store refused the change (a bound, an unknown stash, not a thread);
/// nothing changed.
pub const DRAFT_STASH_REFUSED_CODE: &str = "draft-stash-refused";
/// The write failed and may have reached disk.
pub const DRAFT_STASH_FAILED_CODE: &str = "draft-stash-failed";

/// The store as the stash owner and the prompt-context owner share it.
pub(crate) type SharedStash = Arc<Mutex<Result<DraftStashStore, String>>>;

fn refused(message: impl Into<String>) -> DispatchError {
    DispatchError::new(DRAFT_STASH_REFUSED_CODE, message, false, true)
}

fn store_error(error: SettingsError) -> DispatchError {
    match error {
        // The store's own refusals: nothing was written.
        SettingsError::InvalidAgent(message) => refused(message),
        SettingsError::Storage(interface_storage::StorageError::WriteFailed { .. }) => {
            DispatchError::new(
                DRAFT_STASH_FAILED_CODE,
                "The stashed drafts could not be saved; nothing changed.",
                true,
                true,
            )
        }
        other => {
            tracing::warn!(error = %other, "a stashed drafts write may not have reached disk");
            DispatchError::new(
                DRAFT_STASH_FAILED_CODE,
                "The stashed drafts may not have been saved. Restart Interface and check the list before trying again.",
                false,
                false,
            )
        }
    }
}

impl ThreadHost {
    /// Opens `draft-stash.v1.json` in the private data root. A file that
    /// does not decode is quarantined and the store starts empty.
    ///
    /// # Errors
    /// Why the store could not open (every stash command then refuses).
    pub fn open_stash_store(data_root: &Path) -> Result<DraftStashStore, String> {
        DraftStashStore::open(&data_root.join(DRAFT_STASH_FILE_NAME)).map_err(|error| {
            tracing::error!(%error, "the stashed drafts store did not open; drafts cannot be stashed");
            error.to_string()
        })
    }

    /// Attaches the stash store. Attaching twice keeps the first.
    pub fn attach_stash(&self, store: Result<DraftStashStore, String>) {
        let _ = self.inner.stash.set(Arc::new(Mutex::new(store)));
    }

    fn stash(&self) -> Result<SharedStash, DispatchError> {
        self.inner.stash.get().cloned().ok_or_else(|| {
            DispatchError::new(
                DRAFT_STASH_UNAVAILABLE_CODE,
                "Stashing drafts is not available in this Interface.",
                false,
                true,
            )
        })
    }

    async fn stash_store<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut DraftStashStore) -> Result<T, SettingsError> + Send + 'static,
    ) -> Result<T, DispatchError> {
        let store = self.stash()?;
        tokio::task::spawn_blocking(move || {
            let mut store = lock(&store);
            match store.as_mut() {
                Ok(store) => work(store).map_err(store_error),
                Err(reason) => Err(DispatchError::new(
                    DRAFT_STASH_UNAVAILABLE_CODE,
                    format!("The stashed drafts could not be opened ({reason}). Restart Interface to try again."),
                    false,
                    true,
                )),
            }
        })
        .await
        .map_err(|_| {
            DispatchError::new(
                DRAFT_STASH_FAILED_CODE,
                "The stashed drafts stopped unexpectedly. Check the list before trying again.",
                false,
                false,
            )
        })?
    }

    /// One `iface:draftStash` command.
    pub(super) async fn stash_command(&self, payload: Value) -> Result<Value, DispatchError> {
        let command = crate::payload::decode_object::<DraftStashCommand, _>(payload, |error| {
            if let Some(error) = error {
                tracing::warn!(%error, "rejected an invalid draft stash command");
            }
            DispatchError::new(
                INVALID_PAYLOAD_CODE,
                "The draft stash command does not match its v1 schema.",
                false,
                true,
            )
        })?;
        command.validate().map_err(refused)?;
        // A switched thread is one thread: an earlier segment reaches its
        // head, and only a thread keeps stashes.
        let key = self.head_key(command.key()).await;
        let owned = key.clone();
        let known = self
            .peek(move |document, _| document.record(&owned).is_some())
            .await?;
        if !known {
            return Err(refused("Only a Threads thread can stash drafts."));
        }
        let now = self.now();
        let view: DraftStashView = match command {
            DraftStashCommand::List { .. } => {
                let key = key.clone();
                self.stash_store(move |store| Ok(store.view(&key))).await?
            }
            DraftStashCommand::Push { draft, .. } => {
                let key = key.clone();
                self.stash_store(move |store| {
                    store.push(&key, draft, now)?;
                    Ok(store.view(&key))
                })
                .await?
            }
            DraftStashCommand::Pop { id, current, .. } => {
                let key = key.clone();
                self.stash_store(move |store| {
                    let popped = store.pop(&key, &id, current, now)?;
                    Ok(DraftStashView {
                        popped: Some(popped),
                        ..store.view(&key)
                    })
                })
                .await?
            }
            DraftStashCommand::Delete { id, .. } => {
                let key = key.clone();
                self.stash_store(move |store| {
                    store.delete(&key, &id)?;
                    Ok(store.view(&key))
                })
                .await?
            }
        };
        serde_json::to_value(view).map_err(|error| {
            DispatchError::new(
                "draft-stash-encode-failed",
                format!("The stashed drafts could not be encoded: {error}"),
                false,
                false,
            )
        })
    }

    /// A deleted or forgotten thread's stashed drafts go with it.
    pub(crate) fn stash_removed(&self, key: &str) {
        let Some(store) = self.inner.stash.get().cloned() else {
            return;
        };
        let owned = key.to_owned();
        self.spawn_tracked(async move {
            let forgot = tokio::task::spawn_blocking(move || {
                lock(&store)
                    .as_mut()
                    .map_err(|_| ())
                    .and_then(|store| store.forget(&owned).map_err(|_| ()))
            })
            .await;
            if !matches!(forgot, Ok(Ok(()))) {
                tracing::warn!("a removed thread's stashed drafts could not be forgotten");
            }
        });
    }

    /// A switch moved the thread from `from` to `to`: its stashed drafts
    /// follow the head. A failed write is logged; the switch stands.
    pub(super) async fn stash_follow(&self, from: &str, to: &str) {
        if self.inner.stash.get().is_none() {
            return;
        }
        let (from, to) = (from.to_owned(), to.to_owned());
        if self
            .stash_store(move |store| store.follow(&from, &to))
            .await
            .is_err()
        {
            tracing::warn!("a switched thread's stashed drafts could not follow it");
        }
    }

    /// File paths stashed drafts refer to (they keep pasted images).
    pub(crate) fn stash_retained_paths(&self) -> Vec<PathBuf> {
        self.inner
            .stash
            .get()
            .and_then(|store| {
                lock(store)
                    .as_ref()
                    .ok()
                    .map(DraftStashStore::retained_paths)
            })
            .unwrap_or_default()
    }
}
