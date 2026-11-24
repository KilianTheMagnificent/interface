//! Adapter for the pane-local observation copied from `OpenCode`'s TUI API.
use std::path::Path;

use interface_storage::{JsonLimits, PrivateDirectory, read_json};
use interface_terminal::{
    OPENCODE_SESSION_RECEIPT_FILE, TerminalBackendError, TerminalProcessIdentity,
    TerminalProviderSession, opencode_session_receipt,
};

pub(crate) fn is_opencode_receipt_entry_name(name: &str) -> bool {
    name == OPENCODE_SESSION_RECEIPT_FILE || name == ".interface-opencode-session.json.pending"
}

pub(crate) fn read_opencode_receipt(
    generation: &PrivateDirectory,
    capture_token: &str,
    process: &TerminalProcessIdentity,
    cwd: &Path,
) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
    let invalid = || {
        TerminalBackendError::new("OpenCode terminal session observation is unsafe or malformed")
    };
    let limits = JsonLimits::new(8192, 4, 4096).map_err(|_| invalid())?;
    let Some(loaded) =
        read_json::<serde_json::Value>(generation, OPENCODE_SESSION_RECEIPT_FILE, limits)
            .map_err(|_| invalid())?
    else {
        return Ok(None);
    };
    opencode_session_receipt(
        &loaded.into_parts().0,
        capture_token,
        process.process_id,
        cwd,
    )
    .map(Some)
    .ok_or_else(invalid)
}
