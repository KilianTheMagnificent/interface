//! Several accounts per provider (protocol 60): named profile directories for
//! the Claude, Codex and Grok CLIs, and the `iface:providerAccounts` request
//! that lists and changes them. Rust owns the stores
//! (`interface_settings::provider_accounts`) and every rule; the renderer only
//! asks. Contract: `docs/superpowers/specs/2026-11-14-t3-systems-design.md`
//! section 6. Both languages read
//! `electron/src/fixtures/provider-accounts-v60.json`.

use serde::{Deserialize, Serialize};

use crate::{ValidationError, engine::EngineId};

/// The account Interface has always used: the host's inherited environment.
/// It has no directory of its own and cannot be removed.
pub const DEFAULT_ACCOUNT_ID: &str = "default";
/// The most added accounts one provider may have (Default not counted).
pub const MAX_ACCOUNTS_PER_PROVIDER: usize = 8;
/// The longest account label, in characters.
pub const MAX_ACCOUNT_LABEL_CHARS: usize = 40;
/// The longest account id, in bytes.
pub const MAX_ACCOUNT_ID_BYTES: usize = 40;
/// [`ProviderAccountsView::version`].
pub const PROVIDER_ACCOUNTS_VIEW_VERSION: u8 = 1;

/// A provider whose CLI keeps its sign-in in one directory an environment
/// variable names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountProvider {
    Claude,
    Codex,
    Grok,
}

impl AccountProvider {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Grok];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }

    /// The engine id its structured sessions run under.
    #[must_use]
    pub const fn engine(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "acp:grok",
        }
    }

    /// The provider whose structured sessions run under `engine`.
    #[must_use]
    pub fn for_engine(engine: &EngineId) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.engine() == engine.as_str())
    }

    /// The variable that points the CLI at an account's directory.
    #[must_use]
    pub const fn directory_variable(self) -> &'static str {
        match self {
            Self::Claude => "CLAUDE_CONFIG_DIR",
            Self::Codex => "CODEX_HOME",
            Self::Grok => "GROK_HOME",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }
}

/// Whether `id` is a well-formed account id: `default`, or the lowercase
/// letters, digits and hyphens Rust mints.
#[must_use]
pub fn valid_account_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ACCOUNT_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A label trimmed and checked: 1 to 40 characters, no control characters.
///
/// # Errors
/// The label is empty, too long or carries control characters.
pub fn clean_account_label(label: &str) -> Result<String, ValidationError> {
    let label = label.trim();
    if label.is_empty() {
        return Err(ValidationError::new("label", "an account needs a name"));
    }
    if label.chars().count() > MAX_ACCOUNT_LABEL_CHARS {
        return Err(ValidationError::new(
            "label",
            "an account name is at most 40 characters",
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(ValidationError::new(
            "label",
            "an account name cannot hold control characters",
        ));
    }
    Ok(label.to_owned())
}

/// What `iface:providerAccounts` is asked.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "command",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderAccountsCommand {
    /// The accounts and the last sign-in each CLI reported. Runs no CLI.
    Read {},
    /// Asks each account's CLI whether it is signed in (`claude auth status`,
    /// Codex `account/read`, Grok's own probe), with that account's
    /// environment, then answers the view. `provider` limits it to one.
    Refresh {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<AccountProvider>,
    },
    /// Creates the account's directory and record.
    Add {
        provider: AccountProvider,
        label: String,
    },
    Rename {
        provider: AccountProvider,
        account: String,
        label: String,
    },
    /// New sessions and new terminal panes of `provider` use `account`
    /// (`default` for the inherited one).
    SetDefault {
        provider: AccountProvider,
        account: String,
    },
    /// Removes the record; `deleteDirectory` also deletes its directory and
    /// is sent only after the owner confirmed what goes. Sessions bound to it
    /// stay listed as history that cannot be resumed.
    Remove {
        provider: AccountProvider,
        account: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        delete_directory: bool,
    },
}

impl ProviderAccountsCommand {
    /// # Errors
    /// An account id that is not well formed, or a bad label.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Read {} | Self::Refresh { .. } => Ok(()),
            Self::Add { label, .. } => clean_account_label(label).map(|_| ()),
            Self::Rename { account, label, .. } => {
                validate_account_field(account)?;
                clean_account_label(label).map(|_| ())
            }
            Self::SetDefault { account, .. } | Self::Remove { account, .. } => {
                validate_account_field(account)
            }
        }
    }
}

/// # Errors
/// `account` is not a well-formed account id.
pub fn validate_account_field(account: &str) -> Result<(), ValidationError> {
    if valid_account_id(account) {
        Ok(())
    } else {
        Err(ValidationError::new("account", "is not an account id"))
    }
}

/// Whether an account's CLI says it is signed in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountSignIn {
    SignedIn,
    SignedOut,
    /// Not asked yet, or the CLI could not say.
    Unknown,
}

/// One account as Settings → Providers shows it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderAccountView {
    /// `default` for the inherited account.
    pub id: String,
    pub label: String,
    /// New sessions and terminal panes use it.
    pub is_default: bool,
    pub sign_in: AccountSignIn,
    /// The signed-in identity the CLI named (an email or account type).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// The account's own directory; absent for Default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Unix milliseconds; absent for Default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}

/// One provider's accounts, Default first.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderAccountGroup {
    pub provider: AccountProvider,
    /// The engine id its sessions run under.
    pub engine: EngineId,
    pub accounts: Vec<ProviderAccountView>,
}

/// What every `iface:providerAccounts` command answers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderAccountsView {
    /// Always [`PROVIDER_ACCOUNTS_VIEW_VERSION`].
    pub version: u8,
    /// Claude, Codex, Grok, in that order.
    pub providers: Vec<ProviderAccountGroup>,
    /// What the last command did, for a notice; absent on `read`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/provider-accounts-v60.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn provider_accounts_fixture_round_trips_every_shape() {
        let fixture = fixture();
        assert_eq!(fixture["channel"], crate::ipc::CHANNELS.provider_accounts);
        assert_eq!(fixture["protocol"], 60);
        for request in fixture["requests"].as_array().expect("requests") {
            let command: ProviderAccountsCommand =
                serde_json::from_value(request.clone()).expect("command");
            command.validate().expect("a valid command");
            assert_eq!(serde_json::to_value(&command).expect("encode"), *request);
        }
        for refused in fixture["refusedRequests"].as_array().expect("refused") {
            let decoded = serde_json::from_value::<ProviderAccountsCommand>(refused.clone());
            assert!(
                decoded.is_err() || decoded.is_ok_and(|command| command.validate().is_err()),
                "{refused}"
            );
        }
        let view: ProviderAccountsView =
            serde_json::from_value(fixture["view"].clone()).expect("view");
        assert_eq!(view.version, PROVIDER_ACCOUNTS_VIEW_VERSION);
        assert_eq!(
            serde_json::to_value(&view).expect("encode"),
            fixture["view"]
        );
        let launch: crate::engine::LaunchOptions =
            serde_json::from_value(fixture["launch"].clone()).expect("launch");
        assert_eq!(launch.account.as_deref(), Some("acct-0123456789ab"));
        assert_eq!(
            serde_json::to_value(&launch).expect("encode"),
            fixture["launch"]
        );
        let usage: crate::engine::EngineUsage =
            serde_json::from_value(fixture["usage"].clone()).expect("usage");
        assert_eq!(usage.account_label.as_deref(), Some("Work"));
        assert_eq!(
            serde_json::to_value(&usage).expect("encode"),
            fixture["usage"]
        );
        let terminal: crate::terminal::CreateTerminalRequest =
            serde_json::from_value(fixture["signInTerminal"].clone()).expect("terminal");
        terminal.validate_sign_in().expect("a valid sign-in");
        assert_eq!(terminal.account.as_deref(), Some("acct-0123456789ab"));
        assert_eq!(
            serde_json::to_value(&terminal).expect("encode"),
            fixture["signInTerminal"]
        );
        let continued: crate::threads::ThreadCommand =
            serde_json::from_value(fixture["continueWith"].clone()).expect("continueWith");
        assert!(matches!(
            &continued,
            crate::threads::ThreadCommand::ContinueWith { account: Some(account), .. }
                if account == "acct-0123456789ab"
        ));
        assert_eq!(
            serde_json::to_value(&continued).expect("encode"),
            fixture["continueWith"]
        );
        let thread: crate::threads::ThreadView =
            serde_json::from_value(fixture["thread"].clone()).expect("thread");
        assert_eq!(thread.account.as_deref(), Some("acct-0123456789ab"));
        assert_eq!(
            serde_json::to_value(&thread).expect("encode"),
            fixture["thread"]
        );
    }

    #[test]
    fn account_ids_and_labels_are_checked() {
        assert!(valid_account_id(DEFAULT_ACCOUNT_ID));
        assert!(valid_account_id("acct-0123456789ab"));
        assert!(!valid_account_id(""));
        assert!(!valid_account_id("../x"));
        assert!(!valid_account_id("Work"));
        assert!(!valid_account_id(&"a".repeat(41)));
        assert_eq!(clean_account_label("  Work ").unwrap(), "Work");
        assert!(clean_account_label("   ").is_err());
        assert!(clean_account_label(&"x".repeat(41)).is_err());
        assert!(clean_account_label("a\nb").is_err());
        assert_eq!(
            AccountProvider::for_engine(&EngineId::new("acp:grok").unwrap()),
            Some(AccountProvider::Grok)
        );
        assert_eq!(
            AccountProvider::for_engine(&EngineId::new("acp:qwen").unwrap()),
            None
        );
    }
}
