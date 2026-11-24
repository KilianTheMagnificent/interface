//! Folder trust, wired: the Claude adapters ask
//! [`DesktopService::folder_trust_of`] before every launch and resume, each
//! with its own account's `.claude.json`, and Threads reads the same
//! decision for its view and its two commands (protocol 72).
//!
//! Where an account's `.claude.json` is: Claude Code keeps its global
//! settings, Claude Code's own trust answers among them, in
//! `$CLAUDE_CONFIG_DIR/.claude.json` when that variable is set (as it is for
//! every added account, set to the account's own folder) and in
//! `~/.claude.json` otherwise
//! ([`interface_settings::folder_trust::claude_config_file`]).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use interface_contracts::{engine::EngineId, provider_accounts::DEFAULT_ACCOUNT_ID};
use interface_desktop_services::{DesktopService, DesktopServiceError, FolderTrustDecision};
use interface_runtime::accounts::AccountDirectory;
use interface_settings::folder_trust::claude_config_file;

/// The engine whose sessions folder trust applies to.
pub const CLAUDE_ENGINE: &str = "claude";

/// The host's one folder-trust decision.
pub struct HostFolderTrust {
    desktop: Arc<DesktopService>,
    home: PathBuf,
    /// The Default account's configuration folder (`CLAUDE_CONFIG_DIR` in
    /// the host's environment), when set.
    default_config_dir: Option<PathBuf>,
    /// Added accounts, once the host has them.
    accounts: OnceLock<Arc<crate::provider_accounts::ProviderAccounts>>,
}

impl HostFolderTrust {
    #[must_use]
    pub fn new(
        desktop: Arc<DesktopService>,
        home: &Path,
        default_config_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            desktop,
            home: home.to_path_buf(),
            default_config_dir: default_config_dir.filter(|folder| folder.is_absolute()),
            accounts: OnceLock::new(),
        })
    }

    /// Attaches the provider accounts, so a thread on an added account is
    /// decided with that account's own record.
    pub fn attach_accounts(&self, accounts: Arc<crate::provider_accounts::ProviderAccounts>) {
        let _ = self.accounts.set(accounts);
    }

    /// Registers a folder Interface made and owns (the Chat folder).
    pub fn add_owned_folder(&self, folder: &Path) {
        self.desktop.add_owned_folder(folder);
    }

    /// The `.claude.json` of the account whose configuration folder is
    /// `config_dir` (`None`: the Default account).
    #[must_use]
    pub fn claude_config(&self, config_dir: Option<&Path>) -> PathBuf {
        claude_config_file(
            config_dir.or(self.default_config_dir.as_deref()),
            &self.home,
        )
    }

    /// The `.claude.json` of Claude account `account` (`None` or `default`:
    /// the Default account); `None` for an added account that no longer
    /// exists, whose record is then not read.
    fn account_config(&self, account: Option<&str>) -> Option<PathBuf> {
        match account.filter(|id| *id != DEFAULT_ACCOUNT_ID) {
            None => Some(self.claude_config(None)),
            Some(id) => {
                let accounts = self.accounts.get()?;
                let engine = EngineId::new(CLAUDE_ENGINE).ok()?;
                let profile = AccountDirectory::profile(accounts.as_ref(), &engine, id)?;
                Some(self.claude_config(Some(&profile.directory)))
            }
        }
    }

    /// A session of Claude account `account` started in `cwd`.
    #[must_use]
    pub fn decide(&self, cwd: &Path, account: Option<&str>) -> FolderTrustDecision {
        self.desktop
            .folder_trust_of(cwd, self.account_config(account).as_deref())
    }

    /// A new Claude thread in `folder`, on the account a launch naming none
    /// uses.
    #[must_use]
    pub fn decide_new(&self, folder: &Path) -> FolderTrustDecision {
        let account = self.accounts.get().and_then(|accounts| {
            let engine = EngineId::new(CLAUDE_ENGINE).ok()?;
            Some(AccountDirectory::default_account(
                accounts.as_ref(),
                &engine,
            ))
        });
        self.decide(folder, account.as_deref())
    }

    /// Records the owner's answer for `folder`'s trust root.
    ///
    /// # Errors
    /// See [`DesktopService::record_folder_trust`].
    pub fn record(&self, folder: &Path, trust: bool) -> Result<PathBuf, DesktopServiceError> {
        self.desktop.record_folder_trust(folder, trust)
    }

    /// The trust one Claude adapter asks: the Default account's when
    /// `config_dir` is `None`, else the added account's whose folder it is.
    #[must_use]
    pub fn for_adapter(self: &Arc<Self>, config_dir: Option<PathBuf>) -> Arc<AdapterFolderTrust> {
        Arc::new(AdapterFolderTrust {
            host: Arc::clone(self),
            config_dir,
        })
    }
}

/// [`engine_claude::FolderTrust`] for one account's adapter.
pub struct AdapterFolderTrust {
    host: Arc<HostFolderTrust>,
    config_dir: Option<PathBuf>,
}

impl engine_claude::FolderTrust for AdapterFolderTrust {
    fn settings_allowed(&self, cwd: &Path) -> bool {
        let config = self.host.claude_config(self.config_dir.as_deref());
        self.host
            .desktop
            .folder_trust_of(cwd, Some(&config))
            .trusted()
    }
}
