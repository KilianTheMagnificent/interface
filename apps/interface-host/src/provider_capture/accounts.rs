//! Several accounts per provider (protocol 60): a Claude pane running on an
//! added account keeps its PID sidecar and transcript under that account's
//! `CLAUDE_CONFIG_DIR`, not the inherited one. [`AccountCaptureLookup`]
//! answers every call with the Default account's lookup exactly as before;
//! only when that lookup finds no Claude session (or refuses a transcript
//! outside its store) does it ask each added Claude account's lookup, with
//! the same checks, and bind a session found there to its account.
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use interface_contracts::{
    engine::EngineId,
    provider_accounts::AccountProvider,
    terminal::{TerminalLaunchProfileId, TerminalLaunchSessionV1},
};
use interface_runtime::accounts::AccountDirectory;
use interface_terminal::{
    TerminalBackendError, TerminalProcessIdentity, TerminalProviderSession,
    TerminalProviderSessionCaptureOwner, TerminalProviderSessionCaptureProcess,
    TerminalProviderSessionLookup,
};

use super::ProviderCaptureLookup;
use crate::provider_accounts::ProviderAccounts;

/// The Default account's capture lookup, then each added Claude account's.
pub(crate) struct AccountCaptureLookup {
    default: ProviderCaptureLookup,
    receipts: PathBuf,
    accounts: OnceLock<Arc<ProviderAccounts>>,
}

type Found = Result<Option<TerminalProviderSession>, TerminalBackendError>;

impl AccountCaptureLookup {
    pub(crate) fn new(default: ProviderCaptureLookup, receipts: &Path) -> Self {
        Self {
            default,
            receipts: receipts.to_path_buf(),
            accounts: OnceLock::new(),
        }
    }

    /// Installs the host's provider accounts once.
    pub(crate) fn install_accounts(&self, accounts: Arc<ProviderAccounts>) {
        let _ = self.accounts.set(accounts);
    }

    /// `ask` of the Default lookup; when that finds no Claude session, `ask`
    /// of each added Claude account's lookup, binding what one finds.
    fn with_accounts(
        &self,
        profile_id: TerminalLaunchProfileId,
        ask: impl Fn(&ProviderCaptureLookup) -> Found,
    ) -> Found {
        let found = ask(&self.default);
        if profile_id != TerminalLaunchProfileId::Claude || matches!(found, Ok(Some(_))) {
            return found;
        }
        let Some(accounts) = self.accounts.get() else {
            return found;
        };
        let Ok(engine) = EngineId::new(AccountProvider::Claude.engine()) else {
            return found;
        };
        for profile in accounts.profiles(&engine) {
            let Ok(lookup) =
                ProviderCaptureLookup::new(&profile.directory.join("projects"), &self.receipts)
            else {
                continue;
            };
            if let Ok(Some(session)) = ask(&lookup) {
                accounts.bind(&engine, vec![(session.id.clone(), profile.id)], false);
                return Ok(Some(session));
            }
        }
        found
    }
}

impl TerminalProviderSessionLookup for AccountCaptureLookup {
    fn begin_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        self.default.begin_capture(profile_id, owner)
    }

    fn record_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        self.default
            .record_process(profile_id, capture_token, process)
    }

    fn capture_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        self.default.capture_process(profile_id, capture_token)
    }

    fn seal_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<Option<TerminalProviderSessionCaptureProcess>, TerminalBackendError> {
        self.default.seal_capture(profile_id, capture_token)
    }

    fn replace_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        previous: &TerminalProcessIdentity,
        replacement: &TerminalProcessIdentity,
        cwd: &Path,
        prior_launch_session: Option<&TerminalLaunchSessionV1>,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        self.default.replace_nested_process(
            profile_id,
            capture_token,
            previous,
            replacement,
            cwd,
            prior_launch_session,
        )
    }

    fn rearm_nested_process(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        process: &TerminalProcessIdentity,
        cwd: &Path,
    ) -> Result<(), TerminalBackendError> {
        self.default
            .rearm_nested_process(profile_id, capture_token, process, cwd)
    }

    fn checkpoint_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
        launch_session: &TerminalLaunchSessionV1,
    ) -> Result<(), TerminalBackendError> {
        self.default
            .checkpoint_capture(profile_id, capture_token, launch_session)
    }

    fn current_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Found {
        self.with_accounts(profile_id, |lookup| {
            lookup.current_session(profile_id, process, capture_token)
        })
    }

    fn settled_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Found {
        self.with_accounts(profile_id, |lookup| {
            lookup.settled_session(profile_id, process, capture_token)
        })
    }

    fn final_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        process: &TerminalProcessIdentity,
        capture_token: &str,
    ) -> Found {
        self.with_accounts(profile_id, |lookup| {
            lookup.final_session(profile_id, process, capture_token)
        })
    }

    fn end_capture(
        &self,
        profile_id: TerminalLaunchProfileId,
        capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        self.default.end_capture(profile_id, capture_token)
    }
}
