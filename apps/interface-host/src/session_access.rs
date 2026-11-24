//! Host owner of `session-access.v1.json`: the permission mode the owner
//! chose for each session outside the agent roster, which the runtime
//! dispatcher records at launch and on every change and applies on resume.
//! A store that does not open leaves resumes in each provider's default, as
//! before; it never stops the host.

use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use interface_common::sync::lock;
use interface_contracts::engine::{SessionKind, SessionSummary, session_key};
use interface_settings::SessionAccessStore;

use crate::dispatcher::SessionAccessMemory;

/// Beside `threads.v2.json` in the private data root.
pub const SESSION_ACCESS_FILE_NAME: &str = "session-access.v1.json";

/// [`SessionAccessMemory`] over the store.
pub struct HostSessionAccess {
    store: Mutex<SessionAccessStore>,
    /// Moves on every remember (protocol 68: the registry push carries modes).
    revision: AtomicU64,
}

impl HostSessionAccess {
    /// Where an unreadable file found at open was set aside, if it was.
    #[must_use]
    pub fn quarantined_artifact(&self) -> Option<std::path::PathBuf> {
        interface_common::sync::lock(&self.store)
            .quarantined_artifact()
            .map(std::path::Path::to_path_buf)
    }

    /// The store in `data_root`, or `None` (logged) when it cannot be opened.
    #[must_use]
    pub fn open(data_root: &Path) -> Option<Arc<Self>> {
        match SessionAccessStore::open(&data_root.join(SESSION_ACCESS_FILE_NAME)) {
            Ok(store) => {
                if store.quarantined() {
                    tracing::warn!(
                        "session permission modes were unreadable; they were set aside and every session resumes in its default"
                    );
                }
                Some(Arc::new(Self {
                    store: Mutex::new(store),
                    revision: AtomicU64::new(0),
                }))
            }
            Err(error) => {
                tracing::warn!(%error, "session permission modes are unavailable; sessions resume in their defaults");
                None
            }
        }
    }
}

impl SessionAccessMemory for HostSessionAccess {
    fn mode(&self, session_key: &str) -> Option<String> {
        lock(&self.store).mode(session_key).map(str::to_owned)
    }

    fn remember(&self, session_key: &str, mode: Option<&str>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            });
        if let Err(error) = lock(&self.store).remember(session_key, mode, now) {
            tracing::warn!(%session_key, %error, "a session's permission mode was not kept");
        }
        self.revision.fetch_add(1, Ordering::Relaxed);
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }
}

/// Protocol 68: puts on each listed session the permission mode the host runs
/// it in and resumes it in, as `memory` keeps it: its remembered mode, else
/// `default` (a session launched or resumed in its provider's default keeps
/// no entry). Chat and Voice have no permission controls and an agent's
/// session follows its agent's access, so those carry none. Only the
/// desktop's copies are marked; the registry cache never holds the mode.
pub fn with_permission_modes(sessions: &mut [SessionSummary], memory: &dyn SessionAccessMemory) {
    for summary in sessions {
        summary.permission_mode = match summary.kind {
            Some(SessionKind::Chat | SessionKind::Voice | SessionKind::Agent) => None,
            Some(SessionKind::Code) | None => Some(
                memory
                    .mode(&session_key(&summary.engine, &summary.session_id))
                    .unwrap_or_else(|| DEFAULT_MODE.to_owned()),
            ),
        };
    }
}

/// [`with_permission_modes`] when the host keeps modes at all.
pub fn mark_permission_modes(
    sessions: &mut [SessionSummary],
    memory: Option<&Arc<dyn SessionAccessMemory>>,
) {
    if let Some(memory) = memory {
        with_permission_modes(sessions, memory.as_ref());
    }
}

/// The mode a session with no remembered one runs in.
const DEFAULT_MODE: &str = "default";
