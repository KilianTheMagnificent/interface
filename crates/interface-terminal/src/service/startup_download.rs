//! A pane whose program is waiting on evicted startup files says so.
//!
//! Create and restart start the cwd's startup-file fetch, or join the one
//! already running there. The service ticker projects what is still in flight
//! onto the pane's summary, and stops once nothing is. Every few minutes it
//! also re-fetches the cwds of running panes, so a File Provider that evicted
//! those files again is asked for them before the next pane needs them.
use std::collections::BTreeSet;

use super::{
    Arc, Duration, Path, PathBuf, Shared, StartupFetch, TerminalGeneration, TerminalId,
    TerminalLifecycleEvent, TerminalState, Weak, lock, now,
};
use crate::startup_files::startup_download;

/// Local files resolve in microseconds. A file still pending after this long
/// is being downloaded, so only then does the pane say so.
const STARTUP_DOWNLOAD_GRACE: Duration = Duration::from_secs(1);

/// The cwd's live fetch, or a new one. Create and restart hold it on the new
/// entry, with its refresh armed on the ticker.
pub(super) fn startup_fetch(shared: &Shared, cwd: &Path) -> Arc<StartupFetch> {
    let mut fetches = lock(&shared.startup_fetches);
    fetches.retain(|_, fetch| fetch.strong_count() > 0);
    if let Some(fetch) = fetches.get(cwd).and_then(Weak::upgrade) {
        return fetch;
    }
    let fetch = StartupFetch::start(
        Arc::clone(&shared.startup_files),
        cwd,
        shared.home.as_deref(),
    );
    fetches.insert(cwd.to_path_buf(), Arc::downgrade(&fetch));
    fetch
}

/// One tick: publish the pane's pending startup files. `false` once none are
/// pending or the generation is gone, which ends its refresh.
pub(super) fn refresh_startup_download(
    shared: &Weak<Shared>,
    id: TerminalId,
    generation: TerminalGeneration,
) -> bool {
    let Some(shared) = shared.upgrade() else {
        return false;
    };
    let event_gate = lock(&shared.event_gate);
    let (event, done) = {
        let mut state = lock(&shared.state);
        let Some(entry) = state
            .entries
            .get_mut(&id)
            .filter(|entry| entry.summary.generation == generation)
        else {
            return false;
        };
        let Some(fetch) = entry
            .startup_fetch
            .clone()
            .filter(|_| entry.summary.state == TerminalState::Running)
        else {
            entry.startup_fetch = None;
            return false;
        };
        let pending = fetch.pending();
        if !pending.is_empty()
            && entry.summary.startup_download.is_none()
            && fetch.elapsed() < STARTUP_DOWNLOAD_GRACE
        {
            return true;
        }
        let download = startup_download(&pending, &entry.cwd, shared.home.as_deref());
        let done = download.is_none();
        if done {
            entry.startup_fetch = None;
        }
        if entry.summary.startup_download == download {
            return !done;
        }
        entry.summary.startup_download = download;
        entry.summary.updated_at = now();
        let summary = entry.summary.clone();
        state.revision = state.revision.saturating_add(1);
        let event = TerminalLifecycleEvent::Upsert {
            revision: state.revision,
            terminal: summary,
        };
        (event, done)
    };
    shared.events.publish(event);
    drop(event_gate);
    !done
}

/// Re-fetches the startup files of every running pane's cwd.
pub(super) fn warm_startup_files(shared: &Shared) {
    let cwds: BTreeSet<PathBuf> = lock(&shared.state)
        .entries
        .values()
        .filter(|entry| entry.summary.state == TerminalState::Running && !entry.closing)
        .map(|entry| entry.cwd.clone())
        .collect();
    for cwd in cwds {
        drop(startup_fetch(shared, &cwd));
    }
}
