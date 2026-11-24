//! Plan usage: one normalized row per provider that meters a plan.

use std::{sync::Arc, time::Duration};

use interface_contracts::{
    engine::{EngineId, EngineUsage},
    ipc::EngineOp,
};
use tokio::task::JoinSet;

use crate::SessionRuntime;

use super::public_provider_reason;

const PROVIDER_USAGE_TIMEOUT: Duration = Duration::from_secs(15);

impl SessionRuntime {
    /// Reads one normalized row per registered provider; no provider failure
    /// prevents another provider's usage from being returned.
    pub async fn read_all_usage(&self) -> Vec<EngineUsage> {
        if self.ensure_accepting().is_err() {
            return self
                .registered_engines()
                .into_iter()
                .map(|engine| unavailable_usage(engine, "Interface is closing.".into()))
                .collect();
        }
        let mut work = JoinSet::new();
        for (engine, adapter) in &self.inner.adapters {
            // An engine with no plan to meter gets no row at all. A permanent
            // "does not report plan usage" line is not a budget signal, it is
            // furniture the reader has to skip past every time the panel opens.
            if !adapter.supported_ops().contains(&EngineOp::ReadUsage) {
                continue;
            }
            let engine = engine.clone();
            let adapter = Arc::clone(adapter);
            work.spawn(async move {
                // Protocol 60: one row per provider account; a provider with
                // no added accounts answers exactly its one row.
                match tokio::time::timeout(PROVIDER_USAGE_TIMEOUT, adapter.read_usage_rows()).await
                {
                    Ok(rows) => rows
                        .into_iter()
                        .map(|row| match row {
                            Ok(usage) => normalize_usage(engine.clone(), usage),
                            Err(error) => {
                                let why = public_provider_reason(&error.message);
                                unavailable_usage(
                                    engine.clone(),
                                    format!(
                                        "Interface could not read usage from the {engine} engine{}.",
                                        if why.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" ({why})")
                                        }
                                    ),
                                )
                            }
                        })
                        .collect::<Vec<_>>(),
                    Err(_) => vec![unavailable_usage(
                        engine.clone(),
                        format!(
                            "Interface could not read usage from the {engine} engine (timed out)."
                        ),
                    )],
                }
            });
        }
        let mut usage = Vec::new();
        while let Some(result) = work.join_next().await {
            if let Ok(rows) = result {
                usage.extend(rows);
            }
        }
        usage.sort_by(|left, right| left.engine.as_str().cmp(right.engine.as_str()));
        usage
    }
}

fn normalize_usage(engine: EngineId, mut usage: EngineUsage) -> EngineUsage {
    usage.engine = engine;
    if usage.available {
        for window in &mut usage.windows {
            window.utilization = window
                .utilization
                .filter(|value| value.is_finite())
                .map(|value| value.clamp(0.0, 100.0));
        }
    } else {
        usage.windows.clear();
    }
    usage.cost_usd = usage.cost_usd.filter(|value| value.is_finite());
    usage
}

fn unavailable_usage(engine: EngineId, note: String) -> EngineUsage {
    EngineUsage {
        engine,
        available: false,
        plan: None,
        windows: Vec::new(),
        cost_usd: None,
        note: Some(note),
        account: None,
        account_label: None,
    }
}
