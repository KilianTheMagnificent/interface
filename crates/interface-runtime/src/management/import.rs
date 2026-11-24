//! Bulk adoption: applying provider adoption metadata to every listed session
//! with bounded workers, progress and a partial-success report.

use std::sync::{Arc, atomic::Ordering};

use interface_contracts::{
    engine::{SessionOrigin, SessionSummary, session_key},
    ipc::EngineOp,
};
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use crate::{RuntimeError, RuntimeErrorKind, SessionRuntime};

use super::{
    IMPORT_CONCURRENCY, SessionManagementState, local_error, optional_cwd,
    provider_operation_error, public_provider_reason,
};

const MAX_IMPORT_ERRORS: usize = 5;

/// Progress emitted by the bounded all-session adoption operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportProgress {
    pub adopt: bool,
    pub done: u64,
    pub total: u64,
    pub ok: u64,
    pub failed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Exact partial-success report returned by all-session adoption.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportReport {
    pub adopt: bool,
    pub total: u64,
    pub ok: u64,
    pub local: u64,
    pub failed: u64,
    pub errors: Vec<String>,
}

/// Transport-independent progress callback. Implementations must be quick and
/// must not call back into the same import operation synchronously.
pub trait ImportProgressSink: Send + Sync {
    fn emit(&self, progress: ImportProgress);
}

impl<F> ImportProgressSink for F
where
    F: Fn(ImportProgress) + Send + Sync,
{
    fn emit(&self, progress: ImportProgress) {
        self(progress);
    }
}

struct ImportGuard(Arc<SessionManagementState>);

impl ImportGuard {
    fn acquire(state: &Arc<SessionManagementState>) -> Result<Self, RuntimeError> {
        state
            .importing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                local_error(
                    RuntimeErrorKind::Lifecycle,
                    "An import is running. Wait for it to finish before starting another.",
                    true,
                )
            })?;
        Ok(Self(Arc::clone(state)))
    }
}

impl Drop for ImportGuard {
    fn drop(&mut self) {
        self.0.importing.store(false, Ordering::Release);
    }
}

impl SessionRuntime {
    /// Applies provider adoption metadata to the complete freshly-listed
    /// corpus with four workers and partial-success progress/reporting.
    ///
    /// # Errors
    ///
    /// Refuses concurrent imports and reports durable listing/registry failures.
    pub async fn adopt_all(
        &self,
        adopt: bool,
        progress: Option<Arc<dyn ImportProgressSink>>,
    ) -> Result<ImportReport, RuntimeError> {
        let _import = ImportGuard::acquire(&self.inner.management)?;
        self.refresh_all_provider_sessions().await;
        let targets = self.registry_all().await?;
        let mut report = ImportReport {
            adopt,
            total: u64::try_from(targets.len()).unwrap_or(u64::MAX),
            ok: 0,
            local: 0,
            failed: 0,
            errors: Vec::new(),
        };
        emit_import_progress(progress.as_deref(), &report, 0, None);
        let mut targets = targets.into_iter();
        let mut work = JoinSet::new();
        for summary in targets.by_ref().take(IMPORT_CONCURRENCY) {
            spawn_import_task(&mut work, self.clone(), summary, adopt);
        }
        let mut done = 0_u64;
        while let Some(joined) = work.join_next().await {
            match joined {
                Ok((key, result)) => {
                    match result {
                        Ok(ImportOne::Provider) => report.ok = report.ok.saturating_add(1),
                        Ok(ImportOne::Local) => report.local = report.local.saturating_add(1),
                        Err(error) => {
                            report.failed = report.failed.saturating_add(1);
                            if report.errors.len() < MAX_IMPORT_ERRORS {
                                let why = public_provider_reason(&error.message);
                                report.errors.push(if why.is_empty() {
                                    key.clone()
                                } else {
                                    format!("{key}: {why}")
                                });
                            }
                        }
                    }
                    done = done.saturating_add(1);
                    emit_import_progress(progress.as_deref(), &report, done, Some(key));
                }
                Err(error) => {
                    report.failed = report.failed.saturating_add(1);
                    if report.errors.len() < MAX_IMPORT_ERRORS {
                        report.errors.push(format!("import worker failed: {error}"));
                    }
                    done = done.saturating_add(1);
                    emit_import_progress(progress.as_deref(), &report, done, None);
                }
            }
            if let Some(summary) = targets.next() {
                spawn_import_task(&mut work, self.clone(), summary, adopt);
            }
        }
        Ok(report)
    }

    async fn import_one(
        &self,
        key: String,
        summary: SessionSummary,
        adopt: bool,
    ) -> Result<ImportOne, RuntimeError> {
        let _lifecycle = self
            .inner
            .management
            .begin(&key, "changing its import metadata")?;
        let Some(adapter) = self.inner.adapters.get(&summary.engine).cloned() else {
            if adopt {
                return Err(local_error(
                    RuntimeErrorKind::EngineNotRegistered,
                    format!("no {} engine in this build", summary.engine),
                    false,
                ));
            }
            self.set_origin_local(&key, SessionOrigin::Store).await?;
            return Ok(ImportOne::Local);
        };
        if !adapter.supported_ops().contains(&EngineOp::AdoptSession) {
            self.set_origin_local(
                &key,
                if adopt {
                    SessionOrigin::App
                } else {
                    SessionOrigin::Store
                },
            )
            .await?;
            return Ok(ImportOne::Local);
        }
        adapter
            .adopt_session(summary.session_id.clone(), adopt, optional_cwd(&summary))
            .await
            .map_err(|error| {
                provider_operation_error(
                    &format!(
                        "{} that {} session",
                        if adopt { "import" } else { "release" },
                        summary.engine
                    ),
                    &error,
                )
            })?;
        self.registry_after_provider("adopt", move |registry| {
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
        })
        .await?;
        Ok(ImportOne::Provider)
    }

    async fn set_origin_local(&self, key: &str, origin: SessionOrigin) -> Result<(), RuntimeError> {
        self.registry_transaction({
            let key = key.to_owned();
            move |registry| {
                let revision = registry.revision();
                registry.set_origin(revision, &key, origin).map(|_| ())
            }
        })
        .await
    }
}

#[derive(Clone, Copy)]
enum ImportOne {
    Provider,
    Local,
}

fn spawn_import_task(
    work: &mut JoinSet<(String, Result<ImportOne, RuntimeError>)>,
    runtime: SessionRuntime,
    summary: SessionSummary,
    adopt: bool,
) {
    work.spawn(async move {
        let key = session_key(&summary.engine, &summary.session_id);
        let result = runtime.import_one(key.clone(), summary, adopt).await;
        (key, result)
    });
}

fn emit_import_progress(
    sink: Option<&dyn ImportProgressSink>,
    report: &ImportReport,
    done: u64,
    key: Option<String>,
) {
    if let Some(sink) = sink {
        sink.emit(ImportProgress {
            adopt: report.adopt,
            done,
            total: report.total,
            ok: report.ok,
            failed: report.failed,
            key,
        });
    }
}
