//! Models & providers → Refresh and the launcher's Try again (protocol 41):
//! `probeEngines` and `listTerminalLaunchProfiles` with `{refresh: true}`
//! find the provider CLIs again before they answer, so a CLI installed while
//! Interface runs appears without a restart.
//!
//! A rebuild runs a fresh login shell for its `PATH`, builds a new launch
//! catalog and repoints each engine adapter's executable
//! (`bootstrap::ProviderCatalogSource`), then swaps the catalog into the
//! terminal service. It is race-free with running panes and sessions: they
//! keep the process they started, every launch reads one catalog (and one
//! executable path) from start to spawn, and refreshes never overlap — one
//! that arrives while another runs waits and takes its result.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use interface_terminal::TerminalService;
use serde::Deserialize;
use serde_json::Value;

use crate::bootstrap::ProviderCatalogSource;
use crate::dispatcher::DispatchError;

/// The optional payload of `probeEngines` and `listTerminalLaunchProfiles`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefreshPayload {
    #[serde(default)]
    refresh: bool,
}

/// Whether a `probeEngines` or `listTerminalLaunchProfiles` payload asks
/// for a refresh: `null`/`{}` do not, `{refresh: true}` does.
///
/// # Errors
///
/// Anything else is refused, definitely unsent.
pub fn refresh_requested(
    payload: &Value,
    invalid: impl FnOnce() -> DispatchError,
) -> Result<bool, DispatchError> {
    if crate::payload::is_empty(payload) {
        return Ok(false);
    }
    if !payload.is_object() {
        return Err(invalid());
    }
    serde_json::from_value::<RefreshPayload>(payload.clone())
        .map(|payload| payload.refresh)
        .map_err(|_| invalid())
}

/// The optional payload of `listTerminalLaunchProfiles`: `refresh` (protocol
/// 41), `details` (protocol 43: each found CLI's `--version`, for Settings'
/// Terminal tools) and `paneTools` (protocol 48: turn one CLI's use of
/// Interface's tools on or off first; the answer is the details listing).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LaunchProfilesRequest {
    #[serde(default)]
    pub refresh: bool,
    #[serde(default)]
    pub details: bool,
    #[serde(default)]
    pub pane_tools: Option<interface_contracts::terminal::TerminalPaneToolsChange>,
}

/// Reads a `listTerminalLaunchProfiles` payload.
///
/// # Errors
///
/// Anything but `null`, `{}` or those two flags is refused, definitely unsent.
pub fn launch_profiles_request(
    payload: &Value,
    invalid: impl FnOnce() -> DispatchError,
) -> Result<LaunchProfilesRequest, DispatchError> {
    if crate::payload::is_empty(payload) {
        return Ok(LaunchProfilesRequest::default());
    }
    if !payload.is_object() {
        return Err(invalid());
    }
    serde_json::from_value(payload.clone()).map_err(|_| invalid())
}

/// Rebuilds what the host found at startup about the provider CLIs.
pub trait ProviderRebuild: Send + Sync + 'static {
    /// Blocking. Answers the new catalog, having repointed the engines.
    ///
    /// # Errors
    /// Nothing changed.
    fn rebuild(&self) -> Result<interface_terminal::TerminalLaunchCatalog, String>;
}

impl ProviderRebuild for ProviderCatalogSource {
    fn rebuild(&self) -> Result<interface_terminal::TerminalLaunchCatalog, String> {
        let rebuilt = Self::rebuild(self).map_err(|error| error.to_string())?;
        if !rebuilt.moved.is_empty() {
            tracing::info!(moved = ?rebuilt.moved, "provider CLIs found at new paths");
        }
        Ok(rebuilt.catalog)
    }
}

/// One host-wide refresher, shared by the runtime and terminal owners.
pub struct ProviderRefresh {
    source: Arc<dyn ProviderRebuild>,
    terminal: OnceLock<TerminalService>,
    /// When the last refresh finished; held across a refresh so they never overlap.
    finished: tokio::sync::Mutex<Option<Instant>>,
}

impl ProviderRefresh {
    #[must_use]
    pub fn new(source: Arc<dyn ProviderRebuild>) -> Self {
        Self {
            source,
            terminal: OnceLock::new(),
            finished: tokio::sync::Mutex::new(None),
        }
    }

    /// The terminal service the new catalog goes into, once it exists.
    pub fn attach_terminal(&self, terminal: TerminalService) {
        let _ = self.terminal.set(terminal);
    }

    /// Finds the provider CLIs again. A refresh that finished after this
    /// call began (one that was already running) answers for it too.
    ///
    /// # Errors
    ///
    /// The rebuild failed; the previous catalog and paths stay.
    pub async fn refresh(&self) -> Result<(), String> {
        self.refresh_from(false).await
    }

    /// Builds the launch catalog again even when a refresh just finished,
    /// after an owner setting the catalog reads changed (Settings › Threads ›
    /// Pane tools without asking, protocol 71): a refresh already running
    /// may have read the setting before it changed.
    ///
    /// # Errors
    ///
    /// The rebuild failed; the previous catalog and paths stay.
    pub async fn rebuild_after_setting_change(&self) -> Result<(), String> {
        self.refresh_from(true).await
    }

    async fn refresh_from(&self, always: bool) -> Result<(), String> {
        let asked = Instant::now();
        let mut finished = self.finished.lock().await;
        if !always && finished.is_some_and(|at| at >= asked) {
            return Ok(());
        }
        let source = Arc::clone(&self.source);
        let rebuilt = tokio::task::spawn_blocking(move || source.rebuild())
            .await
            .map_err(|_| "the provider refresh stopped".to_owned())?;
        *finished = Some(Instant::now());
        let catalog = rebuilt?;
        if let Some(terminal) = self.terminal.get() {
            terminal.replace_launch_catalog(catalog);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use interface_terminal::TerminalLaunchCatalog;
    use serde_json::json;

    use super::*;

    struct Counting(AtomicUsize);

    impl ProviderRebuild for Counting {
        fn rebuild(&self) -> Result<TerminalLaunchCatalog, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            Ok(TerminalLaunchCatalog::shell_only())
        }
    }

    fn invalid() -> DispatchError {
        DispatchError::new("invalid-payload", "bad", false, true)
    }

    #[test]
    fn a_launch_profile_listing_may_ask_for_a_refresh_and_versions() {
        assert_eq!(
            launch_profiles_request(&json!(null), invalid).unwrap(),
            LaunchProfilesRequest::default()
        );
        assert_eq!(
            launch_profiles_request(&json!({"details": true}), invalid).unwrap(),
            LaunchProfilesRequest {
                refresh: false,
                details: true,
                pane_tools: None,
            }
        );
        assert_eq!(
            launch_profiles_request(&json!({"refresh": true, "details": true}), invalid).unwrap(),
            LaunchProfilesRequest {
                refresh: true,
                details: true,
                pane_tools: None,
            }
        );
        assert_eq!(
            launch_profiles_request(
                &json!({"details": true, "paneTools": {"profileId": "gemini", "enabled": false}}),
                invalid
            )
            .unwrap()
            .pane_tools,
            Some(interface_contracts::terminal::TerminalPaneToolsChange {
                profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Gemini,
                enabled: false,
            })
        );
        for refused in [
            json!({"details": 1}),
            json!({"paths": true}),
            json!("details"),
            json!({"paneTools": {"profileId": "grok"}}),
            json!({"paneTools": {"profileId": "grok", "enabled": true, "path": "/x"}}),
        ] {
            assert!(
                launch_profiles_request(&refused, invalid).is_err(),
                "{refused}"
            );
        }
    }

    #[test]
    fn only_an_explicit_refresh_payload_asks_for_one() {
        assert!(!refresh_requested(&json!(null), invalid).unwrap());
        assert!(!refresh_requested(&json!({}), invalid).unwrap());
        assert!(!refresh_requested(&json!({"refresh": false}), invalid).unwrap());
        assert!(refresh_requested(&json!({"refresh": true}), invalid).unwrap());
        for refused in [
            json!([]),
            json!({"refresh": "yes"}),
            json!({"reload": true}),
            json!(1),
        ] {
            assert_eq!(
                refresh_requested(&refused, invalid).unwrap_err().code,
                "invalid-payload",
                "{refused}"
            );
        }
    }

    #[tokio::test]
    async fn refreshes_never_overlap_and_a_waiting_one_takes_the_running_result() {
        let source = Arc::new(Counting(AtomicUsize::new(0)));
        let refresh = Arc::new(ProviderRefresh::new(source.clone()));
        let first = tokio::spawn({
            let refresh = Arc::clone(&refresh);
            async move { refresh.refresh().await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Asked while the first runs: it waits, then shares that rebuild.
        refresh.refresh().await.unwrap();
        first.await.unwrap().unwrap();
        assert_eq!(source.0.load(Ordering::SeqCst), 1);
        // Asked after it finished: a new rebuild.
        refresh.refresh().await.unwrap();
        assert_eq!(source.0.load(Ordering::SeqCst), 2);
    }

    /// A rebuild after a setting change never shares a refresh that was
    /// already running: that one may have read the setting before it changed.
    #[tokio::test]
    async fn a_rebuild_after_a_setting_change_never_shares_a_running_refresh() {
        let source = Arc::new(Counting(AtomicUsize::new(0)));
        let refresh = Arc::new(ProviderRefresh::new(source.clone()));
        let first = tokio::spawn({
            let refresh = Arc::clone(&refresh);
            async move { refresh.refresh().await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        refresh.rebuild_after_setting_change().await.unwrap();
        first.await.unwrap().unwrap();
        assert_eq!(source.0.load(Ordering::SeqCst), 2);
    }
}
