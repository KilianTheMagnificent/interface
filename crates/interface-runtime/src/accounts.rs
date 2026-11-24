//! Several accounts per provider (protocol 60): one adapter per provider
//! account, behind the one engine id the registry knows.
//!
//! [`AccountRoutedAdapter`] stands where a provider's adapter stood. The
//! Default account (the host's inherited environment) is the adapter exactly
//! as before: every call for it reaches that adapter unchanged, its handles
//! are not wrapped, and a provider with no added accounts behaves as if the
//! router were not there. An added account gets its own adapter, built on
//! first use by the host's factory with that account's environment and store
//! roots (so a Codex account has its own app-server lanes). A launch names an
//! account (`LaunchOptions.account`, absent meaning the provider's default);
//! every later call that names a session (resume, history, fork, rename,
//! delete, archive, adopt) goes to the account the session is bound to. A
//! session bound to an account that no longer exists is history that cannot
//! be resumed. Listings merge every account's store and bind what an added
//! account's store holds. The account is never part of the session key or
//! the engine id.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::engine::{
    EngineEvent, EngineId, EngineInfo, EngineSessionListing, EngineUsage, LaunchOptions,
    ResumeOptions,
};
use interface_contracts::ipc::{EngineOp, HistoryPage, unavailable_history};
use interface_contracts::provider_accounts::DEFAULT_ACCOUNT_ID;
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EngineAdapter,
    EngineError, EngineErrorKind, ForkBoundary, HistoryRequest, ProviderCommandRequest,
    ProviderCommandResult, ReadySession, SessionHandle,
};

/// Answer for a call on a session whose account was removed.
pub const ACCOUNT_REMOVED_MESSAGE: &str =
    "This session ran on a provider account that was removed, so it cannot be resumed.";
/// Answer for a provider archive change on such a session: only Interface's
/// own archive (which needs no provider) can hide it.
pub const ACCOUNT_REMOVED_ARCHIVE_MESSAGE: &str = "This session ran on a provider account that was removed, so its provider archive cannot change. Archive it in Interface instead.";
/// How long one added account's usage read may take before its row says so.
const ACCOUNT_USAGE_TIMEOUT: Duration = Duration::from_secs(12);

/// One added account, as the router's factory needs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountProfile {
    pub id: String,
    pub label: String,
    /// The account's own directory (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`,
    /// `GROK_HOME`).
    pub directory: PathBuf,
}

/// The host's accounts and bindings, as the router reads and writes them.
pub trait AccountDirectory: Send + Sync + 'static {
    /// The account a launch naming none uses: `default` or an added id.
    fn default_account(&self, engine: &EngineId) -> String;
    /// The added account `account` of `engine`, when it exists now.
    fn profile(&self, engine: &EngineId, account: &str) -> Option<AccountProfile>;
    /// Every added account of `engine` that exists now, in order.
    fn profiles(&self, engine: &EngineId) -> Vec<AccountProfile>;
    /// The added account a session is bound to; `None` means Default.
    fn bound(&self, engine: &EngineId, session_id: &str) -> Option<String>;
    /// Binds sessions (id, account) in one write. A session already bound
    /// keeps its account unless `replace`. Failing to record is logged by the
    /// implementation; the session itself is unaffected.
    fn bind(&self, engine: &EngineId, bindings: Vec<(String, String)>, replace: bool);
}

/// Builds the adapter for one added account: the provider's adapter with the
/// account's environment and store roots.
pub type AccountAdapterFactory =
    Arc<dyn Fn(&AccountProfile) -> Result<Arc<dyn EngineAdapter>, EngineError> + Send + Sync>;

/// Where a call goes.
enum Route {
    Default,
    Account(String, Arc<dyn EngineAdapter>),
}

/// A provider's adapters, one per account, behind its one engine id.
pub struct AccountRoutedAdapter {
    id: EngineId,
    default: Arc<dyn EngineAdapter>,
    factory: AccountAdapterFactory,
    directory: Arc<dyn AccountDirectory>,
    built: Mutex<HashMap<String, Arc<dyn EngineAdapter>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn removed() -> EngineError {
    EngineError::definitely_unsent(
        EngineErrorKind::InvalidInput,
        ACCOUNT_REMOVED_MESSAGE,
        false,
    )
}

fn unknown_account() -> EngineError {
    EngineError::definitely_unsent(
        EngineErrorKind::InvalidInput,
        "That provider account no longer exists; nothing was started.",
        false,
    )
}

impl AccountRoutedAdapter {
    /// `default` is the provider's adapter exactly as it ran before accounts.
    #[must_use]
    pub fn new(
        default: Arc<dyn EngineAdapter>,
        factory: AccountAdapterFactory,
        directory: Arc<dyn AccountDirectory>,
    ) -> Self {
        Self {
            id: default.id().clone(),
            default,
            factory,
            directory,
            built: Mutex::new(HashMap::new()),
        }
    }

    /// The adapter for added account `profile`, built on first use.
    fn account_adapter(
        &self,
        profile: &AccountProfile,
    ) -> Result<Arc<dyn EngineAdapter>, EngineError> {
        if let Some(adapter) = lock(&self.built).get(&profile.id) {
            return Ok(Arc::clone(adapter));
        }
        let adapter = (self.factory)(profile)?;
        if adapter.id() != &self.id {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Internal,
                "an account adapter answered for a different engine",
                false,
            ));
        }
        Ok(Arc::clone(
            lock(&self.built)
                .entry(profile.id.clone())
                .or_insert(adapter),
        ))
    }

    /// The account a launch naming `account` runs on.
    fn launch_route(&self, account: Option<&str>) -> Result<Route, EngineError> {
        let account = account.map_or_else(
            || self.directory.default_account(&self.id),
            ToOwned::to_owned,
        );
        if account == DEFAULT_ACCOUNT_ID {
            return Ok(Route::Default);
        }
        let profile = self
            .directory
            .profile(&self.id, &account)
            .ok_or_else(unknown_account)?;
        Ok(Route::Account(account, self.account_adapter(&profile)?))
    }

    /// The account `session_id` is bound to.
    fn session_route(&self, session_id: &str) -> Result<Route, EngineError> {
        let Some(account) = self.directory.bound(&self.id, session_id) else {
            return Ok(Route::Default);
        };
        let profile = self
            .directory
            .profile(&self.id, &account)
            .ok_or_else(removed)?;
        Ok(Route::Account(account, self.account_adapter(&profile)?))
    }

    /// Whether `session_id` is bound to an added account that no longer
    /// exists: history the provider can no longer list or resume.
    fn bound_to_removed_account(&self, session_id: &str) -> bool {
        self.directory
            .bound(&self.id, session_id)
            .is_some_and(|account| self.directory.profile(&self.id, &account).is_none())
    }

    fn route_adapter<'a>(&'a self, route: &'a Route) -> &'a dyn EngineAdapter {
        match route {
            Route::Default => self.default.as_ref(),
            Route::Account(_, adapter) => adapter.as_ref(),
        }
    }

    /// Asks one account's CLI what it is (signed in, as whom), with that
    /// account's environment. `default` asks the provider's own adapter.
    ///
    /// # Errors
    /// An account that no longer exists, or the probe's own error.
    pub async fn probe_account(&self, account: &str) -> Result<EngineInfo, EngineError> {
        if account == DEFAULT_ACCOUNT_ID {
            return self.default.probe().await;
        }
        let profile = self
            .directory
            .profile(&self.id, account)
            .ok_or_else(unknown_account)?;
        self.account_adapter(&profile)?.probe().await
    }

    /// Forgets a removed account's adapter. Live sessions keep running on
    /// what they hold; nothing new can start on it.
    pub fn forget_account(&self, account: &str) {
        lock(&self.built).remove(account);
    }

    fn bind_fork(&self, route: &Route, forked: &str) {
        if let Route::Account(account, _) = route {
            self.directory
                .bind(&self.id, vec![(forked.to_owned(), account.clone())], true);
        }
    }
}

#[async_trait]
impl EngineAdapter for AccountRoutedAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }

    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        self.default.probe().await
    }

    fn executable(&self) -> Option<PathBuf> {
        self.default.executable()
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        let profiles = self.directory.profiles(&self.id);
        if profiles.is_empty() {
            return self.default.list_sessions().await;
        }
        let mut listings = Vec::with_capacity(profiles.len());
        for profile in &profiles {
            listings.push(async move {
                let adapter = self.account_adapter(profile)?;
                adapter.list_sessions().await
            });
        }
        let (default, accounts) =
            tokio::join!(self.default.list_sessions(), futures_join_all(listings));
        let mut merged = default?;
        let mut seen = merged
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<HashSet<_>>();
        let mut bindings = Vec::new();
        for (profile, listing) in profiles.iter().zip(accounts) {
            match listing {
                Ok(listing) => {
                    merged.complete &= listing.complete;
                    for session in listing.sessions {
                        bindings.push((session.session_id.clone(), profile.id.clone()));
                        if seen.insert(session.session_id.clone()) {
                            merged.sessions.push(session);
                        } else if self
                            .directory
                            .bound(&self.id, &session.session_id)
                            .as_deref()
                            == Some(profile.id.as_str())
                            && let Some(slot) = merged
                                .sessions
                                .iter_mut()
                                .find(|kept| kept.session_id == session.session_id)
                        {
                            *slot = session;
                        }
                    }
                }
                Err(_) => merged.complete = false,
            }
        }
        if !bindings.is_empty() {
            self.directory.bind(&self.id, bindings, false);
        }
        Ok(merged)
    }

    fn listing_reuse(&self) -> Duration {
        self.default.listing_reuse()
    }

    /// A removed account's sessions stay listed as history: no store lists
    /// them any more, and the owner deletes or archives them in Interface.
    fn keeps_unlisted_session(&self, session_id: &str) -> bool {
        self.bound_to_removed_account(session_id) || self.default.keeps_unlisted_session(session_id)
    }

    fn supported_ops(&self) -> Vec<EngineOp> {
        self.default.supported_ops()
    }

    fn supports_history(&self) -> bool {
        self.default.supports_history()
    }

    async fn rename_session(
        &self,
        session_id: String,
        title: String,
        cwd: Option<String>,
    ) -> Result<(), EngineError> {
        let route = self.session_route(&session_id)?;
        self.route_adapter(&route)
            .rename_session(session_id, title, cwd)
            .await
    }

    async fn delete_session(
        &self,
        session_id: String,
        cwd: Option<String>,
    ) -> Result<(), EngineError> {
        if self.bound_to_removed_account(&session_id) {
            // The account record is gone, so where its CLI kept the
            // transcript is not guessed: a kept folder is left as it is, and
            // only Interface's binding goes (the runtime drops the row).
            self.directory.bind(
                &self.id,
                vec![(session_id, DEFAULT_ACCOUNT_ID.to_owned())],
                true,
            );
            return Ok(());
        }
        let route = self.session_route(&session_id)?;
        self.route_adapter(&route)
            .delete_session(session_id, cwd)
            .await
    }

    async fn fork_session(
        &self,
        session_id: String,
        cwd: Option<String>,
    ) -> Result<String, EngineError> {
        let route = self.session_route(&session_id)?;
        let forked = self
            .route_adapter(&route)
            .fork_session(session_id, cwd)
            .await?;
        self.bind_fork(&route, &forked);
        Ok(forked)
    }

    async fn fork_session_at(
        &self,
        session_id: String,
        cwd: Option<String>,
        boundary: ForkBoundary,
    ) -> Result<String, EngineError> {
        let route = self.session_route(&session_id)?;
        let forked = self
            .route_adapter(&route)
            .fork_session_at(session_id, cwd, boundary)
            .await?;
        self.bind_fork(&route, &forked);
        Ok(forked)
    }

    async fn set_archived(
        &self,
        session_id: String,
        archived: bool,
        cwd: Option<String>,
    ) -> Result<(), EngineError> {
        if self.bound_to_removed_account(&session_id) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                ACCOUNT_REMOVED_ARCHIVE_MESSAGE,
                false,
            ));
        }
        let route = self.session_route(&session_id)?;
        self.route_adapter(&route)
            .set_archived(session_id, archived, cwd)
            .await
    }

    async fn adopt_session(
        &self,
        session_id: String,
        adopt: bool,
        cwd: Option<String>,
    ) -> Result<(), EngineError> {
        let route = self.session_route(&session_id)?;
        self.route_adapter(&route)
            .adopt_session(session_id, adopt, cwd)
            .await
    }

    async fn read_usage(&self) -> Result<EngineUsage, EngineError> {
        self.default.read_usage().await
    }

    async fn read_usage_rows(&self) -> Vec<Result<EngineUsage, EngineError>> {
        let profiles = self.directory.profiles(&self.id);
        let mut reads = Vec::with_capacity(profiles.len());
        for profile in profiles {
            reads.push(async move {
                let usage = match self.account_adapter(&profile) {
                    Ok(adapter) => {
                        tokio::time::timeout(ACCOUNT_USAGE_TIMEOUT, adapter.read_usage()).await
                    }
                    Err(error) => Ok(Err(error)),
                };
                let mut row = match usage {
                    Ok(Ok(usage)) => usage,
                    Ok(Err(_)) | Err(_) => EngineUsage {
                        engine: self.id.clone(),
                        available: false,
                        plan: None,
                        windows: Vec::new(),
                        cost_usd: None,
                        note: Some(format!(
                            "Interface could not read usage for the {} account.",
                            profile.label
                        )),
                        account: None,
                        account_label: None,
                    },
                };
                row.account = Some(profile.id.clone());
                row.account_label = Some(profile.label.clone());
                Ok(row)
            });
        }
        let (default, accounts) = tokio::join!(self.default.read_usage(), futures_join_all(reads));
        let mut rows = vec![default];
        rows.extend(accounts);
        rows
    }

    async fn list_provider_commands(
        &self,
        scope: ProviderCommandRequest,
    ) -> Result<ProviderCommandResult, EngineError> {
        let route = self.launch_route(None)?;
        self.route_adapter(&route)
            .list_provider_commands(scope)
            .await
    }

    async fn read_history(
        &self,
        session_id: String,
        request: HistoryRequest,
    ) -> Result<HistoryPage, EngineError> {
        let Ok(route) = self.session_route(&session_id) else {
            return Ok(unavailable_history(ACCOUNT_REMOVED_MESSAGE));
        };
        self.route_adapter(&route)
            .read_history(session_id, request)
            .await
    }

    fn create_session(
        &self,
        mut options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        let route = self.launch_route(options.account.take().as_deref())?;
        match route {
            Route::Default => self.default.create_session(options),
            Route::Account(account, adapter) => {
                let handle = adapter.create_session(options)?;
                Ok(Box::new(BindOnReady {
                    inner: handle,
                    engine: self.id.clone(),
                    account,
                    directory: Arc::clone(&self.directory),
                }))
            }
        }
    }

    fn resume_session(
        &self,
        session_id: String,
        cwd: String,
        options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        let route = self.session_route(&session_id)?;
        self.route_adapter(&route)
            .resume_session(session_id, cwd, options)
    }

    async fn dispose(&self) -> Result<(), EngineError> {
        let built = lock(&self.built).values().cloned().collect::<Vec<_>>();
        for adapter in built {
            let _ = adapter.dispose().await;
        }
        self.default.dispose().await
    }

    fn approval_facts(
        &self,
        tool: &str,
        input: &serde_json::Value,
    ) -> interface_contracts::agents::ApprovalFacts {
        self.default.approval_facts(tool, input)
    }
}

/// Runs futures concurrently and answers their outputs in order.
async fn futures_join_all<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
    let mut pinned = futures.into_iter().map(Box::pin).collect::<Vec<_>>();
    let mut outputs = Vec::with_capacity(pinned.len());
    outputs.resize_with(pinned.len(), || None);
    std::future::poll_fn(|context| {
        let mut pending = false;
        for (slot, future) in outputs.iter_mut().zip(pinned.iter_mut()) {
            if slot.is_none() {
                match future.as_mut().poll(context) {
                    std::task::Poll::Ready(output) => *slot = Some(output),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    outputs.into_iter().flatten().collect()
}

/// An added account's session handle: binds the session to its account once
/// the provider names it, and is otherwise the account adapter's handle.
struct BindOnReady {
    inner: Box<dyn SessionHandle>,
    engine: EngineId,
    account: String,
    directory: Arc<dyn AccountDirectory>,
}

#[async_trait]
impl SessionHandle for BindOnReady {
    fn engine(&self) -> &EngineId {
        self.inner.engine()
    }

    fn session_id(&self) -> String {
        self.inner.session_id()
    }

    async fn ready(&self) -> Result<ReadySession, EngineError> {
        let ready = self.inner.ready().await?;
        self.directory.bind(
            &self.engine,
            vec![(ready.session_id.clone(), self.account.clone())],
            true,
        );
        Ok(ready)
    }

    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError> {
        self.inner.take_events()
    }

    async fn send(&self, text: String) -> DeliveryOutcome<()> {
        self.inner.send(text).await
    }

    async fn send_reporting_dispatch(
        &self,
        text: String,
        dispatched: tokio::sync::oneshot::Sender<()>,
    ) -> DeliveryOutcome<()> {
        self.inner.send_reporting_dispatch(text, dispatched).await
    }

    async fn interrupt(&self) -> DeliveryOutcome<()> {
        self.inner.interrupt().await
    }

    async fn close(&self) -> Result<(), EngineError> {
        self.inner.close().await
    }

    async fn respond_to_approval(
        &self,
        approval_id: String,
        verdict: ApprovalVerdict,
        response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        self.inner
            .respond_to_approval(approval_id, verdict, response)
            .await
    }

    async fn set_model(&self, model: Option<String>) -> DeliveryOutcome<()> {
        self.inner.set_model(model).await
    }

    async fn set_effort(&self, effort: Option<String>) -> DeliveryOutcome<()> {
        self.inner.set_effort(effort).await
    }

    async fn set_fast_mode(&self, enabled: bool) -> DeliveryOutcome<()> {
        self.inner.set_fast_mode(enabled).await
    }

    async fn set_permission_mode(&self, mode: String) -> DeliveryOutcome<()> {
        self.inner.set_permission_mode(mode).await
    }
}

#[cfg(test)]
#[path = "accounts_tests.rs"]
mod tests;
