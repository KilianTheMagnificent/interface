//! Session management over the registered providers. This module holds what
//! the parts share: the management state, settle-once slots, registry access
//! and the sanitized error helpers. Each part lives in its own submodule:
//!
//! - `listing` — provider listing refresh and the ingest fences.
//! - `lifecycle` — per-session lifecycle leases and coalesced units.
//! - `operations` — forget, archive, rename, delete, fork, adopt and send-live.
//! - `commands` — provider command discovery for a launch scope.
//! - `transcripts` — stored transcript reads and the bounded search.
//! - `import` — bulk adoption with progress.
//! - `usage` — per-provider plan usage.

mod commands;
mod import;
mod lifecycle;
mod listing;
mod operations;
mod transcripts;
mod usage;

pub use import::{ImportProgress, ImportProgressSink, ImportReport};
pub use operations::RuntimeSendLiveOutcome;

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64},
    },
};

use interface_contracts::{
    engine::{EngineId, SessionSummary},
    ipc::{ProviderListingFreshness, ProviderListingStatus},
};
use interface_engine::{EffectCertainty, EngineAdapter};
use tokio::sync::{Notify, Semaphore};

use crate::{
    RuntimeError, RuntimeErrorKind, SessionRuntime,
    runtime::{blocking, engine_missing, lock, registry_error, remove_if_current},
};

use commands::{CommandDiscoveryEntry, CommandDiscoveryKey, MAX_PROVIDER_COMMAND_DISCOVERIES};
use lifecycle::LifecycleEntry;
use listing::KeptListing;

// The tests import these from this module; nothing here uses them.
#[cfg(test)]
use commands::validate_command_scope_text;
#[cfg(test)]
use transcripts::{SEARCH_MATCH_LIMIT, find_utf16, scan_history_page, search_preview};

const IMPORT_CONCURRENCY: usize = 4;
/// Provider history items the thread journal's one-time import reads: well
/// past what a thread's journal keeps (32 segments of 1 MiB), so only a
/// conversation longer than the journal could hold loses its oldest part.
pub const IMPORT_HISTORY_LIMIT: u64 = 20_000;

pub struct SessionManagementState {
    listing_status: tokio::sync::RwLock<HashMap<EngineId, ProviderListingStatus>>,
    refresh: tokio::sync::Mutex<()>,
    completed_refresh: AtomicU64,
    /// `listing_restarts` as the last completed refresh began, so a refresh
    /// asked after an explicit one never settles for an earlier refresh.
    completed_refresh_restarts: AtomicU64,
    /// The listings a refresh may skip asking for again while they are
    /// younger than their adapter's `listing_reuse`. Failures are never kept.
    kept_listings: Mutex<HashMap<EngineId, KeptListing>>,
    /// Counts explicit refreshes (`probe_all_providers`).
    listing_restarts: AtomicU64,
    listing_generation: AtomicU64,
    deleted_through_listing: Mutex<HashMap<String, u64>>,
    lifecycle: Mutex<HashMap<String, Arc<LifecycleEntry>>>,
    importing: AtomicBool,
    command_limit: Arc<Semaphore>,
    command_engines: HashMap<EngineId, Arc<tokio::sync::Mutex<()>>>,
    command_discoveries: Mutex<HashMap<CommandDiscoveryKey, Arc<CommandDiscoveryEntry>>>,
}

impl SessionManagementState {
    pub(crate) fn new(engines: impl IntoIterator<Item = EngineId>) -> Self {
        let engines = engines.into_iter().collect::<Vec<_>>();
        let listing_status = engines
            .iter()
            .cloned()
            .map(|engine| {
                (
                    engine.clone(),
                    ProviderListingStatus {
                        engine,
                        status: ProviderListingFreshness::Stale,
                        complete: false,
                        refreshed_at: None,
                        error: Some("Not refreshed yet.".into()),
                    },
                )
            })
            .collect();
        let command_engines = engines
            .into_iter()
            .map(|engine| (engine, Arc::new(tokio::sync::Mutex::new(()))))
            .collect();
        Self {
            listing_status: tokio::sync::RwLock::new(listing_status),
            refresh: tokio::sync::Mutex::new(()),
            completed_refresh: AtomicU64::new(0),
            completed_refresh_restarts: AtomicU64::new(0),
            kept_listings: Mutex::new(HashMap::new()),
            listing_restarts: AtomicU64::new(0),
            listing_generation: AtomicU64::new(0),
            deleted_through_listing: Mutex::new(HashMap::new()),
            lifecycle: Mutex::new(HashMap::new()),
            importing: AtomicBool::new(false),
            command_limit: Arc::new(Semaphore::new(MAX_PROVIDER_COMMAND_DISCOVERIES)),
            command_engines,
            command_discoveries: Mutex::new(HashMap::new()),
        }
    }
}

/// One settle-once slot with waiters: the owner settles it exactly once and
/// every waiter observes the same cloned outcome, including a poisoned slot.
struct SettleOnce<T> {
    result: Mutex<Option<T>>,
    settled: Notify,
}

impl<T: Clone> SettleOnce<T> {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            settled: Notify::new(),
        }
    }

    fn complete(&self, result: T) {
        match self.result.lock() {
            Ok(mut current) => *current = Some(result),
            Err(poisoned) => *poisoned.into_inner() = Some(result),
        }
        self.settled.notify_waiters();
    }

    async fn wait(&self) -> T {
        loop {
            let notified = self.settled.notified();
            let current = match self.result.lock() {
                Ok(current) => current.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            };
            if let Some(result) = current {
                return result;
            }
            notified.await;
        }
    }
}

fn remove_entry_if_current<K: Eq + std::hash::Hash, T>(
    entries: &Mutex<HashMap<K, Arc<T>>>,
    key: &K,
    entry: &Arc<T>,
) {
    let mut entries = match entries.lock() {
        Ok(entries) => entries,
        Err(poisoned) => poisoned.into_inner(),
    };
    remove_if_current(&mut entries, key, entry);
}

impl SessionRuntime {
    fn adapter(&self, engine: &EngineId) -> Result<Arc<dyn EngineAdapter>, RuntimeError> {
        self.ensure_accepting()?;
        self.inner
            .adapters
            .get(engine)
            .cloned()
            .ok_or_else(|| engine_missing(engine))
    }

    async fn registry_all(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        let sessions = self.inner.services.session_registry();
        blocking(move || Ok(lock(&sessions, "session registry")?.all())).await
    }

    async fn registry_transaction<T: Send + 'static>(
        &self,
        operation: impl FnOnce(
            &mut interface_registry::SessionRegistry,
        ) -> interface_registry::Result<T>
        + Send
        + 'static,
    ) -> Result<T, RuntimeError> {
        let persistence = self.inner.services.clone();
        let sessions = persistence.session_registry();
        let (value, save) = blocking(move || {
            let mut registry = lock(&sessions, "session registry")?;
            let value = operation(&mut registry).map_err(registry_error)?;
            // An operation that changed nothing (an identical provider listing
            // on every window focus) leaves a current cache as it is.
            Ok((value, !registry.cache_is_current()))
        })
        .await?;
        if save {
            persistence.schedule_registry_cache_save();
        }
        Ok(value)
    }

    async fn registry_after_provider<T: Send + 'static>(
        &self,
        action: &'static str,
        operation: impl FnOnce(
            &mut interface_registry::SessionRegistry,
        ) -> interface_registry::Result<T>
        + Send
        + 'static,
    ) -> Result<T, RuntimeError> {
        self.registry_transaction(operation)
            .await
            .map_err(|error| provider_applied_local_error(action, &error))
    }
}

fn exceeds_javascript_string_limit(value: &str, limit: usize) -> bool {
    value.encode_utf16().nth(limit).is_some()
}

fn optional_cwd(summary: &SessionSummary) -> Option<String> {
    (!summary.cwd.is_empty()).then(|| summary.cwd.clone())
}

fn local_error(
    kind: RuntimeErrorKind,
    message: impl Into<String>,
    retryable: bool,
) -> RuntimeError {
    RuntimeError::definitely_unsent(kind, message, retryable)
}

fn provider_operation_error(action: &str, error: &interface_engine::EngineError) -> RuntimeError {
    let why = public_provider_reason(&error.message);
    let detail = if why.is_empty() {
        String::new()
    } else {
        format!(" ({why})")
    };
    let effect = if error.certainty == EffectCertainty::OutcomeUnknown {
        "The provider outcome is unknown; do not blindly replay this action."
    } else {
        "Nothing was changed."
    };
    RuntimeError::new(
        RuntimeErrorKind::Provider,
        format!("Interface could not {action}{detail}. {effect}"),
        error.retryable,
        error.certainty,
    )
}

fn public_provider_reason(message: &str) -> String {
    const SENSITIVE_MARKERS: [&str; 8] = [
        "api_key",
        "api-key",
        "authorization",
        "bearer ",
        "credential",
        "password",
        "secret",
        "token=",
    ];
    let first = message.lines().next().unwrap_or_default().trim();
    let lowercase = first.to_ascii_lowercase();
    if SENSITIVE_MARKERS
        .iter()
        .any(|marker| lowercase.contains(marker))
    {
        return String::new();
    }
    let mut words = Vec::new();
    for word in first.split_whitespace() {
        let clean = word.trim_matches(|character: char| {
            matches!(character, '\'' | '"' | ',' | ';' | ':' | '(' | ')')
        });
        let errno = clean.len() >= 4
            && clean.starts_with('E')
            && clean[1..].bytes().all(|byte| byte.is_ascii_uppercase());
        let local_path = clean.contains('/') || clean.contains('\\') || clean.contains("://");
        if !errno && !local_path {
            words.push(word);
        }
    }
    while words.last().is_some_and(|word| {
        matches!(
            word.trim_matches(|character: char| !character.is_alphabetic())
                .to_ascii_lowercase()
                .as_str(),
            "at" | "open" | "from"
        )
    }) {
        words.pop();
    }
    let flat = words.join(" ");
    let flat = flat.trim_end_matches([' ', ',', ':', ';', '.']).trim();
    ellipsize(flat, 90)
}

fn ellipsize(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_owned()
    } else {
        let mut bounded = text.chars().take(limit - 1).collect::<String>();
        bounded.truncate(bounded.trim_end().len());
        bounded.push('…');
        bounded
    }
}

fn provider_applied_local_error(action: &str, error: &RuntimeError) -> RuntimeError {
    let retryable = error.retryable;
    RuntimeError::new(
        RuntimeErrorKind::Registry,
        format!(
            "The provider confirmed {action}, but Interface could not apply its local projection. Do not blindly replay the provider effect."
        ),
        retryable,
        EffectCertainty::OutcomeUnknown,
    )
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::{EngineId, ProviderCommandScope};

    use super::{
        SEARCH_MATCH_LIMIT, ellipsize, exceeds_javascript_string_limit, find_utf16,
        public_provider_reason, scan_history_page, search_preview, validate_command_scope_text,
    };

    #[test]
    fn history_page_scan_matches_case_insensitively_and_stops_at_the_limit() {
        let message = |text: &str| interface_contracts::engine::EngineEvent {
            at: None,
            event: interface_contracts::engine::EngineEventKind::UserMessage {
                text: text.into(),
                agent_key: None,
                images: None,
                image_references: None,
            },
        };
        let (matches, truncated) = scan_history_page(
            vec![message("no hit"), message("a TARGET here")],
            "target",
            6,
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].preview, "a TARGET here");
        assert!(!truncated);
        let (matches, truncated) = scan_history_page(
            (0..=SEARCH_MATCH_LIMIT)
                .map(|_| message("target"))
                .collect(),
            "target",
            6,
        );
        assert_eq!(matches.len(), SEARCH_MATCH_LIMIT);
        assert!(truncated);
    }

    #[test]
    fn javascript_string_limits_count_utf16_code_units_at_scope_boundaries() {
        assert!(!exceeds_javascript_string_limit(&"😀".repeat(128), 256));
        assert!(exceeds_javascript_string_limit(&"😀".repeat(129), 256));

        let exact = ProviderCommandScope {
            engine: EngineId::new("claude").unwrap(),
            cwd: "😀".repeat(2_048),
            model: Some("😀".repeat(128)),
            setting_sources: None,
        };
        assert!(validate_command_scope_text(&exact).is_ok());

        let mut oversized_cwd = exact.clone();
        oversized_cwd.cwd = "😀".repeat(2_049);
        assert!(validate_command_scope_text(&oversized_cwd).is_err());

        let mut oversized_model = exact;
        oversized_model.model = Some("😀".repeat(129));
        assert!(validate_command_scope_text(&oversized_model).is_err());
    }

    #[test]
    fn session_search_indices_and_previews_count_utf16_code_units() {
        let text = format!("{}TARGET{}", "😀".repeat(50), "x".repeat(130));
        let lower = text.to_lowercase();
        let at = find_utf16(&lower, "target").expect("literal match");
        assert_eq!(at, 100);
        assert_eq!(
            search_preview(&text, at, "TARGET".encode_utf16().count()),
            format!("…{}TARGET{}…", "😀".repeat(40), "x".repeat(120))
        );
    }

    #[test]
    fn session_search_preview_lossily_projects_a_split_surrogate_boundary() {
        let text = format!("{}aTARGET{}", "😀".repeat(50), "x".repeat(130));
        let lower = text.to_lowercase();
        let at = find_utf16(&lower, "target").expect("literal match");
        assert_eq!(at, 101);
        assert_eq!(
            search_preview(&text, at, "TARGET".encode_utf16().count()),
            format!("…�{}aTARGET{}…", "😀".repeat(39), "x".repeat(120))
        );
    }

    #[test]
    fn ellipsize_preserves_short_text_and_trims_before_the_ellipsis() {
        assert_eq!(ellipsize("short", 90), "short");
        assert_eq!(ellipsize(&"a".repeat(90), 90), "a".repeat(90));
        assert_eq!(
            ellipsize(&"a".repeat(91), 90),
            format!("{}…", "a".repeat(89))
        );
        assert_eq!(ellipsize("abcdefghij ", 8), "abcdefg…");
        assert_eq!(ellipsize("abcde   extra", 8), "abcde…");
    }

    #[test]
    fn public_provider_reason_uses_the_shared_ellipsis_rule() {
        let long = format!("{} failed", "word ".repeat(30).trim());
        let reason = public_provider_reason(&long);
        assert!(reason.ends_with('…'));
        assert!(reason.chars().count() <= 90);
    }
}
