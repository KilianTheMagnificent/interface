//! Folder trust in Threads (protocol 72): each untrusted Claude thread's
//! view carries `folderTrust`; `folderTrust` reads where a folder stands
//! for a new thread; `trustFolder` records the owner's answer and, after a
//! trust, ends the provider process of the folder's idle Claude threads so
//! their next message starts it again with the folder's settings (resume is
//! a fresh CLI launch, and the adapter asks the trust again then).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use interface_common::sync::lock;
use interface_contracts::{
    ipc::parse_key,
    threads::{
        FolderTrustAnswer, FolderTrustState as WireState, ThreadControlResponse, ThreadFolderTrust,
        ThreadsView, valid_folder_trust_path,
    },
};
use interface_desktop_services::{
    DesktopEffectCertainty, DesktopServiceError, FolderTrustDecision, FolderTrustState,
};

use super::ThreadHost;
use crate::dispatcher::DispatchError;
use crate::folder_trust::{CLAUDE_ENGINE, HostFolderTrust};

/// A folder path `folderTrust` or `trustFolder` cannot take: relative,
/// overlong, with control characters, or a folder that does not exist.
/// Definitely unsent.
pub const THREADS_FOLDER_TRUST_REFUSED_CODE: &str = "threads-folder-trust-refused";
/// This host has no folder trust (it does not own the stores). Definitely
/// unsent.
pub const THREADS_FOLDER_TRUST_UNAVAILABLE_CODE: &str = "threads-folder-trust-unavailable";

/// How long a thread mid-turn is waited for before its process is left to
/// pick up the trust at its next start.
const RESTART_WAIT: Duration = Duration::from_secs(12 * 60 * 60);
/// How often a waiting restart looks again when nothing woke it.
const RESTART_POLL: Duration = Duration::from_secs(30);

/// The decisions Threads reads; tests replace it.
pub trait ThreadFolderTrustPort: Send + Sync + 'static {
    /// A session of Claude account `account` (`None`: Default) in `cwd`.
    fn decide(&self, cwd: &Path, account: Option<&str>) -> FolderTrustDecision;
    /// A new Claude thread in `folder`, on the account a launch naming none uses.
    fn decide_new(&self, folder: &Path) -> FolderTrustDecision;
    /// Records the owner's answer for `folder`'s trust root; answers it.
    ///
    /// # Errors
    /// Why nothing (or possibly something) was recorded.
    fn record(&self, folder: &Path, trust: bool) -> Result<PathBuf, DesktopServiceError>;
}

impl ThreadFolderTrustPort for HostFolderTrust {
    fn decide(&self, cwd: &Path, account: Option<&str>) -> FolderTrustDecision {
        Self::decide(self, cwd, account)
    }

    fn decide_new(&self, folder: &Path) -> FolderTrustDecision {
        Self::decide_new(self, folder)
    }

    fn record(&self, folder: &Path, trust: bool) -> Result<PathBuf, DesktopServiceError> {
        Self::record(self, folder, trust)
    }
}

fn refused(message: &str) -> DispatchError {
    DispatchError::new(THREADS_FOLDER_TRUST_REFUSED_CODE, message, false, true)
}

fn wire_state(state: FolderTrustState) -> WireState {
    match state {
        FolderTrustState::Trusted => WireState::Trusted,
        FolderTrustState::Unasked => WireState::Unasked,
        FolderTrustState::Declined => WireState::Declined,
    }
}

fn display(path: &Path) -> Option<String> {
    path.to_str().map(ToOwned::to_owned)
}

/// What a thread's view says about its folder: present only when the folder
/// exists and is not trusted.
fn thread_folder_trust(decision: &FolderTrustDecision) -> Option<ThreadFolderTrust> {
    if decision.trusted() {
        return None;
    }
    Some(ThreadFolderTrust {
        folder: display(decision.root.as_deref()?)?,
        asked: decision.state == FolderTrustState::Declined,
    })
}

fn is_claude(key: &str) -> bool {
    parse_key(key).is_ok_and(|(engine, _)| engine.as_str() == CLAUDE_ENGINE)
}

impl ThreadHost {
    /// Installs the folder trust decision once (protocol 72).
    pub fn attach_folder_trust(&self, trust: Arc<dyn ThreadFolderTrustPort>) {
        let _ = self.inner.folder_trust.set(trust);
    }

    /// Puts `folderTrust` on each Claude thread whose folder is not trusted.
    /// Run after [`Self::with_accounts`], whose account it reads.
    pub(super) fn with_folder_trust(&self, view: &mut ThreadsView) {
        let Some(trust) = self.inner.folder_trust.get() else {
            return;
        };
        for thread in &mut view.threads {
            if !is_claude(&thread.key) {
                continue;
            }
            let Ok(Some(summary)) = self.inner.ports.sessions.session(&thread.key) else {
                continue;
            };
            let decision = trust.decide(Path::new(&summary.cwd), thread.account.as_deref());
            thread.folder_trust = thread_folder_trust(&decision);
        }
    }

    fn folder_trust_port(&self) -> Result<Arc<dyn ThreadFolderTrustPort>, DispatchError> {
        self.inner.folder_trust.get().cloned().ok_or_else(|| {
            DispatchError::new(
                THREADS_FOLDER_TRUST_UNAVAILABLE_CODE,
                "Folder trust is not available in this Interface.",
                false,
                true,
            )
        })
    }

    /// `folderTrust`: where `folder` stands for a new Claude thread.
    pub(super) async fn folder_trust(
        &self,
        folder: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if !valid_folder_trust_path(&folder) {
            return Err(refused("That is not a folder Interface can check."));
        }
        let trust = self.folder_trust_port()?;
        let decision = tokio::task::spawn_blocking(move || trust.decide_new(Path::new(&folder)))
            .await
            .map_err(|_| refused("The folder could not be checked."))?;
        let root = decision
            .root
            .as_deref()
            .and_then(display)
            .ok_or_else(|| refused("That folder is not available."))?;
        let mut response = self.view_response().await?;
        response.folder_trust = Some(FolderTrustAnswer {
            folder: root,
            state: wire_state(decision.state),
        });
        Ok(response)
    }

    /// `trustFolder`: records the owner's answer for `folder`'s trust root.
    /// After a trust, the root's idle Claude threads restart their provider
    /// process on their next message.
    pub(super) async fn trust_folder(
        &self,
        folder: String,
        trust: bool,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if !valid_folder_trust_path(&folder) {
            return Err(refused("That is not a folder Interface can trust."));
        }
        let port = self.folder_trust_port()?;
        let (root, decision) = tokio::task::spawn_blocking(move || {
            let root = port.record(Path::new(&folder), trust)?;
            let decision = port.decide_new(&root);
            Ok::<_, DesktopServiceError>((root, decision))
        })
        .await
        .map_err(|_| {
            DispatchError::new(
                THREADS_FOLDER_TRUST_REFUSED_CODE,
                "Interface stopped before it knew whether your answer was saved.",
                false,
                false,
            )
        })?
        .map_err(|error| {
            let replay_safe = error.certainty != DesktopEffectCertainty::OutcomeUnknown;
            DispatchError::new(error.code, error.message, false, replay_safe)
        })?;
        if trust {
            self.restart_sessions_in(root.clone()).await;
        }
        let mut response = self.view_response().await?;
        response.folder_trust = Some(FolderTrustAnswer {
            folder: display(&root).unwrap_or_default(),
            state: wire_state(decision.state),
        });
        Ok(response)
    }

    /// The live Claude sessions of listed threads whose folder's trust root
    /// is `root`, each restarted once it is idle. A thread an Away loop
    /// holds, a session a Code pane drives and a side thread are left as
    /// they are: they pick up the trust the next time their process starts.
    async fn restart_sessions_in(&self, root: PathBuf) {
        let Some(trust) = self.inner.folder_trust.get().cloned() else {
            return;
        };
        let sessions = Arc::clone(&self.inner.ports.sessions);
        let accounts = self.accounts().cloned();
        let candidates = tokio::task::spawn_blocking(move || {
            let live = sessions.live_keys().unwrap_or_default();
            let Ok(rows) = sessions.sessions() else {
                return Vec::new();
            };
            rows.into_iter()
                .filter_map(|summary| {
                    let key = interface_contracts::engine::session_key(
                        &summary.engine,
                        &summary.session_id,
                    );
                    if summary.engine.as_str() != CLAUDE_ENGINE || !live.contains(&key) {
                        return None;
                    }
                    let account = accounts
                        .as_ref()
                        .and_then(|accounts| accounts.account_of_key(&key));
                    let decision = trust.decide(Path::new(&summary.cwd), account.as_deref());
                    (decision.root.as_deref() == Some(root.as_path())).then_some(key)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        if candidates.is_empty() {
            return;
        }
        let listed = candidates.clone();
        let listed: HashSet<String> = self
            .peek(move |document, _| {
                listed
                    .into_iter()
                    .filter(|key| document.record(key).is_some())
                    .collect()
            })
            .await
            .unwrap_or_default();
        for key in candidates {
            if !listed.contains(&key)
                || self.away_unheld(&key).is_err()
                || self.inner.ports.claims.claim(&key).is_some()
                || lock(&self.inner.side_keys).contains(&key)
            {
                continue;
            }
            let host = self.clone();
            self.spawn_tracked(async move { host.restart_when_idle(key).await });
        }
    }

    /// Closes `key`'s live provider session once no turn runs and no send
    /// is in flight; the next message resumes it as a fresh CLI launch. A
    /// close that is not confirmed leaves the session as it was.
    async fn restart_when_idle(&self, key: String) {
        let deadline = tokio::time::Instant::now() + RESTART_WAIT;
        let mut changes = self.inner.changes.subscribe();
        let mut stop = self.inner.stop.subscribe();
        loop {
            changes.borrow_and_update();
            let live = self
                .inner
                .ports
                .sessions
                .live_keys()
                .is_ok_and(|live| live.contains(&key));
            if !live || *stop.borrow() {
                return;
            }
            if !self.busy(&key)
                && let Ok(_gate) = self.hold_send_gate(&key)
            {
                // Checked again under the gate: a run the provider started
                // on its own is never cut off.
                if !self.busy(&key) {
                    if let Err(error) = self.inner.ports.sessions.close(&key).await {
                        tracing::warn!(%error, "a trusted folder's thread kept its process");
                    }
                    return;
                }
            }
            tokio::select! {
                moved = changes.changed() => if moved.is_err() { return; },
                () = tokio::time::sleep(RESTART_POLL) => {}
                () = tokio::time::sleep_until(deadline) => return,
                _ = stop.changed() => return,
            }
        }
    }
}
