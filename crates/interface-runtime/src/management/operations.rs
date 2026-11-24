//! Stored-session operations: forget, local and provider archive, rename,
//! delete, fork, adopt, send to a live session, and the operations each
//! engine supports.

use std::{
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
};

use interface_contracts::{
    engine::{EngineId, SessionOrigin, SessionStatus, SessionSummary, session_key},
    ipc::{ENGINE_OPS, EngineOp, EngineOpSupport},
};
use interface_engine::{DeliveryOutcome, EffectCertainty, EngineAdapter, ForkBoundary};

use crate::{
    RuntimeDelivery, RuntimeError, RuntimeErrorKind, SessionRuntime,
    runtime::{blocking, lock},
};

use super::{
    lifecycle::LifecycleIdentity, local_error, optional_cwd, provider_applied_local_error,
    provider_operation_error,
};

const MAX_TITLE_CHARS: usize = 80;

/// `sendLive` distinguishes a missing attachment from a provider delivery and
/// retains the provider's effect certainty for callers that need safe replay.
#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeSendLiveOutcome {
    NotLive,
    Delivery(RuntimeDelivery),
}

#[derive(Clone)]
struct OperationTarget {
    adapter: Arc<dyn EngineAdapter>,
    engine: EngineId,
    session_id: String,
    summary: SessionSummary,
}

impl SessionRuntime {
    /// Drops only Interface's projection and installs a durable re-list fence.
    ///
    /// # Errors
    ///
    /// Refuses live or concurrently-changing sessions and reports projection failures.
    pub async fn forget_session(&self, key: &str) -> Result<(), RuntimeError> {
        let work = async {
            if self.is_live(key).await {
                return Err(local_error(
                    RuntimeErrorKind::Lifecycle,
                    "That agent is running. Stop this session before closing it; nothing was removed.",
                    false,
                ));
            }
            self.registry_transaction({
                let key = key.to_owned();
                move |registry| {
                    let revision = registry.revision();
                    registry.forget(revision, &key).map(|_| ())
                }
            })
            .await
        };
        self.inner
            .management
            .coalesced_unit(key, "closing it", LifecycleIdentity::Forget, work)
            .await
    }

    /// Changes Interface's local-only archive flag. Hiding a live row is
    /// refused; restoring one is always local and reversible.
    ///
    /// # Errors
    ///
    /// Refuses invalid, live, or concurrently-changing sessions and reports projection failures.
    pub async fn archive_session(&self, key: &str, archived: bool) -> Result<(), RuntimeError> {
        let operation = if archived {
            "archiving it"
        } else {
            "restoring it"
        };
        let work = async {
            if archived && self.is_live(key).await {
                return Err(local_error(
                    RuntimeErrorKind::Lifecycle,
                    "That agent is running. Stop this session before hiding it.",
                    false,
                ));
            }
            self.registry_transaction({
                let key = key.to_owned();
                move |registry| {
                    let revision = registry.revision();
                    registry.mark_archived(revision, &key, archived).map(|_| ())
                }
            })
            .await
        };
        self.inner
            .management
            .coalesced_unit(
                key,
                operation,
                LifecycleIdentity::LocalArchive(archived),
                work,
            )
            .await
    }

    /// Renames in the provider store first and applies the local title override
    /// second. Cache persistence is coalesced and best-effort.
    ///
    /// # Errors
    ///
    /// Returns validation, capability, provider, lifecycle, or local-projection failures.
    pub async fn rename_stored_session(
        &self,
        key: &str,
        title: String,
    ) -> Result<(), RuntimeError> {
        let work = async {
            let target = self.operation_target(key, EngineOp::RenameSession).await?;
            let clean = title
                .trim()
                .chars()
                .take(MAX_TITLE_CHARS)
                .collect::<String>();
            if clean.is_empty() {
                return Err(local_error(
                    RuntimeErrorKind::InvalidInput,
                    "That name is empty. Nothing was changed.",
                    false,
                ));
            }
            target
                .adapter
                .rename_session(
                    target.session_id,
                    clean.clone(),
                    optional_cwd(&target.summary),
                )
                .await
                .map_err(|error| {
                    provider_operation_error(
                        &format!("rename that {} session", target.engine),
                        &error,
                    )
                })?;
            self.registry_after_provider("rename", {
                let key = key.to_owned();
                move |registry| {
                    let revision = registry.revision();
                    registry.set_title(revision, &key, clean).map(|_| ())
                }
            })
            .await
        };
        self.inner
            .management
            .coalesced_unit(
                key,
                "renaming it",
                LifecycleIdentity::Rename(title.clone()),
                work,
            )
            .await
    }

    /// Deletes a cold provider transcript and then removes its durable projection.
    ///
    /// # Errors
    ///
    /// Returns capability, provider, lifecycle, or local-projection failures.
    pub async fn delete_stored_session(&self, key: &str) -> Result<(), RuntimeError> {
        let work = async {
            let target = self.operation_target(key, EngineOp::DeleteSession).await?;
            if self.is_live(key).await {
                return Err(local_error(
                    RuntimeErrorKind::Lifecycle,
                    "That agent is running. Interrupt it before deleting its stored transcript.",
                    false,
                ));
            }
            target
                .adapter
                .delete_session(target.session_id, optional_cwd(&target.summary))
                .await
                .map_err(|error| {
                    provider_operation_error(
                        &format!("delete that {} session", target.engine),
                        &error,
                    )
                })?;
            let generation = self
                .inner
                .management
                .listing_generation
                .load(Ordering::Acquire);
            self.inner
                .management
                .deleted_through_listing
                .lock()
                .map_err(|_| {
                    provider_applied_local_error(
                        "delete",
                        &local_error(
                            RuntimeErrorKind::Internal,
                            "listing deletion fence mutex is poisoned",
                            false,
                        ),
                    )
                })?
                .insert(key.to_owned(), generation);
            self.registry_after_provider("delete", {
                let key = key.to_owned();
                move |registry| {
                    let revision = registry.revision();
                    registry.remove(revision, &key).map(|_| ())
                }
            })
            .await
        };
        self.inner
            .management
            .coalesced_unit(key, "deleting it", LifecycleIdentity::Delete, work)
            .await
    }

    /// Forks in the provider store and registers the exact returned provider identifier.
    ///
    /// # Errors
    ///
    /// Returns capability, provider, collision, lifecycle, or local-projection failures.
    pub async fn fork_stored_session(&self, key: &str) -> Result<String, RuntimeError> {
        self.fork_stored(key, None).await
    }

    /// Forks `engine#session_id` keeping its transcript through the last
    /// answer to the owner prompt `boundary` names, then registers the copy
    /// exactly as [`Self::fork_stored_session`] does. Providers that cannot
    /// cut a transcript at a turn, or cannot find the prompt, refuse
    /// definitely-unsent.
    ///
    /// Only sessions in a managed (ledger) worktree are refused, as for a
    /// whole fork: a thread worktree is the session's plain `cwd` and the copy
    /// shares it. A boundary that is not a prompt digest is refused here,
    /// before any provider call.
    ///
    /// # Errors
    ///
    /// Returns capability, provider, collision, lifecycle, or local-projection failures.
    pub async fn fork_session_at(
        &self,
        engine: &EngineId,
        session_id: &str,
        boundary: ForkBoundary,
    ) -> Result<String, RuntimeError> {
        if boundary.prompt_digest.len() != 16
            || !boundary
                .prompt_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(local_error(
                RuntimeErrorKind::InvalidInput,
                "A fork names the prompt it keeps by its digest; refresh the thread and try again.",
                false,
            ));
        }
        self.fork_stored(&session_key(engine, session_id), Some(boundary))
            .await
    }

    /// `boundary: None` copies the whole session.
    #[allow(clippy::too_many_lines)] // The provider fork, collision check and registration are one effect.
    async fn fork_stored(
        &self,
        key: &str,
        boundary: Option<ForkBoundary>,
    ) -> Result<String, RuntimeError> {
        let _lifecycle = self.inner.management.begin(key, "forking it")?;
        if self.inner.management.importing.load(Ordering::Acquire) {
            return Err(local_error(
                RuntimeErrorKind::Lifecycle,
                "An import is running. Wait for it to finish before forking a session.",
                true,
            ));
        }
        let target = self.operation_target(key, EngineOp::ForkSession).await?;
        if target.summary.worktree_home.is_some()
            && target.summary.cwd != target.summary.worktree_home.as_deref().unwrap_or_default()
        {
            return Err(local_error(
                RuntimeErrorKind::Lifecycle,
                "Finish isolation before forking this session.",
                false,
            ));
        }
        let cwd = optional_cwd(&target.summary);
        let forked = match boundary {
            None => target.adapter.fork_session(target.session_id, cwd).await,
            Some(boundary) => {
                target
                    .adapter
                    .fork_session_at(target.session_id, cwd, boundary)
                    .await
            }
        };
        let new_id = forked.map_err(|error| {
            provider_operation_error(&format!("fork that {} session", target.engine), &error)
        })?;
        if new_id.is_empty() {
            return Err(RuntimeError::new(
                RuntimeErrorKind::Provider,
                "The provider copied that session but did not identify the copy. Refresh before retrying.",
                false,
                EffectCertainty::OutcomeUnknown,
            ));
        }
        let new_key = session_key(&target.engine, &new_id);
        if self.is_live(&new_key).await
            || self.inner.management.is_changing(&new_key)
            || self.inner.management.importing.load(Ordering::Acquire)
            || self
                .require_summary_optional(&new_key)
                .await
                .map_err(|error| provider_applied_local_error("fork", &error))?
                .is_some()
        {
            return Err(RuntimeError::new(
                RuntimeErrorKind::Lifecycle,
                format!(
                    "The provider reused {new_key}, which Interface already controls. The fork may exist; refresh after lifecycle work settles."
                ),
                false,
                EffectCertainty::OutcomeUnknown,
            ));
        }
        let now = self.inner.clock.now();
        let mut fork = target.summary.clone();
        fork.session_id.clone_from(&new_id);
        fork.status = SessionStatus::Idle;
        fork.last_activity.clone_from(&now);
        fork.first_seen = now;
        fork.archived = false;
        fork.size = None;
        fork.origin = Some(SessionOrigin::App);
        fork.worktree_home = None;
        fork.worktree_path = None;
        fork.kind = target.summary.kind;
        let fork_title = fork.title.clone();
        let engine = target.engine.clone();
        let commit_key = new_key.clone();
        let project_roots = Arc::clone(&self.inner.project_roots);
        let projection = Arc::clone(&project_roots);
        let unresolved = self
            .registry_after_provider("fork", move |registry| {
                let mut forks = [fork];
                let unresolved = projection.prepare(registry, &engine, &mut forks);
                let revision = registry.revision();
                let revision = registry.remember(revision, &commit_key)?;
                let revision = registry.upsert_all(revision, &engine, forks)?;
                registry.set_title(revision, &commit_key, fork_title)?;
                Ok(unresolved)
            })
            .await?;
        project_roots.schedule(&self.inner.services, unresolved);
        if target
            .adapter
            .supported_ops()
            .contains(&EngineOp::AdoptSession)
        {
            let adapter = Arc::clone(&target.adapter);
            let cwd = optional_cwd(&target.summary);
            let adopt_id = new_id.clone();
            tokio::spawn(async move {
                let _ = adapter.adopt_session(adopt_id, true, cwd).await;
            });
        }
        Ok(new_key)
    }

    /// Changes provider adoption metadata before committing the local origin.
    ///
    /// # Errors
    ///
    /// Returns capability, provider, lifecycle, or local-projection failures.
    pub async fn adopt_stored_session(&self, key: &str, adopt: bool) -> Result<(), RuntimeError> {
        let work = async {
            let target = self.operation_target(key, EngineOp::AdoptSession).await?;
            target
                .adapter
                .adopt_session(target.session_id, adopt, optional_cwd(&target.summary))
                .await
                .map_err(|error| {
                    provider_operation_error(
                        &format!(
                            "{} that {} session",
                            if adopt { "import" } else { "release" },
                            target.engine
                        ),
                        &error,
                    )
                })?;
            self.registry_after_provider("adopt", {
                let key = key.to_owned();
                move |registry| {
                    let revision = registry.revision();
                    registry
                        .set_origin(
                            revision,
                            &key,
                            if adopt {
                                SessionOrigin::App
                            } else {
                                SessionOrigin::Store
                            },
                        )
                        .map(|_| ())
                }
            })
            .await
        };
        self.inner
            .management
            .coalesced_unit(
                key,
                "changing its import metadata",
                LifecycleIdentity::Adopt(adopt),
                work,
            )
            .await
    }

    /// Changes provider archive state before mirroring the local archive flag.
    ///
    /// # Errors
    ///
    /// Returns capability, provider, lifecycle, or local-projection failures.
    pub async fn set_engine_archived(&self, key: &str, archived: bool) -> Result<(), RuntimeError> {
        let operation = if archived {
            "archiving it"
        } else {
            "restoring it"
        };
        let work = async {
            let target = self.operation_target(key, EngineOp::SetArchived).await?;
            if self.is_live(key).await {
                return Err(local_error(
                    RuntimeErrorKind::Lifecycle,
                    "That agent is running. Stop it before changing provider archive state.",
                    false,
                ));
            }
            target
                .adapter
                .set_archived(target.session_id, archived, optional_cwd(&target.summary))
                .await
                .map_err(|error| {
                    provider_operation_error(
                        &format!("archive that {} session", target.engine),
                        &error,
                    )
                })?;
            self.registry_after_provider("provider archive", {
                let key = key.to_owned();
                move |registry| {
                    let revision = registry.revision();
                    registry.mark_archived(revision, &key, archived).map(|_| ())
                }
            })
            .await
        };
        self.inner
            .management
            .coalesced_unit(
                key,
                operation,
                LifecycleIdentity::ProviderArchive(archived),
                work,
            )
            .await
    }

    #[must_use]
    pub fn engine_ops(&self) -> Vec<EngineOpSupport> {
        let mut supports = self
            .inner
            .adapters
            .iter()
            .map(|(engine, adapter)| {
                let advertised = adapter.supported_ops().into_iter().collect::<HashSet<_>>();
                EngineOpSupport {
                    engine: engine.clone(),
                    ops: ENGINE_OPS
                        .iter()
                        .copied()
                        .filter(|operation| advertised.contains(operation))
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        supports.sort_by(|left, right| left.engine.as_str().cmp(right.engine.as_str()));
        supports
    }

    /// Sends only through an already-live attachment. No cold resume occurs.
    pub async fn send_live(&self, key: &str, text: String) -> RuntimeSendLiveOutcome {
        let Ok(_lifecycle) = self.inner.management.begin(key, "sending to it") else {
            return RuntimeSendLiveOutcome::Delivery(RuntimeDelivery {
                outcome: DeliveryOutcome::DefinitelyUnsent(
                    interface_engine::EngineError::definitely_unsent(
                        interface_engine::EngineErrorKind::Lifecycle,
                        "That session is already changing.",
                        true,
                    ),
                ),
                audit: crate::RuntimeDeliveryAudit::NotApplicable,
            });
        };
        if !self.is_live(key).await {
            return RuntimeSendLiveOutcome::NotLive;
        }
        RuntimeSendLiveOutcome::Delivery(self.send(key, text).await)
    }

    async fn operation_target(
        &self,
        key: &str,
        operation: EngineOp,
    ) -> Result<OperationTarget, RuntimeError> {
        let summary = self.require_summary(key).await?;
        if session_key(&summary.engine, &summary.session_id) != key {
            return Err(local_error(
                RuntimeErrorKind::InvalidInput,
                "Interface could not resolve that session key; nothing was changed.",
                false,
            ));
        }
        let adapter = self.adapter(&summary.engine)?;
        if !adapter.supported_ops().contains(&operation) {
            return Err(local_error(
                RuntimeErrorKind::Provider,
                format!("{} cannot {}.", summary.engine, operation_name(operation)),
                false,
            ));
        }
        Ok(OperationTarget {
            adapter,
            engine: summary.engine.clone(),
            session_id: summary.session_id.clone(),
            summary,
        })
    }

    async fn require_summary(&self, key: &str) -> Result<SessionSummary, RuntimeError> {
        self.require_summary_optional(key).await?.ok_or_else(|| {
            local_error(
                RuntimeErrorKind::InvalidInput,
                "Interface has no record of that session. Refresh and try again.",
                false,
            )
        })
    }

    async fn require_summary_optional(
        &self,
        key: &str,
    ) -> Result<Option<SessionSummary>, RuntimeError> {
        let sessions = self.inner.services.session_registry();
        let key = key.to_owned();
        blocking(move || Ok(lock(&sessions, "session registry")?.get(&key).cloned())).await
    }

    async fn is_live(&self, key: &str) -> bool {
        self.inner.live.read().await.contains_key(key)
    }
}

fn operation_name(operation: EngineOp) -> &'static str {
    match operation {
        EngineOp::RenameSession => "rename a session",
        EngineOp::DeleteSession => "delete a session",
        EngineOp::ForkSession => "fork a session",
        EngineOp::SetArchived => "archive a session",
        EngineOp::AdoptSession => "change session adoption",
        EngineOp::ReadUsage => "read usage",
    }
}
