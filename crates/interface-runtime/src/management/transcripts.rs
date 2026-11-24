//! Provider-owned transcripts: reading one session's stored history, and the
//! bounded literal search across them.

use std::{collections::HashMap, sync::Arc, time::Duration};

use interface_contracts::{
    engine::{EngineEventKind, SessionSummary, session_key},
    ipc::{
        HISTORY_LIMIT, HISTORY_MAX_LIMIT, HistoryAvailability, HistoryPage, SessionSearchHit,
        SessionSearchMatch, SessionSearchMatchKind, SessionSearchProviderNotice,
        SessionSearchResponse, parse_key, unavailable_history,
    },
};
use interface_engine::HistoryRequest;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    RuntimeError, RuntimeErrorKind, SessionRuntime,
    runtime::{blocking, lock},
};

use super::{
    IMPORT_CONCURRENCY, IMPORT_HISTORY_LIMIT, exceeds_javascript_string_limit, local_error,
};

const SESSION_HISTORY_TIMEOUT: Duration = Duration::from_secs(5);

const SEARCH_SESSION_LIMIT: usize = 100;
const SEARCH_HIT_LIMIT: usize = 100;
pub(super) const SEARCH_MATCH_LIMIT: usize = 10;
const SEARCH_QUERY_CHAR_LIMIT: usize = 256;

impl SessionRuntime {
    /// Reads the provider-owned transcript for a stored or live session.
    ///
    /// This is intentionally separate from [`SessionRuntime::history_session`],
    /// which is the Rust host's effect/event audit. Interface 0.1.169 used the
    /// provider's canonical store for `readHistory`; substituting the new audit
    /// would make every pre-port conversation appear empty.
    ///
    /// # Errors
    ///
    /// Returns a replay-safe, sanitized read failure for an invalid key, an
    /// unregistered provider, an unavailable registry, or a provider read
    /// failure. Provider-native paths and diagnostics are not exposed.
    pub async fn read_provider_history(
        &self,
        session_key: &str,
        requested_limit: u64,
    ) -> Result<HistoryPage, RuntimeError> {
        self.read_provider_history_within(
            session_key,
            requested_limit.clamp(HISTORY_LIMIT, HISTORY_MAX_LIMIT),
        )
        .await
    }

    /// [`Self::read_provider_history`] for the thread journal's one-time
    /// import: the newest [`IMPORT_HISTORY_LIMIT`] items, past the renderer's
    /// [`HISTORY_MAX_LIMIT`], so a long conversation is imported whole.
    ///
    /// # Errors
    ///
    /// As [`Self::read_provider_history`].
    pub async fn read_provider_history_for_import(
        &self,
        session_key: &str,
    ) -> Result<HistoryPage, RuntimeError> {
        self.read_provider_history_within(session_key, IMPORT_HISTORY_LIMIT)
            .await
    }

    async fn read_provider_history_within(
        &self,
        session_key: &str,
        limit: u64,
    ) -> Result<HistoryPage, RuntimeError> {
        self.ensure_accepting()?;
        let (engine, provider_session_id) = parse_key(session_key).map_err(|_| {
            local_error(
                RuntimeErrorKind::InvalidInput,
                "Can't open that chat.\nInterface could not identify its provider session. Its transcript is unchanged on disk.",
                false,
            )
        })?;
        let Some(adapter) = self.inner.adapters.get(&engine) else {
            return Err(local_error(
                RuntimeErrorKind::EngineNotRegistered,
                "Can't open that chat.\nInterface has no engine that can read this session. Its transcript is unchanged on disk.",
                false,
            ));
        };
        if !adapter.supports_history() {
            return Ok(unavailable_history(format!(
                "{} does not expose stored transcript replay.",
                adapter.id()
            )));
        }

        let registry = self.inner.services.session_registry();
        let session_key = session_key.to_owned();
        let cwd = blocking(move || {
            Ok(lock(&registry, "session registry")?
                .get(&session_key)
                .map_or_else(String::new, |summary| summary.cwd.clone()))
        })
        .await?;
        adapter
            .read_history(
                provider_session_id,
                HistoryRequest {
                    cwd,
                    limit: usize::try_from(limit).unwrap_or(usize::MAX),
                },
            )
            .await
            .map_err(|error| {
                local_error(
                    RuntimeErrorKind::Provider,
                    format!(
                        "Couldn't load this chat.\nInterface could not read the earlier messages of this {engine} session. Nothing has been changed; resume it to carry on, and the conversation from here forward will stream normally."
                    ),
                    error.retryable,
                )
            })
    }

    /// Bounded literal search over provider-owned transcript replay.
    ///
    /// # Errors
    ///
    /// Returns query validation or registry-projection read failures. Individual
    /// provider failures are isolated in the returned provider notices.
    #[allow(clippy::too_many_lines)]
    pub async fn search_sessions(
        &self,
        query: String,
    ) -> Result<SessionSearchResponse, RuntimeError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err(local_error(
                RuntimeErrorKind::InvalidInput,
                "Session search needs a non-empty literal query.",
                false,
            ));
        }
        if exceeds_javascript_string_limit(&query, SEARCH_QUERY_CHAR_LIMIT) {
            return Err(local_error(
                RuntimeErrorKind::InvalidInput,
                format!("Use at most {SEARCH_QUERY_CHAR_LIMIT} characters."),
                false,
            ));
        }
        let needle = query.to_lowercase();
        let query_units = query.encode_utf16().count();
        let mut selected = self.registry_all().await?;
        selected.sort_by(|left, right| right.last_activity.cmp(&left.last_activity));
        let skipped = selected.split_off(selected.len().min(SEARCH_SESSION_LIMIT));
        let mut notices = self
            .inner
            .adapters
            .iter()
            .map(|(engine, adapter)| {
                (
                    engine.clone(),
                    if adapter.supports_history() {
                        SessionSearchProviderNotice {
                            engine: engine.clone(),
                            complete: true,
                            error: None,
                        }
                    } else {
                        SessionSearchProviderNotice {
                            engine: engine.clone(),
                            complete: false,
                            error: Some(
                                "This provider does not expose stored transcript replay.".into(),
                            ),
                        }
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        for skipped in &skipped {
            notices.insert(
                skipped.engine.clone(),
                SessionSearchProviderNotice {
                    engine: skipped.engine.clone(),
                    complete: false,
                    error: Some("The bounded session limit was reached.".into()),
                },
            );
        }
        let semaphore = Arc::new(Semaphore::new(IMPORT_CONCURRENCY));
        let mut work = JoinSet::new();
        for summary in selected {
            let Some(adapter) = self.inner.adapters.get(&summary.engine).cloned() else {
                continue;
            };
            if !adapter.supports_history() {
                continue;
            }
            let needle = needle.clone();
            let semaphore = Arc::clone(&semaphore);
            work.spawn(async move {
                let _permit = semaphore.acquire_owned().await.ok()?;
                let page = tokio::time::timeout(
                    SESSION_HISTORY_TIMEOUT,
                    adapter.read_history(
                        summary.session_id.clone(),
                        HistoryRequest {
                            cwd: summary.cwd.clone(),
                            limit: usize::try_from(HISTORY_MAX_LIMIT).unwrap_or(5_000),
                        },
                    ),
                )
                .await;
                let Ok(Ok(page)) = page else {
                    return Some(SearchedSession::Unread(summary));
                };
                if matches!(page.availability, Some(HistoryAvailability::Unavailable)) {
                    return Some(SearchedSession::Unavailable(summary, page.reason));
                }
                // The lowercase-and-find scan is CPU-bound over up to
                // HISTORY_MAX_LIMIT events: keep it off the async workers.
                let (matches, truncated) = tokio::task::spawn_blocking(move || {
                    scan_history_page(page.events, &needle, query_units)
                })
                .await
                .ok()?;
                Some(SearchedSession::Scanned {
                    summary,
                    matches,
                    truncated,
                })
            });
        }
        let mut hits = Vec::new();
        let mut sessions_scanned = 0_u64;
        let mut truncated = !skipped.is_empty();
        while let Some(joined) = work.join_next().await {
            let Ok(Some(searched)) = joined else {
                continue;
            };
            let (summary, matches) = match searched {
                SearchedSession::Unread(summary) => {
                    notices.insert(
                        summary.engine.clone(),
                        SessionSearchProviderNotice {
                            engine: summary.engine,
                            complete: false,
                            error: Some(
                                "At least one stored transcript could not be searched.".into(),
                            ),
                        },
                    );
                    continue;
                }
                SearchedSession::Unavailable(summary, reason) => {
                    sessions_scanned = sessions_scanned.saturating_add(1);
                    notices.insert(
                        summary.engine.clone(),
                        SessionSearchProviderNotice {
                            engine: summary.engine,
                            complete: false,
                            error: Some(reason.unwrap_or_else(|| {
                                "This provider does not expose stored transcript replay.".into()
                            })),
                        },
                    );
                    continue;
                }
                SearchedSession::Scanned {
                    summary,
                    matches,
                    truncated: page_truncated,
                } => {
                    sessions_scanned = sessions_scanned.saturating_add(1);
                    truncated |= page_truncated;
                    (summary, matches)
                }
            };
            if matches.is_empty() {
                continue;
            }
            if hits.len() < SEARCH_HIT_LIMIT {
                hits.push(SessionSearchHit {
                    key: session_key(&summary.engine, &summary.session_id),
                    title: summary.title,
                    cwd: summary.cwd,
                    matches,
                });
            } else {
                truncated = true;
            }
        }
        hits.sort_by(|left, right| left.title.cmp(&right.title));
        let mut providers = notices.into_values().collect::<Vec<_>>();
        providers.sort_by(|left, right| left.engine.as_str().cmp(right.engine.as_str()));
        Ok(SessionSearchResponse {
            hits,
            providers,
            sessions_scanned,
            truncated,
        })
    }
}

/// What one session's search task hands back to the ordered merge.
enum SearchedSession {
    /// The history read failed or timed out.
    Unread(SessionSummary),
    /// The provider answered that stored replay is unavailable.
    Unavailable(SessionSummary, Option<String>),
    Scanned {
        summary: SessionSummary,
        matches: Vec<SessionSearchMatch>,
        truncated: bool,
    },
}

/// Literal, case-insensitive scan of one history page; `true` when it
/// stopped at `SEARCH_MATCH_LIMIT`.
pub(super) fn scan_history_page(
    events: Vec<interface_contracts::engine::EngineEvent>,
    needle: &str,
    query_units: usize,
) -> (Vec<SessionSearchMatch>, bool) {
    let mut matches = Vec::new();
    for event in events {
        let Some((kind, text)) = searchable_event(&event.event) else {
            continue;
        };
        let lower = text.to_lowercase();
        let Some(at) = find_utf16(&lower, needle) else {
            continue;
        };
        matches.push(SessionSearchMatch {
            kind,
            preview: search_preview(text, at, query_units),
        });
        if matches.len() >= SEARCH_MATCH_LIMIT {
            return (matches, true);
        }
    }
    (matches, false)
}

fn searchable_event(event: &EngineEventKind) -> Option<(SessionSearchMatchKind, &str)> {
    match event {
        EngineEventKind::UserMessage { text, .. } => {
            Some((SessionSearchMatchKind::User, text.as_str()))
        }
        EngineEventKind::MessageDelta { text, .. } => {
            Some((SessionSearchMatchKind::Assistant, text.as_str()))
        }
        EngineEventKind::ToolCall { summary, .. } | EngineEventKind::ToolResult { summary, .. } => {
            Some((SessionSearchMatchKind::Tool, summary.as_str()))
        }
        _ => None,
    }
}

pub(super) fn find_utf16(haystack: &str, needle: &str) -> Option<usize> {
    let byte_at = haystack.find(needle)?;
    Some(haystack[..byte_at].encode_utf16().count())
}

pub(super) fn search_preview(text: &str, unit_at: usize, query_units: usize) -> String {
    let total_units = text.encode_utf16().count();
    let start = unit_at.saturating_sub(80);
    let end = total_units.min(
        unit_at
            .saturating_add(query_units.max(1))
            .saturating_add(120),
    );
    // JavaScript String#slice is indexed in UTF-16 units and can return a lone
    // surrogate when a boundary bisects an astral scalar. Rust String cannot
    // represent that state, so the closest transport-safe projection is the
    // same UTF-16 slice decoded with the standard replacement character.
    let slice = text
        .encode_utf16()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect::<Vec<_>>();
    let flat = String::from_utf16_lossy(&slice)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        flat,
        if end < total_units { "…" } else { "" }
    )
}
