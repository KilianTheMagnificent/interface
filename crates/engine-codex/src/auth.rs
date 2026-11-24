//! Whether Codex is signed in, by its own `codex login status`, for callers
//! without an app-server connection (`iface doctor`). The adapter's probe
//! reads the same fact from the app-server's `account/read`. Interface never
//! reads Codex's credentials.

use std::path::Path;
use std::time::Duration;

use interface_contracts::engine::EngineAuth;
use interface_engine::{CliAnswer, cli_query};

/// How long `codex login status` may take before the answer is "unknown".
pub const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// `None` when Codex could not say: it did not start or finish in time, or
/// failed for another reason than being signed out.
pub async fn auth_status(executable: &Path, limit: Duration) -> Option<EngineAuth> {
    auth_from_answer(&cli_query(executable, &["login", "status"], limit).await?)
}

fn auth_from_answer(answer: &CliAnswer) -> Option<EngineAuth> {
    if answer.success {
        return Some(EngineAuth::SignedIn);
    }
    let said = format!("{}\n{}", answer.stdout, answer.stderr).to_ascii_lowercase();
    said.contains("not logged in")
        .then_some(EngineAuth::SignedOut)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(success: bool, stdout: &str, stderr: &str) -> CliAnswer {
        CliAnswer {
            success,
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn codex_login_status_reads_signed_in_and_out_and_nothing_else() {
        assert_eq!(
            auth_from_answer(&answer(true, "Logged in using ChatGPT\n", "")),
            Some(EngineAuth::SignedIn)
        );
        assert_eq!(
            auth_from_answer(&answer(false, "", "Not logged in\n")),
            Some(EngineAuth::SignedOut)
        );
        assert_eq!(
            auth_from_answer(&answer(false, "", "Error loading config.toml")),
            None
        );
    }
}
