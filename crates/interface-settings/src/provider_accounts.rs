//! Several accounts per provider (protocol 60): the named profile
//! directories the Claude, Codex and Grok CLIs run with, and which account
//! each session is bound to. Contract: `docs/superpowers/specs/
//! 2026-11-14-t3-systems-design.md` section 6.
//!
//! Two Rust-owned files under the data root, each leased, atomic and set
//! aside when it does not decode:
//! - `provider-accounts.v1.json`: the added accounts (at most
//!   [`MAX_ACCOUNTS_PER_PROVIDER`] per provider) and each provider's default.
//!   The Default account (the host's inherited environment) is never
//!   recorded: it always exists, has no directory and cannot be removed.
//! - `session-accounts.v1.json`: session key → account id, only for sessions
//!   on an added account (absent means Default). The account is never part
//!   of the session key or the engine id.
//!
//! Added accounts' directories live under `<data root>/accounts/<provider>/
//! <id>/`, owner-private (0700). Nothing here reads what a CLI keeps there.
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use interface_contracts::provider_accounts::{
    AccountProvider, DEFAULT_ACCOUNT_ID, MAX_ACCOUNTS_PER_PROVIDER, clean_account_label,
    valid_account_id,
};
use interface_storage::{JsonLimits, PrivateDirectory};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{SettingsError, leased::LeasedDocument};

/// The accounts file, under the data root.
pub const PROVIDER_ACCOUNTS_FILE: &str = "provider-accounts.v1.json";
/// The session bindings file, under the data root.
pub const SESSION_ACCOUNTS_FILE: &str = "session-accounts.v1.json";
/// The folder holding every added account's directory, under the data root.
pub const ACCOUNTS_DIRECTORY: &str = "accounts";
/// The most sessions `session-accounts.v1.json` binds. Entries whose session
/// left the registry are dropped first ([`SessionAccountsStore::retain`]).
pub const MAX_SESSION_BINDINGS: usize = 10_000;
const MAX_SESSION_KEY_BYTES: usize = 512;
const ACCOUNTS_BYTES: usize = 64 * 1024;
const SESSIONS_BYTES: usize = 2 * 1024 * 1024;

/// One added account as `provider-accounts.v1.json` keeps it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredAccount {
    pub id: String,
    pub provider: AccountProvider,
    pub label: String,
    /// Unix milliseconds.
    pub created_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AccountsState {
    version: u8,
    accounts: Vec<StoredAccount>,
    /// Absent means `default`.
    defaults: BTreeMap<AccountProvider, String>,
}

impl AccountsState {
    const fn empty() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            defaults: BTreeMap::new(),
        }
    }
}

fn valid_accounts(state: &AccountsState) -> bool {
    let mut ids = BTreeSet::new();
    state.version == 1
        && state.accounts.iter().all(|account| {
            account.id != DEFAULT_ACCOUNT_ID
                && valid_account_id(&account.id)
                && ids.insert(account.id.clone())
                && clean_account_label(&account.label).is_ok_and(|label| label == account.label)
        })
        && AccountProvider::ALL.iter().all(|provider| {
            state
                .accounts
                .iter()
                .filter(|account| account.provider == *provider)
                .count()
                <= MAX_ACCOUNTS_PER_PROVIDER
        })
        && state.defaults.iter().all(|(provider, id)| {
            id == DEFAULT_ACCOUNT_ID
                || state
                    .accounts
                    .iter()
                    .any(|account| account.provider == *provider && &account.id == id)
        })
}

/// What a removal did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemovedAccount {
    pub account: StoredAccount,
    /// The account's directory, and whether it was deleted.
    pub directory: PathBuf,
    pub directory_deleted: bool,
}

/// The added provider accounts and each provider's default.
pub struct ProviderAccountsStore {
    document: LeasedDocument,
    state: AccountsState,
    root: PathBuf,
    quarantined: Option<PathBuf>,
}

fn refuse(message: impl Into<String>) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}

impl ProviderAccountsStore {
    /// Opens `<data_root>/provider-accounts.v1.json`.
    ///
    /// # Errors
    /// Refuses unsafe or already leased storage. A file that does not decode
    /// is set aside and the store starts with no added accounts.
    pub fn open(data_root: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<AccountsState>(
            &data_root.join(PROVIDER_ACCOUNTS_FILE),
            ".provider-accounts.lock",
            JsonLimits::new(ACCOUNTS_BYTES, 6, 1_024)?,
            "The provider accounts could not confirm their last write; restart Interface.",
            valid_accounts,
        )?;
        Ok(Self {
            document,
            state: state.unwrap_or_else(AccountsState::empty),
            root: data_root.join(ACCOUNTS_DIRECTORY),
            quarantined,
        })
    }

    /// Where opening set an unreadable file aside, when it did.
    #[must_use]
    pub fn quarantined(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// The added accounts of `provider`, oldest first.
    #[must_use]
    pub fn accounts(&self, provider: AccountProvider) -> Vec<StoredAccount> {
        self.state
            .accounts
            .iter()
            .filter(|account| account.provider == provider)
            .cloned()
            .collect()
    }

    /// The added account `id` of `provider`.
    #[must_use]
    pub fn account(&self, provider: AccountProvider, id: &str) -> Option<&StoredAccount> {
        self.state
            .accounts
            .iter()
            .find(|account| account.provider == provider && account.id == id)
    }

    /// The account new sessions and terminal panes of `provider` use:
    /// `default` or an added account's id.
    #[must_use]
    pub fn default_account(&self, provider: AccountProvider) -> &str {
        self.state
            .defaults
            .get(&provider)
            .map_or(DEFAULT_ACCOUNT_ID, String::as_str)
    }

    /// The directory an added account's CLI keeps its sign-in and history in.
    #[must_use]
    pub fn directory(&self, provider: AccountProvider, id: &str) -> PathBuf {
        self.root.join(provider.as_str()).join(id)
    }

    /// Creates a new account's directory and record. Answers the record.
    ///
    /// # Errors
    /// A bad label, a provider already at [`MAX_ACCOUNTS_PER_PROVIDER`], an
    /// unsafe accounts folder, or failed or uncertain storage.
    pub fn add(
        &mut self,
        provider: AccountProvider,
        label: &str,
        now_ms: u64,
    ) -> Result<StoredAccount, SettingsError> {
        self.document.ensure_writable()?;
        let label = clean_account_label(label).map_err(|error| refuse(error.reason))?;
        if self.accounts(provider).len() >= MAX_ACCOUNTS_PER_PROVIDER {
            return Err(refuse(format!(
                "{} already has {MAX_ACCOUNTS_PER_PROVIDER} added accounts; remove one first.",
                provider.display_name()
            )));
        }
        let id = self.mint_id(provider, &label, now_ms);
        self.create_directory(provider, &id)?;
        let account = StoredAccount {
            id,
            provider,
            label,
            created_at: now_ms,
        };
        let mut state = self.state.clone();
        state.accounts.push(account.clone());
        self.document.commit(&state)?;
        self.state = state;
        Ok(account)
    }

    /// # Errors
    /// An unknown account, a bad label, or failed or uncertain storage.
    pub fn rename(
        &mut self,
        provider: AccountProvider,
        id: &str,
        label: &str,
    ) -> Result<StoredAccount, SettingsError> {
        self.document.ensure_writable()?;
        if id == DEFAULT_ACCOUNT_ID {
            return Err(refuse("The Default account keeps its name."));
        }
        let label = clean_account_label(label).map_err(|error| refuse(error.reason))?;
        let mut state = self.state.clone();
        let account = state
            .accounts
            .iter_mut()
            .find(|account| account.provider == provider && account.id == id)
            .ok_or_else(|| refuse("That account no longer exists."))?;
        account.label = label;
        let renamed = account.clone();
        if state != self.state {
            self.document.commit(&state)?;
            self.state = state;
        }
        Ok(renamed)
    }

    /// # Errors
    /// An unknown account, or failed or uncertain storage.
    pub fn set_default(
        &mut self,
        provider: AccountProvider,
        id: &str,
    ) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if id != DEFAULT_ACCOUNT_ID && self.account(provider, id).is_none() {
            return Err(refuse("That account no longer exists."));
        }
        let mut state = self.state.clone();
        if id == DEFAULT_ACCOUNT_ID {
            state.defaults.remove(&provider);
        } else {
            state.defaults.insert(provider, id.to_owned());
        }
        if state != self.state {
            self.document.commit(&state)?;
            self.state = state;
        }
        Ok(())
    }

    /// Removes an added account's record (its provider's default goes back to
    /// Default when it was this one), then, only when `delete_directory`, its
    /// directory. The record goes first, so a directory that cannot be
    /// deleted leaves no account pointing at a half-removed folder.
    ///
    /// # Errors
    /// Default, an unknown account, or failed or uncertain storage. A
    /// directory that could not be deleted is not an error: the answer says
    /// it is still there.
    pub fn remove(
        &mut self,
        provider: AccountProvider,
        id: &str,
        delete_directory: bool,
    ) -> Result<RemovedAccount, SettingsError> {
        self.document.ensure_writable()?;
        if id == DEFAULT_ACCOUNT_ID {
            return Err(refuse("The Default account cannot be removed."));
        }
        let account = self
            .account(provider, id)
            .cloned()
            .ok_or_else(|| refuse("That account no longer exists."))?;
        let mut state = self.state.clone();
        state.accounts.retain(|kept| kept != &account);
        if state
            .defaults
            .get(&provider)
            .is_some_and(|default| default == id)
        {
            state.defaults.remove(&provider);
        }
        self.document.commit(&state)?;
        self.state = state;
        let directory = self.directory(provider, id);
        let directory_deleted = delete_directory && self.delete_directory(&directory);
        Ok(RemovedAccount {
            account,
            directory,
            directory_deleted,
        })
    }

    /// The account directory, created owner-private along with its parents
    /// (each private, none a symlink).
    fn create_directory(&self, provider: AccountProvider, id: &str) -> Result<(), SettingsError> {
        let accounts = PrivateDirectory::open_or_create(&self.root)?;
        let provider_directory = accounts.open_or_create_child(provider.as_str())?;
        provider_directory.open_or_create_child(id)?;
        Ok(())
    }

    /// Deletes a removed account's directory when it is a real directory
    /// directly under its provider's folder. Answers whether it is gone.
    fn delete_directory(&self, directory: &Path) -> bool {
        let inside = directory
            .parent()
            .and_then(Path::parent)
            .is_some_and(|parent| parent == self.root);
        let real = std::fs::symlink_metadata(directory).is_ok_and(|meta| meta.is_dir());
        if !inside || !real {
            return !directory.exists();
        }
        std::fs::remove_dir_all(directory).is_ok()
    }

    /// A fresh id: `acct-` and twelve hex digits of a digest of the provider,
    /// label, time, process and a counter, never one already recorded.
    fn mint_id(&self, provider: AccountProvider, label: &str, now_ms: u64) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        loop {
            let mut digest = Sha256::new();
            digest.update(provider.as_str());
            digest.update(label);
            digest.update(now_ms.to_le_bytes());
            digest.update(std::process::id().to_le_bytes());
            digest.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            digest.update(nanos.to_le_bytes());
            let mut id = String::from("acct-");
            for byte in digest.finalize().iter().take(6) {
                use std::fmt::Write as _;
                let _ = write!(id, "{byte:02x}");
            }
            if !self.state.accounts.iter().any(|account| account.id == id) {
                return id;
            }
        }
    }
}

/// The environment an added account's CLI runs with: its directory variable
/// (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME`) pointing at `directory`.
#[must_use]
pub fn account_environment(
    provider: AccountProvider,
    directory: &Path,
) -> Vec<(OsString, OsString)> {
    vec![(
        OsString::from(provider.directory_variable()),
        directory.as_os_str().to_owned(),
    )]
}

/// The account a thread started by team tools runs on: its caller's account
/// (`default` included) when it runs on the caller's provider, otherwise
/// `None`, the target provider's default account.
#[must_use]
pub fn started_thread_account(
    caller_engine: &str,
    caller_account: Option<&str>,
    target_engine: &str,
) -> Option<String> {
    (caller_engine == target_engine)
        .then(|| caller_account.unwrap_or(DEFAULT_ACCOUNT_ID).to_owned())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionsState {
    version: u8,
    /// Session key → added account id.
    sessions: BTreeMap<String, String>,
}

fn valid_sessions(state: &SessionsState) -> bool {
    state.version == 1
        && state.sessions.len() <= MAX_SESSION_BINDINGS
        && state
            .sessions
            .iter()
            .all(|(key, account)| valid_session_key(key) && valid_bound_account(account))
}

fn valid_session_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_SESSION_KEY_BYTES
        && key.contains('#')
        && !key.contains(char::is_control)
}

fn valid_bound_account(account: &str) -> bool {
    account != DEFAULT_ACCOUNT_ID && valid_account_id(account)
}

/// Which added account each session runs on.
pub struct SessionAccountsStore {
    document: LeasedDocument,
    state: SessionsState,
    quarantined: Option<PathBuf>,
}

impl SessionAccountsStore {
    /// Opens `<data_root>/session-accounts.v1.json`.
    ///
    /// # Errors
    /// Refuses unsafe or already leased storage. A file that does not decode
    /// is set aside and every session reads as Default until listings bind
    /// them again.
    pub fn open(data_root: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<SessionsState>(
            &data_root.join(SESSION_ACCOUNTS_FILE),
            ".session-accounts.lock",
            JsonLimits::new(SESSIONS_BYTES, 4, 2 * MAX_SESSION_BINDINGS + 16)?,
            "The session accounts could not confirm their last write; restart Interface.",
            valid_sessions,
        )?;
        Ok(Self {
            document,
            state: state.unwrap_or(SessionsState {
                version: 1,
                sessions: BTreeMap::new(),
            }),
            quarantined,
        })
    }

    /// Where opening set an unreadable file aside, when it did.
    #[must_use]
    pub fn quarantined(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// The added account `key` is bound to; `None` means Default.
    #[must_use]
    pub fn account_of(&self, key: &str) -> Option<&str> {
        self.state.sessions.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.state.sessions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.sessions.is_empty()
    }

    /// Binds sessions to accounts in one write. `default` unbinds. A session
    /// already bound keeps its account unless `replace`.
    ///
    /// # Errors
    /// A malformed key or account, more than [`MAX_SESSION_BINDINGS`]
    /// sessions, or failed or uncertain storage. Nothing is bound then.
    pub fn bind_all(
        &mut self,
        bindings: impl IntoIterator<Item = (String, String)>,
        replace: bool,
    ) -> Result<usize, SettingsError> {
        self.document.ensure_writable()?;
        let mut state = self.state.clone();
        let mut changed = 0;
        for (key, account) in bindings {
            if !valid_session_key(&key) || !valid_account_id(&account) {
                return Err(refuse("Invalid session account binding."));
            }
            if account == DEFAULT_ACCOUNT_ID {
                if replace && state.sessions.remove(&key).is_some() {
                    changed += 1;
                }
                continue;
            }
            match state.sessions.get(&key) {
                Some(bound) if !replace || bound == &account => {}
                _ => {
                    state.sessions.insert(key, account);
                    changed += 1;
                }
            }
        }
        if state.sessions.len() > MAX_SESSION_BINDINGS {
            return Err(refuse(format!(
                "Interface already remembers the account of {MAX_SESSION_BINDINGS} sessions."
            )));
        }
        if changed > 0 {
            self.document.commit(&state)?;
            self.state = state;
        }
        Ok(changed)
    }

    /// Drops every binding whose session `keep` refuses (left the registry),
    /// in one write. Answers how many went.
    ///
    /// # Errors
    /// Failed or uncertain storage; nothing is dropped then.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) -> Result<usize, SettingsError> {
        self.document.ensure_writable()?;
        let mut state = self.state.clone();
        state.sessions.retain(|key, _| keep(key));
        let dropped = self.state.sessions.len() - state.sessions.len();
        if dropped > 0 {
            self.document.commit(&state)?;
            self.state = state;
        }
        Ok(dropped)
    }
}

#[cfg(test)]
#[path = "provider_accounts_tests.rs"]
mod tests;
