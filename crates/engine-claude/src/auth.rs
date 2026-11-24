//! Whether Claude Code is signed in, asked of the CLI itself with
//! `claude auth status --json`. Interface never reads Claude's credentials;
//! the CLI's own answer is the only source.

use std::path::Path;
use std::time::Duration;

use interface_contracts::engine::EngineAuth;
use interface_engine::cli_query_with_environment;
use serde_json::Value;

/// How long `claude auth status` may take before the answer is "unknown".
pub const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// What `claude auth status --json` said.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeAuthStatus {
    pub auth: EngineAuth,
    /// The account's email, when signed in and the CLI named one.
    pub email: Option<String>,
}

/// Asks `executable` whether it is signed in. `None` when it could not say:
/// it did not start, did not finish within `limit`, or printed no
/// `loggedIn` answer (an older CLI without the command).
pub async fn auth_status(executable: &Path, limit: Duration) -> Option<ClaudeAuthStatus> {
    auth_status_with_environment(executable, &[], limit).await
}

/// [`auth_status`] asked with an added account's environment
/// (`CLAUDE_CONFIG_DIR`), so the answer is that account's.
pub async fn auth_status_with_environment(
    executable: &Path,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    limit: Duration,
) -> Option<ClaudeAuthStatus> {
    let answer = cli_query_with_environment(
        executable,
        &["auth", "status", "--json"],
        environment,
        limit,
    )
    .await?;
    // Signed out exits 1 and still prints the JSON.
    parse_auth_status(&answer.stdout)
}

fn parse_auth_status(stdout: &str) -> Option<ClaudeAuthStatus> {
    let value: Value = serde_json::from_str(stdout.trim()).ok()?;
    let signed_in = value.get("loggedIn")?.as_bool()?;
    let email = signed_in
        .then(|| value.get("email").and_then(Value::as_str))
        .flatten()
        .map(str::trim)
        .filter(|email| {
            !email.is_empty() && email.len() <= 320 && !email.contains(char::is_control)
        })
        .map(ToOwned::to_owned);
    Some(ClaudeAuthStatus {
        auth: if signed_in {
            EngineAuth::SignedIn
        } else {
            EngineAuth::SignedOut
        },
        email,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_auth_status_reads_logged_in_and_never_guesses() {
        assert_eq!(
            parse_auth_status(
                r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#
            ),
            Some(ClaudeAuthStatus {
                auth: EngineAuth::SignedOut,
                email: None
            })
        );
        assert_eq!(
            parse_auth_status(
                "{\n  \"loggedIn\": true,\n  \"authMethod\": \"claude.ai\",\n  \"email\": \"person@example.test\"\n}\n"
            ),
            Some(ClaudeAuthStatus {
                auth: EngineAuth::SignedIn,
                email: Some("person@example.test".into())
            })
        );
        assert_eq!(parse_auth_status(""), None);
        assert_eq!(
            parse_auth_status("Not logged in. Run claude auth login."),
            None
        );
        assert_eq!(parse_auth_status(r#"{"loggedIn": "yes"}"#), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_auth_status_asks_the_cli_with_its_own_status_command() {
        use std::os::unix::fs::PermissionsExt;
        let temporary = tempfile::tempdir().expect("temporary");
        let fake = temporary.path().join("claude");
        std::fs::write(
            &fake,
            "#!/bin/sh\n[ \"$1 $2 $3\" = \"auth status --json\" ] || exit 9\n\
             echo '{\"loggedIn\": false}'\nexit 1\n",
        )
        .expect("fake");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(
            auth_status(&fake, Duration::from_secs(10)).await,
            Some(ClaudeAuthStatus {
                auth: EngineAuth::SignedOut,
                email: None
            })
        );
        assert_eq!(
            auth_status(&temporary.path().join("missing"), Duration::from_secs(1)).await,
            None
        );
    }
}
