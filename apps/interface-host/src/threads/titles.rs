//! Generated titles (design "Titles") and the owner-rename hook. A title is
//! one tool-less Claude Haiku 5.5 call over the thread's first prompt and,
//! once there is one, its first answer (read from the provider transcript
//! the renderer reads, else the launch prompt the store kept), applied
//! through the existing rename operation and recorded as
//! `titleSource: generated`. It comes on demand (`RegenerateTitle`) or,
//! while `generatedTitles` is on, automatically for any thread still named
//! by its prompt's seed: asked for as its first prompt goes out, in
//! parallel with the turn (as T3 Code names a thread), and asked for once
//! more after its first completed run only when that first call failed. An
//! owner's rename records `titleSource: user`, which no automatic title
//! replaces; both hold the session's title gate, so neither lands between
//! the other's check and its record.

use std::sync::Arc;

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::{
    engine::EngineEventKind,
    ipc::{HistoryAvailability, HistoryPage},
    threads::{ThreadControlResponse, TitleSource},
};
use interface_settings::threads::ThreadRefusal;
use tokio::sync::OwnedMutexGuard;

use super::{THREADS_TITLE_FAILED_CODE, THREADS_TITLES_UNAVAILABLE_CODE, ThreadHost, refused};
use crate::{
    dispatcher::{DispatchError, runtime_dispatch_error},
    session_management::{RenameHooks, RenameTicket},
};

/// The most of each excerpt a title reads; the Claude call bounds it again.
const EXCERPT_BYTES: usize = engine_claude::title::TITLE_EXCERPT_BYTES;

/// Where a thread's automatic title stands this launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AutoTitle {
    /// A call is under way. `retry`: it is the last one allowed;
    /// `run_done`: the first run completed meanwhile, so a failed first
    /// call is retried at once.
    Asking { retry: bool, run_done: bool },
    /// The call at the first prompt failed; the first completed run asks
    /// once more.
    Failed,
    /// Named, given way to the thread's own title, or out of attempts.
    Settled,
}

/// How one automatic attempt ended.
enum AttemptEnd {
    /// Applied, or given way to a title that is no longer the seed.
    Done,
    /// Claude gave no title: worth one more attempt.
    NoTitle,
    /// The rename or its record failed, or the host is stopping: never
    /// replayed.
    Stopped,
}

/// Whether a title may replace the one a thread has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TitleRequest {
    /// `RegenerateTitle`: the owner asked, so any title is replaced.
    Explicit,
    /// At the first prompt or after the first completed run: only the
    /// launch prompt's seed.
    Automatic,
}

impl ThreadHost {
    /// `RegenerateTitle`: a new title now, whatever the thread's title came
    /// from. Fails definitely unsent when Claude is not installed or answers
    /// nothing usable.
    pub(super) async fn regenerate_title(
        &self,
        key: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        self.require_record(&key).await?;
        if !self.inner.ports.titles.available() {
            return Err(DispatchError::new(
                THREADS_TITLES_UNAVAILABLE_CODE,
                "Titles are written by Claude, and Claude is not installed. Rename the thread instead.",
                false,
                true,
            ));
        }
        let title = self.generate_title(&key).await?;
        self.apply_title(&key, title, TitleRequest::Explicit)
            .await?;
        Ok(self.read(|_, _| ()).await?.1.into())
    }

    /// The automatic title as `key`'s first prompt goes out, in parallel
    /// with its turn: asked for when the store still wants one then
    /// ([`ThreadsDocument::wants_title_at_first_prompt`]) and no attempt
    /// was made this launch. Claude missing does nothing.
    ///
    /// [`ThreadsDocument::wants_title_at_first_prompt`]: interface_settings::ThreadsDocument::wants_title_at_first_prompt
    pub(crate) fn title_at_first_prompt(&self, key: &str) {
        if !self.inner.ports.titles.available() {
            return;
        }
        let host = self.clone();
        let key = key.to_owned();
        self.spawn_tracked(async move {
            let target = key.clone();
            let wanted = host
                .peek(move |document, _| document.wants_title_at_first_prompt(&target))
                .await
                .unwrap_or(false);
            if !wanted {
                return;
            }
            {
                let mut titled = lock(&host.inner.titled);
                if titled.contains_key(&key) {
                    return;
                }
                titled.insert(
                    key.clone(),
                    AutoTitle::Asking {
                        retry: false,
                        run_done: false,
                    },
                );
            }
            host.automatic_title(key).await;
        });
    }

    /// After `key`'s first completed run: asks once more when the call at
    /// its first prompt failed, or once when none was made this launch
    /// (Claude came later, or the host restarted mid-run). A call still
    /// under way is left to retry by itself if it fails.
    pub(crate) fn title_after_first_run(&self, key: &str) {
        if !self.inner.ports.titles.available() {
            return;
        }
        let start = {
            let mut titled = lock(&self.inner.titled);
            match titled.get(key).copied() {
                None | Some(AutoTitle::Failed) => {
                    titled.insert(
                        key.to_owned(),
                        AutoTitle::Asking {
                            retry: true,
                            run_done: true,
                        },
                    );
                    true
                }
                Some(AutoTitle::Asking { retry, .. }) => {
                    titled.insert(
                        key.to_owned(),
                        AutoTitle::Asking {
                            retry,
                            run_done: true,
                        },
                    );
                    false
                }
                Some(AutoTitle::Settled) => false,
            }
        };
        if start {
            let host = self.clone();
            let key = key.to_owned();
            self.spawn_tracked(async move { host.automatic_title(key).await });
        }
    }

    /// Runs the attempt `key`'s state says is under way, and the retry a
    /// failed first call has earned once the first run completed.
    async fn automatic_title(&self, key: String) {
        loop {
            let end = self.automatic_attempt(&key).await;
            let mut titled = lock(&self.inner.titled);
            let next = match (end, titled.get(&key).copied()) {
                (
                    AttemptEnd::NoTitle,
                    Some(AutoTitle::Asking {
                        retry: false,
                        run_done: true,
                    }),
                ) => AutoTitle::Asking {
                    retry: true,
                    run_done: true,
                },
                (
                    AttemptEnd::NoTitle,
                    Some(AutoTitle::Asking {
                        retry: false,
                        run_done: false,
                    }),
                ) => AutoTitle::Failed,
                _ => AutoTitle::Settled,
            };
            titled.insert(key.clone(), next);
            if !matches!(next, AutoTitle::Asking { .. }) {
                return;
            }
        }
    }

    /// One automatic title: asked for, then applied unless the thread's
    /// title stopped being its seed meanwhile. A failure is logged and the
    /// title stays as it was.
    async fn automatic_attempt(&self, key: &str) -> AttemptEnd {
        let mut stop = self.inner.stop.subscribe();
        if *stop.borrow_and_update() {
            return AttemptEnd::Stopped;
        }
        // Shutdown abandons the Claude call (its process group is killed),
        // never a rename already under way.
        let title = tokio::select! {
            _ = stop.changed() => return AttemptEnd::Stopped,
            title = self.generate_title(key) => title,
        };
        let title = match title {
            Ok(title) => title,
            Err(error) => {
                tracing::warn!(code = %error.code, reason = %error.message, "a thread kept its title");
                return AttemptEnd::NoTitle;
            }
        };
        match self.apply_title(key, title, TitleRequest::Automatic).await {
            Ok(()) => AttemptEnd::Done,
            Err(error) => {
                tracing::warn!(code = %error.code, reason = %error.message, "a thread kept its title");
                AttemptEnd::Stopped
            }
        }
    }

    /// Asks Claude for a title from the thread's first exchange.
    async fn generate_title(&self, key: &str) -> Result<String, DispatchError> {
        let (prompt, answer) = self.title_excerpts(key).await?;
        let titles = Arc::clone(&self.inner.ports.titles);
        titles
            .generate_for(key, prompt, answer)
            .await
            .map_err(|reason| {
                DispatchError::new(
                    THREADS_TITLE_FAILED_CODE,
                    format!(
                        "Claude could not write a title ({reason}). The title was not changed."
                    ),
                    true,
                    true,
                )
            })
    }

    /// The first prompt and its answer from the provider transcript when the
    /// page reaches the start of the conversation; otherwise the launch
    /// prompt the store kept (its first 200 characters), or the session's
    /// title, with no answer.
    async fn title_excerpts(&self, key: &str) -> Result<(String, String), DispatchError> {
        let page = self.inner.ports.sessions.history(key).await;
        if let Ok(page) = page
            && let Some(exchange) = first_exchange(&page)
        {
            return Ok(exchange);
        }
        let target = key.to_owned();
        let stored = self
            .peek(move |document, _| {
                let turn = document.record(&target)?.turns.first()?;
                (turn.ordinal == 0).then(|| turn.preview.clone())
            })
            .await?;
        let prompt = stored
            .filter(|prompt| !prompt.trim().is_empty())
            .or_else(|| {
                self.inner
                    .ports
                    .sessions
                    .session(key)
                    .ok()
                    .flatten()
                    .map(|summary| summary.title)
            })
            .filter(|prompt| !prompt.trim().is_empty())
            .ok_or_else(|| {
                DispatchError::new(
                    THREADS_TITLE_FAILED_CODE,
                    "This thread has no prompt to name it from yet.",
                    false,
                    true,
                )
            })?;
        Ok((prompt, String::new()))
    }

    /// Renames the session to `title` and records it as generated, holding
    /// the title gate so an owner's rename cannot interleave. An automatic
    /// title gives way to any title that is no longer the prompt's seed.
    async fn apply_title(
        &self,
        key: &str,
        title: String,
        request: TitleRequest,
    ) -> Result<(), DispatchError> {
        let _gate = self.title_gate(key).lock_owned().await;
        let target = key.to_owned();
        let source = self
            .peek(move |document, _| document.record(&target).map(|record| record.title_source))
            .await?
            .ok_or_else(|| refused(ThreadRefusal::unknown()))?;
        if request == TitleRequest::Automatic && source != Some(TitleSource::Prompt) {
            tracing::info!(?source, "an automatic title gave way to the thread's own");
            return Ok(());
        }
        self.inner
            .ports
            .sessions
            .rename(key, title)
            .await
            .map_err(runtime_dispatch_error)?;
        self.schedule_registry_push();
        let target = key.to_owned();
        self.edit_quiet(move |document, _| {
            document.set_title_source(&target, TitleSource::Generated);
            Ok(())
        })
        .await
        .map_err(|mut error| {
            error.message = format!(
                "The thread was renamed, but its title could not be recorded as generated: {}",
                error.message
            );
            error.replay_safe = false;
            error
        })?;
        Ok(())
    }

    fn title_gate(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = lock(&self.inner.title_gates);
        gates.retain(|_, gate| Arc::strong_count(gate) > 1);
        Arc::clone(gates.entry(key.to_owned()).or_default())
    }
}

/// The first owner prompt of a transcript page that starts at the beginning
/// of the conversation, and the main-thread answer text after it, each cut
/// to [`EXCERPT_BYTES`].
fn first_exchange(page: &HistoryPage) -> Option<(String, String)> {
    if page.older_messages != 0 || page.availability == Some(HistoryAvailability::Unavailable) {
        return None;
    }
    let mut prompt = None;
    let mut answer = String::new();
    for event in &page.events {
        match &event.event {
            EngineEventKind::UserMessage {
                text,
                agent_key: None,
                ..
            } => {
                if prompt.is_some() {
                    break;
                }
                prompt = Some(clip(text.trim(), EXCERPT_BYTES).to_owned());
            }
            EngineEventKind::MessageDelta {
                text,
                agent_key: None,
            } if prompt.is_some() && answer.len() < EXCERPT_BYTES => {
                answer.push_str(text);
            }
            _ => {}
        }
    }
    let prompt = prompt.filter(|prompt| !prompt.is_empty())?;
    let answer = clip(answer.trim(), EXCERPT_BYTES).to_owned();
    Some((prompt, answer))
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[async_trait]
impl RenameHooks for ThreadHost {
    async fn before_rename(&self, session_key: &str) -> Box<dyn RenameTicket> {
        Box::new(OwnerRename {
            host: self.clone(),
            key: session_key.to_owned(),
            gate: self.title_gate(session_key).lock_owned().await,
        })
    }
}

/// An owner's rename in flight; the session's title gate is held until its
/// outcome is recorded.
struct OwnerRename {
    host: ThreadHost,
    key: String,
    gate: OwnedMutexGuard<()>,
}

impl RenameTicket for OwnerRename {
    fn settle(self: Box<Self>, renamed: bool) {
        if !renamed {
            return;
        }
        let Self { host, key, gate } = *self;
        let task = host.clone();
        host.spawn_tracked(async move {
            if let Err(error) = task
                .edit_quiet(move |document, _| {
                    document.set_title_source(&key, TitleSource::User);
                    Ok(())
                })
                .await
            {
                tracing::warn!(code = %error.code, "an owner's rename was not recorded on its thread");
            }
            drop(gate);
        });
    }
}
