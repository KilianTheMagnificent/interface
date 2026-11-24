//! Provider listing refresh: asking every provider for its sessions, merging
//! each answer into the registry through the deletion and live-status fences,
//! and the per-provider freshness the snapshot reports.

use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use interface_contracts::{
    engine::{EngineId, EngineSessionListing, SessionSummary, session_key},
    ipc::{ProviderListingFreshness, ProviderListingStatus, SessionListingSnapshot},
};
use tokio::{task::JoinSet, time::Instant};

use crate::{
    RuntimeError, RuntimeErrorKind, SessionRuntime,
    runtime::{blocking, engine_missing, lock},
};

use super::{SessionManagementState, local_error};

/// Codex lists a large session store in ~15 s; it stops paging at 30 s itself.
const PROVIDER_LISTING_TIMEOUT: Duration = Duration::from_secs(45);

/// A provider's last complete listing, already merged into the registry.
pub(super) struct KeptListing {
    began: Instant,
    /// `listing_restarts` when it began: an explicit refresh since voids it.
    restarts: u64,
}

impl SessionManagementState {
    /// The explicit refresh: every kept listing stops counting, so the next
    /// refresh asks each provider again.
    pub(crate) fn restart_listings(&self) {
        self.listing_restarts.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether a refresh that began at `restarts` may skip `engine`: its last
    /// complete listing began under the same count and within `reuse`.
    fn keeps_listing(&self, engine: &EngineId, reuse: Duration, restarts: u64) -> bool {
        !reuse.is_zero()
            && interface_common::sync::lock(&self.kept_listings)
                .get(engine)
                .is_some_and(|kept| kept.restarts == restarts && kept.began.elapsed() < reuse)
    }

    /// Keeps a complete, available, error-free listing; anything else drops
    /// the kept one, so the next refresh asks the provider again.
    fn keep_listing(
        &self,
        engine: &EngineId,
        listing: &EngineSessionListing,
        began: Instant,
        restarts: u64,
    ) {
        let mut kept = interface_common::sync::lock(&self.kept_listings);
        if listing.complete && listing.unavailable != Some(true) && listing.error.is_none() {
            kept.insert(engine.clone(), KeptListing { began, restarts });
        } else {
            kept.remove(engine);
        }
    }

    fn forget_listing(&self, engine: &EngineId) {
        interface_common::sync::lock(&self.kept_listings).remove(engine);
    }
}

impl SessionRuntime {
    /// Refreshes every registered provider store concurrently, merges every
    /// successful answer independently, and returns the durable visible view.
    /// Provider failures are isolated in [`SessionRuntime::list_session_snapshot`].
    ///
    /// # Errors
    ///
    /// Returns a registry-projection access failure.
    pub async fn list_managed_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        self.refresh_all_provider_sessions().await;
        self.registry_visible().await
    }

    /// Returns the visible local projection without starting provider discovery.
    /// Callers must present this as cached state and refresh independently.
    ///
    /// # Errors
    ///
    /// Returns a registry-projection access failure.
    pub async fn cached_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        self.registry_visible().await
    }

    /// Returns visible sessions plus exact per-provider freshness/completeness.
    ///
    /// # Errors
    ///
    /// Returns a registry-projection access failure.
    pub async fn list_session_snapshot(&self) -> Result<SessionListingSnapshot, RuntimeError> {
        self.refresh_all_provider_sessions().await;
        let sessions = self.registry_visible().await?;
        let mut providers = self
            .inner
            .management
            .listing_status
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| left.engine.as_str().cmp(right.engine.as_str()));
        Ok(SessionListingSnapshot {
            sessions,
            providers,
        })
    }

    /// Returns locally archived rows newest-first without contacting providers.
    ///
    /// # Errors
    ///
    /// Returns a registry-projection access failure.
    pub async fn list_archived_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        let mut sessions = self.registry_all().await?;
        sessions.retain(|summary| summary.archived);
        sessions.sort_by(|left, right| right.last_activity.cmp(&left.last_activity));
        Ok(sessions)
    }

    pub(super) async fn refresh_all_provider_sessions(&self) {
        if self.ensure_accepting().is_err() {
            return;
        }
        let management = &self.inner.management;
        let observed = management.completed_refresh.load(Ordering::Acquire);
        let asked = management.listing_restarts.load(Ordering::Acquire);
        let _refresh = management.refresh.lock().await;
        // A refresh that completed while this one waited answers it too,
        // unless an explicit refresh came between that one's start and this ask.
        if management.completed_refresh.load(Ordering::Acquire) != observed
            && management
                .completed_refresh_restarts
                .load(Ordering::Acquire)
                >= asked
        {
            return;
        }
        let restarts = management.listing_restarts.load(Ordering::Acquire);
        let began = Instant::now();
        let generation = management
            .listing_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let mut work = JoinSet::new();
        for (engine, adapter) in &self.inner.adapters {
            // Its last complete listing is recent enough: those rows are in
            // the registry already and its status still describes them.
            if management.keeps_listing(engine, adapter.listing_reuse(), restarts) {
                continue;
            }
            let engine = engine.clone();
            let adapter = Arc::clone(adapter);
            work.spawn(async move {
                let listing =
                    tokio::time::timeout(PROVIDER_LISTING_TIMEOUT, adapter.list_sessions()).await;
                (engine, listing)
            });
        }
        while let Some(result) = work.join_next().await {
            let Ok((engine, listing)) = result else {
                continue;
            };
            match listing {
                Ok(Ok(listing)) => {
                    if self
                        .ingest_listing(&engine, &listing, generation)
                        .await
                        .is_err()
                    {
                        self.mark_listing_stale(
                            &engine,
                            "Interface could not persist this provider's session list.".into(),
                        )
                        .await;
                        continue;
                    }
                    management.keep_listing(&engine, &listing, began, restarts);
                    let status = ProviderListingStatus {
                        engine: engine.clone(),
                        status: if listing.unavailable == Some(true) {
                            ProviderListingFreshness::Unavailable
                        } else {
                            ProviderListingFreshness::Fresh
                        },
                        complete: listing.complete,
                        refreshed_at: Some(self.inner.clock.now()),
                        error: listing.error.clone(),
                    };
                    management
                        .listing_status
                        .write()
                        .await
                        .insert(engine, status);
                }
                Ok(Err(_)) | Err(_) => {
                    self.mark_listing_stale(
                        &engine,
                        "Interface could not refresh this provider's session list.".into(),
                    )
                    .await;
                }
            }
        }
        management
            .completed_refresh_restarts
            .store(restarts, Ordering::Release);
        management.completed_refresh.fetch_add(1, Ordering::Release);
    }

    /// Queries one provider and merges its listing into the canonical registry
    /// through the same fences as a full refresh: the listing timeout, the
    /// deletion fence, live status kept over a cold snapshot, and pruning on a
    /// complete listing.
    ///
    /// # Errors
    ///
    /// Preserves provider failures and reports local projection failures.
    pub async fn refresh_provider_sessions(
        &self,
        engine: &EngineId,
    ) -> Result<EngineSessionListing, RuntimeError> {
        self.ensure_accepting()?;
        let adapter = self
            .inner
            .adapters
            .get(engine)
            .ok_or_else(|| engine_missing(engine))?;
        let management = &self.inner.management;
        let restarts = management.listing_restarts.load(Ordering::Acquire);
        let began = Instant::now();
        // Taken before the provider is asked, so a delete that completes while
        // it lists still fences the rows it returns.
        let generation = management
            .listing_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        let listing =
            match tokio::time::timeout(PROVIDER_LISTING_TIMEOUT, adapter.list_sessions()).await {
                Ok(Ok(listing)) => listing,
                Ok(Err(error)) => {
                    management.forget_listing(engine);
                    return Err(RuntimeError::from_engine(&error));
                }
                Err(_) => {
                    management.forget_listing(engine);
                    return Err(RuntimeError::definitely_unsent(
                        RuntimeErrorKind::Provider,
                        "Interface could not refresh this provider's session list.",
                        true,
                    ));
                }
            };
        if let Err(error) = self.ingest_listing(engine, &listing, generation).await {
            management.forget_listing(engine);
            return Err(error);
        }
        management.keep_listing(engine, &listing, began, restarts);
        Ok(listing)
    }

    async fn ingest_listing(
        &self,
        engine: &EngineId,
        listing: &EngineSessionListing,
        generation: u64,
    ) -> Result<(), RuntimeError> {
        let mut admitted = Vec::with_capacity(listing.sessions.len());
        let mut admitted_keys = Vec::with_capacity(listing.sessions.len());
        {
            let mut deleted = self
                .inner
                .management
                .deleted_through_listing
                .lock()
                .map_err(|_| {
                    local_error(
                        RuntimeErrorKind::Internal,
                        "listing deletion fence mutex is poisoned",
                        false,
                    )
                })?;
            for summary in &listing.sessions {
                let key = session_key(engine, &summary.session_id);
                match deleted.get(&key).copied() {
                    Some(through) if generation <= through => {}
                    Some(_) => {
                        deleted.remove(&key);
                        admitted_keys.push(key);
                        admitted.push(summary.clone());
                    }
                    None => {
                        admitted_keys.push(key);
                        admitted.push(summary.clone());
                    }
                }
            }
        }
        let live = self
            .inner
            .live
            .read()
            .await
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let present = admitted_keys.into_iter().collect::<HashSet<_>>();
        // A session the provider can no longer list but keeps as history (a
        // removed provider account's, protocol 60) is not stale.
        let adapter = self.inner.adapters.get(engine).cloned();
        let engine = engine.clone();
        let complete = listing.complete;
        let project_roots = Arc::clone(&self.inner.project_roots);
        let projection = Arc::clone(&project_roots);
        let unresolved = self
            .registry_transaction(move |registry| {
                let mut admitted = admitted
                    .into_iter()
                    .map(|mut summary| {
                        let key = session_key(&engine, &summary.session_id);
                        if live.contains(&key)
                            && let Some(current) = registry.get(&key)
                        {
                            // A cold provider snapshot cannot demote a live,
                            // approval-blocked session. Codex can report the
                            // transcript merely `working` while Interface has an
                            // exact live `waiting` state.
                            summary.status = current.status;
                        }
                        summary
                    })
                    .collect::<Vec<_>>();
                let unresolved = projection.prepare(registry, &engine, &mut admitted);
                let mut revision = registry.revision();
                revision = registry.upsert_all(revision, &engine, admitted)?;
                if complete {
                    let stale = registry
                        .summaries()
                        .filter(|summary| summary.engine == engine)
                        .map(|summary| {
                            (
                                session_key(&engine, &summary.session_id),
                                summary.session_id.as_str(),
                            )
                        })
                        .filter(|(key, _)| !present.contains(key) && !live.contains(key))
                        .filter(|(_, session_id)| {
                            !adapter
                                .as_ref()
                                .is_some_and(|adapter| adapter.keeps_unlisted_session(session_id))
                        })
                        .map(|(key, _)| key)
                        .collect::<HashSet<_>>();
                    registry.remove_many(revision, &stale)?;
                }
                Ok(unresolved)
            })
            .await?;
        project_roots.schedule(&self.inner.services, unresolved);
        Ok(())
    }

    /// Records a failed refresh; the provider's kept listing, if any, goes too.
    async fn mark_listing_stale(&self, engine: &EngineId, error: String) {
        self.inner.management.forget_listing(engine);
        let previous = self
            .inner
            .management
            .listing_status
            .read()
            .await
            .get(engine)
            .and_then(|status| status.refreshed_at.clone());
        self.inner.management.listing_status.write().await.insert(
            engine.clone(),
            ProviderListingStatus {
                engine: engine.clone(),
                status: ProviderListingFreshness::Stale,
                complete: false,
                refreshed_at: previous,
                error: Some(error),
            },
        );
    }

    async fn registry_visible(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        let sessions = self.inner.services.session_registry();
        blocking(move || Ok(lock(&sessions, "session registry")?.list())).await
    }
}
