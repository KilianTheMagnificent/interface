//! Several accounts per provider (protocol 60): serves
//! `iface:providerAccounts`, owns the two account stores
//! (`interface_settings::provider_accounts`), routes structured sessions
//! through each provider's `AccountRoutedAdapter`, and tells terminal
//! launches which account's directory to use. Contract:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 6.
//!
//! The Default account is the host's inherited environment: nothing here
//! changes how it runs. Sign-in state comes only from each CLI itself, run
//! with the account's environment.
use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsString,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    engine::{EngineAuth, EngineId, session_key},
    ipc::{CHANNELS, parse_key},
    provider_accounts::{
        AccountProvider, AccountSignIn, DEFAULT_ACCOUNT_ID, PROVIDER_ACCOUNTS_VIEW_VERSION,
        ProviderAccountGroup, ProviderAccountView, ProviderAccountsCommand, ProviderAccountsView,
    },
    terminal::{TerminalLaunchProfileId, TerminalLaunchSessionV1},
};
use interface_runtime::accounts::{AccountDirectory, AccountProfile, AccountRoutedAdapter};
use interface_settings::provider_accounts::{
    ProviderAccountsStore, SessionAccountsStore, account_environment,
};
use interface_terminal::{TerminalAccountLaunch, TerminalAccounts};
use serde_json::Value;

use crate::dispatcher::{DispatchError, HostDispatcher, OPERATION_NOT_MIGRATED_CODE};

/// The one operation this owner serves.
pub const PROVIDER_ACCOUNTS_OPERATIONS: [&str; 1] = [CHANNELS.provider_accounts];

const INVALID_CODE: &str = "provider-accounts-invalid";
const REFUSED_CODE: &str = "provider-accounts-refused";
const UNAVAILABLE_CODE: &str = "provider-accounts-unavailable";
/// How long one account's sign-in check may take.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(30);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What one account's CLI last said: signed in or not, and as whom.
type SignInState = (AccountSignIn, Option<String>);

/// The host's provider accounts and session bindings.
pub struct ProviderAccounts {
    accounts: Mutex<ProviderAccountsStore>,
    sessions: Mutex<SessionAccountsStore>,
    /// What each account's CLI last said: (provider, account) → state, identity.
    sign_in: Mutex<HashMap<(AccountProvider, String), SignInState>>,
    routers: Mutex<HashMap<AccountProvider, Arc<AccountRoutedAdapter>>>,
}

impl ProviderAccounts {
    /// Opens both stores under `data_root`. Only the process holding the
    /// host lock may call this.
    ///
    /// # Errors
    /// Unsafe or already leased storage.
    pub fn open(data_root: &Path) -> Result<Arc<Self>, interface_settings::SettingsError> {
        let accounts = ProviderAccountsStore::open(data_root)?;
        if let Some(path) = accounts.quarantined() {
            tracing::warn!(path = %path.display(), "set aside an unreadable provider accounts file");
        }
        let sessions = SessionAccountsStore::open(data_root)?;
        if let Some(path) = sessions.quarantined() {
            tracing::warn!(path = %path.display(), "set aside an unreadable session accounts file");
        }
        Ok(Arc::new(Self {
            accounts: Mutex::new(accounts),
            sessions: Mutex::new(sessions),
            sign_in: Mutex::new(HashMap::new()),
            routers: Mutex::new(HashMap::new()),
        }))
    }

    /// Records the router standing for `provider`'s adapter.
    pub fn install_router(&self, provider: AccountProvider, router: Arc<AccountRoutedAdapter>) {
        lock(&self.routers).insert(provider, router);
    }

    /// Drops bindings whose session left the registry (`known` keys), in one
    /// write. Run once at startup, before any session opens.
    pub fn prune_sessions(&self, known: &BTreeSet<String>) {
        match lock(&self.sessions).retain(|key| known.contains(key)) {
            Ok(0) => {}
            Ok(dropped) => tracing::info!(dropped, "dropped account bindings of removed sessions"),
            Err(error) => tracing::warn!(%error, "could not drop stale session account bindings"),
        }
    }

    /// The added account a session key is bound to; `None` is Default.
    #[must_use]
    pub fn account_of_key(&self, key: &str) -> Option<String> {
        lock(&self.sessions).account_of(key).map(ToOwned::to_owned)
    }

    /// The environment an added account's CLI runs with; empty for Default
    /// or an account that no longer exists.
    #[must_use]
    pub fn environment(
        &self,
        provider: AccountProvider,
        account: &str,
    ) -> Vec<(OsString, OsString)> {
        if account == DEFAULT_ACCOUNT_ID {
            return Vec::new();
        }
        let accounts = lock(&self.accounts);
        accounts
            .account(provider, account)
            .map(|_| account_environment(provider, &accounts.directory(provider, account)))
            .unwrap_or_default()
    }

    /// The account a session key runs on, as an environment: its bound
    /// account's, or nothing for Default.
    #[must_use]
    pub fn session_environment(&self, key: &str) -> Vec<(OsString, OsString)> {
        let Ok((engine, _)) = parse_key(key) else {
            return Vec::new();
        };
        let (Some(provider), Some(account)) = (
            AccountProvider::for_engine(&engine),
            self.account_of_key(key),
        ) else {
            return Vec::new();
        };
        self.environment(provider, &account)
    }

    /// The account a team-tools thread started by `caller_key` on `engine`
    /// runs on (`None`: that provider's default account).
    #[must_use]
    pub fn started_thread_account(&self, caller_key: &str, engine: &str) -> Option<String> {
        let (caller_engine, _) = parse_key(caller_key).ok()?;
        AccountProvider::for_engine(&EngineId::new(engine).ok()?)?;
        interface_settings::provider_accounts::started_thread_account(
            caller_engine.as_str(),
            self.account_of_key(caller_key).as_deref(),
            engine,
        )
    }

    fn profile_of(&self, provider: AccountProvider, account: &str) -> Option<AccountProfile> {
        let accounts = lock(&self.accounts);
        accounts
            .account(provider, account)
            .map(|stored| AccountProfile {
                id: stored.id.clone(),
                label: stored.label.clone(),
                directory: accounts.directory(provider, &stored.id),
            })
    }

    /// Every provider's accounts as Settings → Providers shows them.
    #[must_use]
    pub fn view(&self, notice: Option<String>) -> ProviderAccountsView {
        let accounts = lock(&self.accounts);
        let sign_in = lock(&self.sign_in);
        let status = |provider: AccountProvider, id: &str| {
            sign_in
                .get(&(provider, id.to_owned()))
                .cloned()
                .unwrap_or((AccountSignIn::Unknown, None))
        };
        let providers = AccountProvider::ALL
            .into_iter()
            .filter_map(|provider| {
                let engine = EngineId::new(provider.engine()).ok()?;
                let default = accounts.default_account(provider).to_owned();
                let (state, identity) = status(provider, DEFAULT_ACCOUNT_ID);
                let mut views = vec![ProviderAccountView {
                    id: DEFAULT_ACCOUNT_ID.into(),
                    label: "Default".into(),
                    is_default: default == DEFAULT_ACCOUNT_ID,
                    sign_in: state,
                    identity,
                    directory: None,
                    created_at: None,
                }];
                for account in accounts.accounts(provider) {
                    let (state, identity) = status(provider, &account.id);
                    views.push(ProviderAccountView {
                        is_default: default == account.id,
                        sign_in: state,
                        identity,
                        directory: Some(
                            accounts
                                .directory(provider, &account.id)
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        created_at: Some(account.created_at),
                        id: account.id,
                        label: account.label,
                    });
                }
                Some(ProviderAccountGroup {
                    provider,
                    engine,
                    accounts: views,
                })
            })
            .collect();
        ProviderAccountsView {
            version: PROVIDER_ACCOUNTS_VIEW_VERSION,
            providers,
            notice,
        }
    }

    /// Asks each account's CLI whether it is signed in, with its own
    /// environment, all at once.
    async fn refresh(&self, only: Option<AccountProvider>) {
        let mut checks = Vec::new();
        for provider in AccountProvider::ALL {
            if only.is_some_and(|only| only != provider) {
                continue;
            }
            let Some(router) = lock(&self.routers).get(&provider).cloned() else {
                continue;
            };
            let mut ids = vec![DEFAULT_ACCOUNT_ID.to_owned()];
            ids.extend(
                lock(&self.accounts)
                    .accounts(provider)
                    .into_iter()
                    .map(|account| account.id),
            );
            for id in ids {
                let router = Arc::clone(&router);
                checks.push(async move {
                    let probed =
                        tokio::time::timeout(SIGN_IN_TIMEOUT, router.probe_account(&id)).await;
                    let status = match probed {
                        Ok(Ok(info)) => match info.auth {
                            Some(EngineAuth::SignedIn) => {
                                (AccountSignIn::SignedIn, info.login_identity)
                            }
                            Some(EngineAuth::SignedOut) => (AccountSignIn::SignedOut, None),
                            None => (AccountSignIn::Unknown, info.login_identity),
                        },
                        Ok(Err(_)) | Err(_) => (AccountSignIn::Unknown, None),
                    };
                    ((provider, id), status)
                });
            }
        }
        let results = futures::future::join_all(checks).await;
        let mut sign_in = lock(&self.sign_in);
        for (key, status) in results {
            sign_in.insert(key, status);
        }
    }

    /// Runs one command and answers the view.
    ///
    /// # Errors
    /// A refused change (unknown account, bounds, Default) is replay-safe;
    /// a storage failure may have written and is not.
    pub async fn handle(
        &self,
        command: ProviderAccountsCommand,
    ) -> Result<ProviderAccountsView, DispatchError> {
        let notice = match command {
            ProviderAccountsCommand::Read {} => None,
            ProviderAccountsCommand::Refresh { provider } => {
                self.refresh(provider).await;
                None
            }
            ProviderAccountsCommand::Add { provider, label } => {
                let added = lock(&self.accounts)
                    .add(provider, &label, crate::threads::now_ms())
                    .map_err(|error| store_error(&error))?;
                Some(format!("Added {}. Sign in to use it.", added.label))
            }
            ProviderAccountsCommand::Rename {
                provider,
                account,
                label,
            } => {
                let renamed = lock(&self.accounts)
                    .rename(provider, &account, &label)
                    .map_err(|error| store_error(&error))?;
                Some(format!("Renamed to {}.", renamed.label))
            }
            ProviderAccountsCommand::SetDefault { provider, account } => {
                lock(&self.accounts)
                    .set_default(provider, &account)
                    .map_err(|error| store_error(&error))?;
                Some(format!(
                    "New {} sessions and terminal panes use this account.",
                    provider.display_name()
                ))
            }
            ProviderAccountsCommand::Remove {
                provider,
                account,
                delete_directory,
            } => {
                let removed = lock(&self.accounts)
                    .remove(provider, &account, delete_directory)
                    .map_err(|error| store_error(&error))?;
                if let Some(router) = lock(&self.routers).get(&provider) {
                    router.forget_account(&account);
                }
                lock(&self.sign_in).remove(&(provider, account));
                Some(if removed.directory_deleted {
                    format!(
                        "Removed {} and deleted its folder. Its sessions stay as history.",
                        removed.account.label
                    )
                } else if delete_directory {
                    format!(
                        "Removed {}. Its folder could not be deleted: {}",
                        removed.account.label,
                        removed.directory.display()
                    )
                } else {
                    format!(
                        "Removed {}. Its folder is kept at {}. Its sessions stay as history.",
                        removed.account.label,
                        removed.directory.display()
                    )
                })
            }
        };
        Ok(self.view(notice))
    }
}

fn store_error(error: &interface_settings::SettingsError) -> DispatchError {
    match error {
        interface_settings::SettingsError::InvalidAgent(message) => {
            DispatchError::new(REFUSED_CODE, message.clone(), false, true)
        }
        other => DispatchError::new(
            UNAVAILABLE_CODE,
            format!("The provider accounts could not be saved: {other}"),
            true,
            false,
        ),
    }
}

impl AccountDirectory for ProviderAccounts {
    fn default_account(&self, engine: &EngineId) -> String {
        AccountProvider::for_engine(engine).map_or_else(
            || DEFAULT_ACCOUNT_ID.to_owned(),
            |provider| lock(&self.accounts).default_account(provider).to_owned(),
        )
    }

    fn profile(&self, engine: &EngineId, account: &str) -> Option<AccountProfile> {
        self.profile_of(AccountProvider::for_engine(engine)?, account)
    }

    fn profiles(&self, engine: &EngineId) -> Vec<AccountProfile> {
        let Some(provider) = AccountProvider::for_engine(engine) else {
            return Vec::new();
        };
        let accounts = lock(&self.accounts);
        accounts
            .accounts(provider)
            .into_iter()
            .map(|stored| AccountProfile {
                directory: accounts.directory(provider, &stored.id),
                id: stored.id,
                label: stored.label,
            })
            .collect()
    }

    fn bound(&self, engine: &EngineId, session_id: &str) -> Option<String> {
        self.account_of_key(&session_key(engine, session_id))
    }

    fn bind(&self, engine: &EngineId, bindings: Vec<(String, String)>, replace: bool) {
        let bindings = bindings
            .into_iter()
            .map(|(session, account)| (session_key(engine, &session), account))
            .collect::<Vec<_>>();
        if let Err(error) = lock(&self.sessions).bind_all(bindings, replace) {
            tracing::warn!(%error, %engine, "could not record which account sessions run on");
        }
    }
}

fn terminal_provider(profile: TerminalLaunchProfileId) -> Option<AccountProvider> {
    match profile {
        TerminalLaunchProfileId::Claude => Some(AccountProvider::Claude),
        TerminalLaunchProfileId::Codex => Some(AccountProvider::Codex),
        TerminalLaunchProfileId::Grok => Some(AccountProvider::Grok),
        _ => None,
    }
}

impl TerminalAccounts for ProviderAccounts {
    fn launch(
        &self,
        profile: TerminalLaunchProfileId,
        account: Option<&str>,
        session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Option<TerminalAccountLaunch>, String> {
        let Some(provider) = terminal_provider(profile) else {
            return Ok(None);
        };
        let engine = EngineId::new(provider.engine()).map_err(|error| error.reason)?;
        let account = match account {
            Some(account) => account.to_owned(),
            None => session
                .and_then(|session| session.id.as_deref())
                .and_then(|id| self.account_of_key(&session_key(&engine, id)))
                .unwrap_or_else(|| lock(&self.accounts).default_account(provider).to_owned()),
        };
        if account == DEFAULT_ACCOUNT_ID {
            return Ok(None);
        }
        let profile = self
            .profile_of(provider, &account)
            .ok_or_else(|| "that account no longer exists".to_owned())?;
        Ok(Some(TerminalAccountLaunch {
            environment: account_environment(provider, &profile.directory),
            codex_sessions_root: (provider == AccountProvider::Codex)
                .then(|| profile.directory.join("sessions")),
            grok_sessions_root: (provider == AccountProvider::Grok)
                .then(|| profile.directory.join("sessions")),
        }))
    }
}

/// Serves `iface:providerAccounts`.
pub struct ProviderAccountsHost {
    accounts: Arc<ProviderAccounts>,
}

impl ProviderAccountsHost {
    #[must_use]
    pub const fn new(accounts: Arc<ProviderAccounts>) -> Self {
        Self { accounts }
    }
}

#[async_trait]
impl HostDispatcher for ProviderAccountsHost {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation != CHANNELS.provider_accounts {
            return Err(DispatchError::new(
                OPERATION_NOT_MIGRATED_CODE,
                "This Interface operation has not migrated to the Rust host.",
                false,
                true,
            ));
        }
        let invalid = || {
            DispatchError::new(
                INVALID_CODE,
                "The provider accounts request was not understood.",
                false,
                true,
            )
        };
        let command: ProviderAccountsCommand =
            crate::payload::decode_object(payload, |_| invalid())?;
        command
            .validate()
            .map_err(|error| DispatchError::new(INVALID_CODE, error.reason, false, true))?;
        let view = self.accounts.handle(command).await?;
        serde_json::to_value(view).map_err(|_| {
            DispatchError::new(
                UNAVAILABLE_CODE,
                "The provider accounts could not be described.",
                false,
                true,
            )
        })
    }
}

#[cfg(test)]
#[path = "provider_accounts_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider_accounts_removed_tests.rs"]
mod removed_tests;
