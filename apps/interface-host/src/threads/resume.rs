//! Resume at reset (T3 section 10, protocol 66).
//!
//! An adapter. The rules (when a resume is due, which prompt it may carry,
//! the reset from usage windows, one resume per thread) live in
//! `interface_settings::thread_schedules`. This module keeps, per session,
//! "its provider hit its usage limit, which resets at T" as the run tap sees
//! it (`ThreadView.limit`), fills a missing reset from the provider's usage
//! windows (the row of the thread's own account), and turns the owner's
//! Resume at reset into the thread's one `resume` schedule. That schedule
//! then goes out exactly as any scheduled prompt does (`schedules.rs`): the
//! same gate, occurrence id and one attempt; an unknown outcome is never
//! tried again. A resume that carries a limit-refused prompt consumes its
//! prompt-recovery receipt as a Retry with its one send, so that prompt
//! never goes twice.

use std::{collections::HashMap, sync::Arc};

use interface_common::sync::lock;
use interface_contracts::{
    engine::{EngineEventKind, EngineUsage, RateLimitNotice},
    ipc::{CHANNELS, parse_key},
    recovery::PromptRecoveryReceipt,
    threads::{
        RunOutcome, ScheduleKind, ScheduleSetBy, ScheduleWhen, ThreadControlResponse, ThreadLimit,
        ThreadsView,
    },
};
use interface_settings::thread_schedules::{self, NewSchedule};
use serde_json::json;

use super::{ThreadHost, run_tap::RunChange};
use crate::dispatcher::{DispatchError, HostDispatcher};

/// Sessions whose limit is remembered at most; the oldest hit goes first.
const MAX_LIMITS: usize = 512;

/// The latest prompt-recovery receipt of a session (the desktop service's
/// store); the resume reads it to find a prompt the limit refused.
pub trait PromptReceipts: Send + Sync {
    fn latest(&self, key: &str) -> Option<PromptRecoveryReceipt>;
}

impl PromptReceipts for interface_desktop_services::DesktopService {
    fn latest(&self, key: &str) -> Option<PromptRecoveryReceipt> {
        self.prompt_recovery_receipt(key)
    }
}

/// One session's limit, as the run tap last saw it.
#[derive(Clone, Debug)]
pub(crate) struct LimitHit {
    /// Which hit this is, so a usage read finishing late fills only its own.
    seq: u64,
    resets_at: Option<String>,
}

/// Limits by session key, and the optional ports the resume reads.
#[derive(Default)]
pub(crate) struct ResumeParts {
    limits: std::sync::Mutex<HashMap<String, LimitHit>>,
    seq: std::sync::atomic::AtomicU64,
    /// Answers `readUsage` (the session management owner).
    usage: std::sync::OnceLock<Arc<dyn HostDispatcher>>,
    receipts: std::sync::OnceLock<Arc<dyn PromptReceipts>>,
}

/// What one event did to a session's limit.
enum LimitChange {
    /// A new or changed limit; `fallback` when its reset is unknown.
    Hit {
        seq: u64,
        fallback: bool,
    },
    Cleared,
}

impl ThreadHost {
    /// Attaches the usage reader the reset fallback asks. Attaching twice
    /// keeps the first.
    pub fn attach_usage(&self, usage: Arc<dyn HostDispatcher>) {
        let _ = self.inner.resume.usage.set(usage);
    }

    /// Attaches the prompt-recovery receipts a resume may reuse.
    pub fn attach_prompt_receipts(&self, receipts: Arc<dyn PromptReceipts>) {
        let _ = self.inner.resume.receipts.set(receipts);
    }

    /// The run tap's hook: a rate-limit notice records the session's limit
    /// (its reset when the notice says, else from its usage windows); a run
    /// that completes clears it. Inline on every event: no allocation unless
    /// the limit changes.
    pub(crate) fn observe_limit(
        &self,
        key: &str,
        event: &EngineEventKind,
        change: Option<RunChange>,
    ) {
        let changed = match (event, change) {
            (
                EngineEventKind::UsageUpdated {
                    rate_limited: Some(notice),
                    ..
                },
                _,
            ) => self.limit_hit(key, notice),
            (_, Some(RunChange::Ended(RunOutcome::Completed))) => {
                let removed = lock(&self.inner.resume.limits).remove(key).is_some();
                removed.then_some(LimitChange::Cleared)
            }
            _ => None,
        };
        let Some(changed) = changed else {
            return;
        };
        let host = self.clone();
        let owned = key.to_owned();
        self.spawn_tracked(async move {
            if let LimitChange::Hit {
                seq,
                fallback: true,
            } = changed
            {
                host.limit_fallback(&owned, seq).await;
            }
            host.pushed_quietly().await;
        });
    }

    fn limit_hit(&self, key: &str, notice: &RateLimitNotice) -> Option<LimitChange> {
        let resets_at = notice
            .resets_at
            .clone()
            .filter(|text| thread_schedules::iso_millis(text).is_some());
        let mut limits = lock(&self.inner.resume.limits);
        if let Some(hit) = limits.get(key)
            && (hit.resets_at == resets_at || resets_at.is_none())
        {
            // The same limit again; a notice without a reset keeps the one known.
            return None;
        }
        if limits.len() >= MAX_LIMITS && !limits.contains_key(key) {
            let oldest = limits
                .iter()
                .min_by_key(|(_, hit)| hit.seq)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                limits.remove(&oldest);
            }
        }
        let seq = self
            .inner
            .resume
            .seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        let fallback = resets_at.is_none();
        limits.insert(key.to_owned(), LimitHit { seq, resets_at });
        Some(LimitChange::Hit { seq, fallback })
    }

    /// The limit `key`'s notice left without a reset: the provider's usage
    /// windows, the row of the thread's own account, say when a full one
    /// resets. Fills only the same hit, and only when still unknown.
    async fn limit_fallback(&self, key: &str, seq: u64) {
        let Some(usage) = self.inner.resume.usage.get().cloned() else {
            return;
        };
        let Ok((engine, _)) = parse_key(key) else {
            return;
        };
        let account = self
            .accounts()
            .and_then(|accounts| accounts.account_of_key(key));
        let rows = match usage.dispatch(CHANNELS.read_usage, json!({})).await {
            Ok(value) => serde_json::from_value::<Vec<EngineUsage>>(value).unwrap_or_default(),
            Err(error) => {
                tracing::info!(code = %error.code, "a limit's reset could not be read from usage");
                return;
            }
        };
        let now = self.now();
        let Some(resets_at) = rows
            .iter()
            .find(|row| row.engine == engine && row.account == account)
            .and_then(|row| thread_schedules::usage_windows_reset(&row.windows, now))
        else {
            return;
        };
        if let Some(hit) = lock(&self.inner.resume.limits)
            .get_mut(key)
            .filter(|hit| hit.seq == seq && hit.resets_at.is_none())
        {
            hit.resets_at = Some(resets_at);
        }
    }

    /// Puts each thread's limit on `view`.
    pub(super) fn with_limits(&self, view: &mut ThreadsView) {
        let limits = lock(&self.inner.resume.limits);
        if limits.is_empty() {
            return;
        }
        for thread in &mut view.threads {
            thread.limit = limits.get(&thread.key).map(|hit| ThreadLimit {
                resets_at: hit.resets_at.clone(),
            });
        }
    }

    /// `resumeAtReset`: thread `key` carries on by itself a minute after its
    /// limit resets, with the owner's queued `prompt`, else a prompt the
    /// limit refused before it reached the provider, else "Continue from
    /// where you stopped." Replaces the thread's earlier resume.
    pub(super) async fn resume_at_reset(
        &self,
        key: String,
        prompt: Option<String>,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let head = self.listed_thread(&key).await?;
        let reset = lock(&self.inner.resume.limits)
            .get(&head)
            .and_then(|hit| hit.resets_at.as_deref())
            .and_then(thread_schedules::iso_millis)
            .ok_or_else(|| {
                DispatchError::new(
                    thread_schedules::LIMIT_UNKNOWN_CODE,
                    "Interface does not know when this thread's usage limit resets, so it cannot resume it then. Send again once it has reset.",
                    false,
                    true,
                )
            })?;
        let now = self.now();
        let at = thread_schedules::resume_at(reset, now);
        let when = ScheduleWhen::Once { at };
        if let Some(prompt) = &prompt {
            thread_schedules::validate(prompt, None, &when, now).map_err(|refusal| {
                DispatchError::new(refusal.code, refusal.message, false, true)
            })?;
        }
        let receipts = self.inner.resume.receipts.get().cloned();
        let target = head.clone();
        let receipt = tokio::task::spawn_blocking(move || {
            receipts.and_then(|receipts| receipts.latest(&target))
        })
        .await
        .ok()
        .flatten();
        let (text, receipt) =
            thread_schedules::resume_prompt_choice(prompt.as_deref(), receipt.as_ref());
        let added = NewSchedule {
            id: super::schedules::new_id(),
            thread_key: head,
            name: None,
            prompt: text,
            when,
            set_by: ScheduleSetBy::Owner,
            kind: ScheduleKind::Resume,
            receipt,
        };
        self.schedules_write(move |store| store.set_resume(added, at, now))
            .await?;
        Ok(self.push_schedules().await?.into())
    }
}
