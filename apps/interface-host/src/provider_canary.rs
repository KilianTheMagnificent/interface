//! Protocol 51: the provider CLI drift canary in the host (review F2,
//! 2026-10-15).
//!
//! Whenever a CLI's `--version` differs from the one the host last checked,
//! or Interface's own expectations changed, the host runs
//! `interface_terminal::provider_canary` for that CLI — its version, help,
//! listing and schema output and the installed files; never a model turn —
//! and keeps what it found in `provider-canary.v1.json`. Settings → Terminal
//! tools shows the last change as "Changed since last check": for a week, or
//! for as long as something Interface depends on still does not match.
//!
//! The check runs when a details listing (Settings) is read and once, in the
//! background, shortly after the host starts. One check runs at a time.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use interface_common::time::{IsoFraction, unix_millis_iso};
use interface_contracts::terminal::{
    TERMINAL_TOOL_CHANGE_VERSION, TerminalLaunchProfileId, TerminalToolChangeV1,
};
use interface_settings::{ProviderCanaryChange, ProviderCanarySeen, ProviderCanaryStore};
use interface_terminal::provider_canary::{
    expectation, expectations_fingerprint, run_canary, version_number,
};

/// Beside `session-access.v1.json` in the private data root.
pub const PROVIDER_CANARY_FILE_NAME: &str = "provider-canary.v1.json";

/// How long a change that still matches stays in Settings.
const SHOWN_FOR_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// How long after the host starts its background check runs.
pub const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(45);

/// The host's record of each CLI and the lock that keeps one check at a time.
pub struct ProviderCanaryWatch {
    store: tokio::sync::Mutex<ProviderCanaryStore>,
}

/// One found CLI: its profile, executable and `--version` line.
pub type FoundCli = (TerminalLaunchProfileId, PathBuf, String);

/// What `observe` does with a CLI the host has never checked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirstSight {
    /// Check it now (the background pass).
    Check,
    /// Hand it back to be checked in the background, so a Settings listing
    /// waits only on CLIs whose version changed.
    Defer,
}

impl ProviderCanaryWatch {
    /// The store in `data_root`, or `None` (logged) when it cannot be opened:
    /// the canary then never runs in the host, and nothing else changes.
    #[must_use]
    pub fn open(data_root: &Path) -> Option<Arc<Self>> {
        Self::open_file(&data_root.join(PROVIDER_CANARY_FILE_NAME))
    }

    #[must_use]
    pub fn open_file(path: &Path) -> Option<Arc<Self>> {
        match ProviderCanaryStore::open(path) {
            Ok(store) => {
                if store.quarantined() {
                    tracing::warn!(
                        "the provider CLI check record was unreadable; it was set aside and every CLI is checked again"
                    );
                }
                Some(Arc::new(Self {
                    store: tokio::sync::Mutex::new(store),
                }))
            }
            Err(error) => {
                tracing::warn!(%error, "the provider CLI check is unavailable");
                None
            }
        }
    }

    /// What Settings shows for each found CLI, after checking any whose
    /// version changed since it was last checked, and the CLIs `first_sight`
    /// deferred.
    pub async fn observe(
        &self,
        found: Vec<FoundCli>,
        first_sight: FirstSight,
    ) -> (
        BTreeMap<TerminalLaunchProfileId, TerminalToolChangeV1>,
        Vec<FoundCli>,
    ) {
        let fingerprint = expectations_fingerprint();
        let mut store = self.store.lock().await;
        let mut shown = BTreeMap::new();
        let mut deferred = Vec::new();
        for (profile, executable, version) in found {
            let Some(expected) = expectation(profile) else {
                continue;
            };
            let key = profile_key(profile);
            let mut seen = store.seen(&key).cloned();
            if seen.is_none() && first_sight == FirstSight::Defer {
                deferred.push((profile, executable, version));
                continue;
            }
            let stale = seen
                .as_ref()
                .is_none_or(|seen| seen.version != version || seen.expectations != fingerprint);
            if stale {
                let checked =
                    tokio::task::spawn_blocking(move || run_canary(expected, &executable)).await;
                let Ok(report) = checked else {
                    tracing::warn!(%key, "the provider CLI check stopped unexpectedly");
                    continue;
                };
                let now = now_ms();
                let previous = seen.as_ref();
                let version_changed = previous.is_some_and(|seen| seen.version != version);
                let change = if version_changed || !report.findings.is_empty() {
                    Some(ProviderCanaryChange {
                        from: previous
                            .filter(|_| version_changed)
                            .map(|seen| shown_number(&seen.version)),
                        to: shown_number(&version),
                        at: now,
                        findings: report.findings,
                    })
                } else {
                    // Same version, nothing wrong now: a still-matching
                    // update stays shown; findings since fixed go.
                    previous
                        .and_then(|seen| seen.change.clone())
                        .filter(|change| change.findings.is_empty())
                };
                let record = ProviderCanarySeen {
                    version: version.clone(),
                    expectations: fingerprint.clone(),
                    checked_at: now,
                    change,
                };
                if let Err(error) = store.record(&key, record.clone()) {
                    tracing::warn!(%key, %error, "the provider CLI check was not kept");
                }
                seen = Some(record);
            }
            if let Some(change) = seen.and_then(|seen| seen.change)
                && (!change.findings.is_empty()
                    || now_ms().saturating_sub(change.at) <= SHOWN_FOR_MS)
            {
                shown.insert(
                    profile,
                    TerminalToolChangeV1 {
                        version: TERMINAL_TOOL_CHANGE_VERSION,
                        from: change.from,
                        to: change.to,
                        checked_at: unix_millis_iso(
                            i64::try_from(change.at).unwrap_or(i64::MAX),
                            IsoFraction::Millis,
                        ),
                        findings: change.findings,
                    },
                );
            }
        }
        (shown, deferred)
    }
}

fn profile_key(profile: TerminalLaunchProfileId) -> String {
    serde_json::to_value(profile)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `2.1.300` from `2.1.300 (Claude Code)`; the line itself when it has no
/// dotted number.
fn shown_number(version: &str) -> String {
    version_number(version).unwrap_or_else(|| version.to_owned())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
