//! The coalesced, best-effort writer of the provider-reconstructible
//! `registry.json` cache: a 1 s writer for content changes, a separate 15 s
//! timer for activity-only changes, and the flush a stream close or shutdown
//! waits on.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use interface_registry::RegistrySave;
use tokio::sync::Notify;

use super::{RuntimeServices, blocking, lock};

const REGISTRY_SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// A provider event that moved only `last_activity` reaches the
/// provider-reconstructible cache on this slower cadence. Status and every
/// other change keep the 1 s writer, and a stream close or shutdown flushes
/// pending activity at once, so only crash-time `last_activity` can lag.
const REGISTRY_ACTIVITY_SAVE_DEBOUNCE: Duration = Duration::from_secs(15);

#[derive(Debug, Default)]
pub(super) struct RegistryCacheState {
    dirty: AtomicBool,
    writer_running: AtomicBool,
    activity_dirty: AtomicBool,
    activity_timer_running: AtomicBool,
    wake: Notify,
    idle: Notify,
}

impl RuntimeServices {
    /// Marks the provider-reconstructible session cache dirty and starts the
    /// coalesced best-effort writer without waiting for filesystem I/O.
    pub fn schedule_registry_cache_save(&self) {
        self.registry_cache.dirty.store(true, Ordering::Release);
        if self
            .registry_cache
            .writer_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let services = self.clone();
        tokio::spawn(async move {
            services.run_registry_cache_writer().await;
        });
    }

    /// Marks an activity-only change (only `last_activity` moved) and arms a
    /// separate slow timer that hands it to the ordinary writer. The timer is
    /// its own task so a status change never waits behind it.
    pub(super) fn schedule_registry_activity_save(&self) {
        self.registry_cache
            .activity_dirty
            .store(true, Ordering::Release);
        if self
            .registry_cache
            .activity_timer_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let services = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(REGISTRY_ACTIVITY_SAVE_DEBOUNCE).await;
            services
                .registry_cache
                .activity_timer_running
                .store(false, Ordering::Release);
            if services
                .registry_cache
                .activity_dirty
                .swap(false, Ordering::AcqRel)
            {
                services.schedule_registry_cache_save();
            }
        });
    }

    async fn run_registry_cache_writer(self) {
        loop {
            tokio::select! {
                () = tokio::time::sleep(REGISTRY_SAVE_DEBOUNCE) => {}
                () = self.registry_cache.wake.notified() => {}
            }
            if self.registry_cache.dirty.swap(false, Ordering::AcqRel) {
                // The save below reads the registry after this, so it carries
                // every activity change made before it.
                self.registry_cache
                    .activity_dirty
                    .store(false, Ordering::Release);
                self.write_registry_cache("coalesced projection").await;
            }
            self.registry_cache
                .writer_running
                .store(false, Ordering::Release);
            self.registry_cache.idle.notify_waiters();
            if self.registry_cache.dirty.load(Ordering::Acquire)
                && self
                    .registry_cache
                    .writer_running
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                continue;
            }
            break;
        }
    }

    async fn write_registry_cache(&self, context: &'static str) {
        let sessions = Arc::clone(&self.sessions);
        if let Err(error) = blocking(move || {
            // Only the copy happens under the registry lock; serializing,
            // validating and the fsync'd write run after it is released. A
            // cache that already holds this revision needs no copy at all.
            let save = {
                let registry = lock(&sessions, "session registry")?;
                if registry.cache_is_current() {
                    return Ok(());
                }
                registry.prepare_save(registry.revision())
            };
            if let Err(error) = save.and_then(RegistrySave::write) {
                eprintln!("[runtime] registry cache save failed after {context}: {error}");
            }
            Ok(())
        })
        .await
        {
            eprintln!("[runtime] registry cache task failed after {context}: {error}");
        }
    }

    /// Writes any pending session cache change now and waits for the writer
    /// to go idle. The host calls it once more at quit, after the automatic
    /// worktree releases that redirect sessions home have drained.
    pub async fn flush_registry_cache(&self) {
        if self
            .registry_cache
            .activity_dirty
            .swap(false, Ordering::AcqRel)
        {
            self.schedule_registry_cache_save();
        }
        loop {
            let idle = self.registry_cache.idle.notified();
            if !self.registry_cache.dirty.load(Ordering::Acquire)
                && !self.registry_cache.writer_running.load(Ordering::Acquire)
            {
                return;
            }
            self.registry_cache.wake.notify_one();
            idle.await;
        }
    }
}
