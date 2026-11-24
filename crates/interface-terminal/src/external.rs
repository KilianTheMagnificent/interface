use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use interface_contracts::ipc::{OpenTerminalResult, TerminalApp};

use crate::error::{TerminalBackendError, TerminalError};
use crate::safety::{ApprovedTerminalRoots, assert_directory, root_error};

pub trait ExternalTerminalBackend: Send + Sync {
    fn ghostty_installed(&self) -> bool;

    /// Launch an external terminal without a shell command string.
    ///
    /// # Errors
    ///
    /// Returns a backend error when the external application cannot be
    /// started.
    fn spawn_detached(
        &self,
        executable: &Path,
        args: &[String],
    ) -> Result<(), TerminalBackendError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemExternalTerminalBackend;

impl ExternalTerminalBackend for SystemExternalTerminalBackend {
    fn ghostty_installed(&self) -> bool {
        fs::metadata("/Applications/Ghostty.app").is_ok()
    }

    fn spawn_detached(
        &self,
        executable: &Path,
        args: &[String],
    ) -> Result<(), TerminalBackendError> {
        let child = Command::new(executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| TerminalBackendError::new(error.to_string()))?;
        std::thread::spawn(move || {
            let mut child = child;
            let _ = child.wait();
        });
        Ok(())
    }
}

/// Open an exact approved root in Ghostty or Terminal.app using direct argv.
///
/// This is the legacy external-terminal operation. It does not create an
/// embedded PTY session.
///
/// # Errors
///
/// Returns an error for an unsafe/unapproved cwd, backend launch failure, or
/// cwd identity drift across the launch operation.
pub fn open_external_terminal(
    target: &Path,
    approved: &ApprovedTerminalRoots,
    backend: &dyn ExternalTerminalBackend,
) -> Result<OpenTerminalResult, TerminalError> {
    if target.as_os_str().is_empty() {
        return Err(TerminalError::UnsafeCwd);
    }
    let root = approved.authorize(target).map_err(root_error)?;
    let canonical = root
        .path
        .to_str()
        .ok_or(TerminalError::UnsafeCwd)?
        .to_owned();
    let (app, args) = if backend.ghostty_installed() {
        (
            TerminalApp::Ghostty,
            vec![
                "-na".to_owned(),
                "Ghostty".to_owned(),
                "--args".to_owned(),
                format!("--working-directory={canonical}"),
            ],
        )
    } else {
        (
            TerminalApp::Terminal,
            vec!["-a".to_owned(), "Terminal".to_owned(), canonical.clone()],
        )
    };
    assert_directory(&root).map_err(|_| TerminalError::CwdChanged)?;
    backend
        .spawn_detached(Path::new("/usr/bin/open"), &args)
        .map_err(|error| TerminalError::Backend(error.to_string()))?;
    assert_directory(&root).map_err(|_| TerminalError::CwdChanged)?;
    Ok(OpenTerminalResult {
        root: canonical,
        app,
    })
}
