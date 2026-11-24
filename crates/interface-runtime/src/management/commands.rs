//! Provider command discovery for one launch scope: scope admission, the
//! coalesced per-scope lease, and the bounded per-engine and global slots.

use std::{collections::HashSet, os::unix::fs::MetadataExt, path::Path, sync::Arc, time::Duration};

use interface_common::paths::lexically_absolute;
use interface_contracts::engine::{EngineId, ProviderCommandCatalogue, ProviderCommandScope};
use interface_engine::ProviderCommandRequest;

use crate::{
    RuntimeError, RuntimeErrorKind, SessionRuntime,
    runtime::{blocking, engine_missing},
};

use super::{
    SessionManagementState, SettleOnce, ellipsize, exceeds_javascript_string_limit, local_error,
    remove_entry_if_current,
};

const PROVIDER_COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

pub(super) const MAX_PROVIDER_COMMAND_DISCOVERIES: usize = 4;

impl SessionManagementState {
    fn command_lease(
        self: &Arc<Self>,
        scope: &ProviderCommandScope,
    ) -> Result<CommandDiscoveryLease, RuntimeError> {
        let key = CommandDiscoveryKey::from(scope);
        let mut discoveries = self.command_discoveries.lock().map_err(|_| {
            local_error(
                RuntimeErrorKind::Internal,
                "provider command discovery mutex is poisoned",
                false,
            )
        })?;
        if let Some(entry) = discoveries.get(&key) {
            return Ok(CommandDiscoveryLease::Waiter(Arc::clone(entry)));
        }
        let entry = Arc::new(CommandDiscoveryEntry::new());
        discoveries.insert(key.clone(), Arc::clone(&entry));
        Ok(CommandDiscoveryLease::Owner(CommandDiscoveryOwner {
            state: Arc::clone(self),
            key,
            entry,
            complete: false,
        }))
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct CommandDiscoveryKey {
    engine: EngineId,
    cwd: String,
    model: Option<String>,
    setting_sources: Option<Vec<String>>,
}

impl From<&ProviderCommandScope> for CommandDiscoveryKey {
    fn from(scope: &ProviderCommandScope) -> Self {
        Self {
            engine: scope.engine.clone(),
            cwd: scope.cwd.clone(),
            model: scope.model.clone(),
            setting_sources: scope.setting_sources.clone(),
        }
    }
}

pub(super) type CommandDiscoveryEntry = SettleOnce<Result<ProviderCommandCatalogue, RuntimeError>>;

enum CommandDiscoveryLease {
    Owner(CommandDiscoveryOwner),
    Waiter(Arc<CommandDiscoveryEntry>),
}

struct CommandDiscoveryOwner {
    state: Arc<SessionManagementState>,
    key: CommandDiscoveryKey,
    entry: Arc<CommandDiscoveryEntry>,
    complete: bool,
}

impl CommandDiscoveryOwner {
    fn finish(
        mut self,
        result: Result<ProviderCommandCatalogue, RuntimeError>,
    ) -> Result<ProviderCommandCatalogue, RuntimeError> {
        self.entry.complete(result.clone());
        self.remove();
        self.complete = true;
        result
    }

    fn remove(&self) {
        remove_entry_if_current(&self.state.command_discoveries, &self.key, &self.entry);
    }
}

impl Drop for CommandDiscoveryOwner {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        self.entry.complete(Err(local_error(
            RuntimeErrorKind::Lifecycle,
            "Provider command discovery ended before a result was available.",
            true,
        )));
        self.remove();
    }
}

impl SessionRuntime {
    /// Discovers commands for one already-authorized launch scope. Discovery
    /// is bounded globally and serialized per engine, and never creates a chat.
    ///
    /// # Errors
    ///
    /// Returns only scope-validation, missing-engine, or runtime-shutdown
    /// failures. Provider failures and timeouts are sanitized into an
    /// unavailable catalogue for the exact requested scope.
    pub async fn list_provider_commands(
        &self,
        scope: ProviderCommandScope,
    ) -> Result<ProviderCommandCatalogue, RuntimeError> {
        let adapter = self.adapter(&scope.engine)?;
        // The lease is claimed on the validated request before the blocking
        // folder check, so an identical concurrent request joins at once. A
        // different spelling of the same folder makes its own call, serialized
        // per engine; both answer with the canonical scope.
        let scope = validated_command_scope(scope)?;
        let owner = match self.inner.management.command_lease(&scope)? {
            CommandDiscoveryLease::Waiter(entry) => return entry.wait().await,
            CommandDiscoveryLease::Owner(owner) => owner,
        };
        let per_engine = self
            .inner
            .management
            .command_engines
            .get(&scope.engine)
            .cloned()
            .ok_or_else(|| engine_missing(&scope.engine))?;
        let result = async {
            let scope = canonical_command_scope(scope).await?;
            // A queued job for an already-busy provider must not consume one
            // of the four global active slots. The source scheduler counts
            // active providers, not every pending scope for one provider.
            let _engine = per_engine.lock().await;
            let _global = self
                .inner
                .management
                .command_limit
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| {
                    local_error(
                        RuntimeErrorKind::Lifecycle,
                        "provider command discovery is shutting down",
                        true,
                    )
                })?;
            let request = ProviderCommandRequest {
                cwd: scope.cwd.clone(),
                model: scope.model.clone(),
                setting_sources: scope.setting_sources.clone(),
            };
            let provider = tokio::time::timeout(
                PROVIDER_COMMAND_TIMEOUT,
                adapter.list_provider_commands(request),
            )
            .await;
            Ok(match provider {
                Ok(Ok(result)) if result.available => ProviderCommandCatalogue {
                    scope,
                    available: true,
                    commands: result.commands,
                    reason: None,
                },
                Ok(Ok(result)) => command_unavailable(scope, result.reason.as_deref()),
                Ok(Err(_)) | Err(_) => command_unavailable(
                    scope,
                    Some(
                        "Interface could not read commands from this provider for the selected launch.",
                    ),
                ),
            })
        }
        .await;
        owner.finish(result)
    }
}

/// The synchronous half of scope admission, run before the discovery lease:
/// text limits and the settings scope, deduplicated in order.
fn validated_command_scope(
    mut scope: ProviderCommandScope,
) -> Result<ProviderCommandScope, RuntimeError> {
    validate_command_scope_text(&scope)?;
    if let Some(sources) = scope.setting_sources.as_mut() {
        if sources.len() > 3
            || sources
                .iter()
                .any(|source| !matches!(source.as_str(), "user" | "project" | "local"))
        {
            return Err(local_error(
                RuntimeErrorKind::InvalidInput,
                "The settings scope is invalid; no provider was contacted.",
                false,
            ));
        }
        let mut seen = HashSet::new();
        sources.retain(|source| seen.insert(source.clone()));
    }
    Ok(scope)
}

/// Replaces the requested folder with its canonical path after the
/// TOCTOU-hardened symlink check, before any provider is contacted.
async fn canonical_command_scope(
    mut scope: ProviderCommandScope,
) -> Result<ProviderCommandScope, RuntimeError> {
    let requested = scope.cwd;
    scope.cwd = blocking(move || {
        let invalid = || {
            local_error(
                RuntimeErrorKind::InvalidInput,
                "Choose an existing project folder that is not a symbolic link.",
                false,
            )
        };
        let resolved = lexically_absolute(Path::new(&requested)).map_err(|_| invalid())?;
        let before = std::fs::symlink_metadata(&resolved).map_err(|_| invalid())?;
        if !before.is_dir() || before.file_type().is_symlink() {
            return Err(invalid());
        }
        let canonical = std::fs::canonicalize(&resolved).map_err(|_| invalid())?;
        let after = std::fs::symlink_metadata(&canonical).map_err(|_| invalid())?;
        if canonical != resolved
            || !after.is_dir()
            || after.file_type().is_symlink()
            || before.dev() != after.dev()
            || before.ino() != after.ino()
        {
            return Err(invalid());
        }
        canonical
            .into_os_string()
            .into_string()
            .map_err(|_| invalid())
    })
    .await?;
    Ok(scope)
}

pub(super) fn validate_command_scope_text(
    scope: &ProviderCommandScope,
) -> Result<(), RuntimeError> {
    if scope.cwd.trim().is_empty()
        || exceeds_javascript_string_limit(&scope.cwd, 4_096)
        || scope.cwd.contains('\0')
    {
        return Err(local_error(
            RuntimeErrorKind::InvalidInput,
            "Choose a registered provider and an exact open project folder first.",
            false,
        ));
    }
    if let Some(model) = scope.model.as_ref() {
        if model.is_empty()
            || exceeds_javascript_string_limit(model, 256)
            || model.trim() != model
            || model
                .chars()
                .any(|character| character <= '\u{001f}' || character == '\u{007f}')
        {
            return Err(local_error(
                RuntimeErrorKind::InvalidInput,
                "The selected provider model is invalid or too long.",
                false,
            ));
        }
    }
    Ok(())
}

fn command_unavailable(
    scope: ProviderCommandScope,
    reason: Option<&str>,
) -> ProviderCommandCatalogue {
    const FALLBACK: &str = "This provider cannot list commands before a chat starts.";
    const REASON_LIMIT: usize = 480;
    let normalized = reason
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let safe = if normalized.is_empty() {
        FALLBACK.to_owned()
    } else {
        ellipsize(&normalized, REASON_LIMIT)
    };
    ProviderCommandCatalogue {
        scope,
        available: false,
        commands: Vec::new(),
        reason: Some(safe),
    }
}
