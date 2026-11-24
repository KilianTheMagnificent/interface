//! The text Help → Copy Diagnostics puts on the clipboard: which build this
//! is, where its data lives, what each provider answered and the host's
//! recent warnings.
//!
//! Everything free-form in it (provider errors, log lines) is filtered here
//! before it is shown: token shapes, credential assignments and the values
//! of credential-named variables ([`redact_credentials`],
//! [`credential_variable`]), any path naming credential material by the
//! shared tables (`interface_common::paths::is_sensitive_name`), long
//! mixed-case secrets, and the home folder, which becomes `~`. No login
//! identity is included.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::LazyLock;

use interface_common::paths::is_sensitive_name;
use interface_contracts::ipc::EngineProbe;
use regex::Regex;

use crate::thread_git::ship::{credential_variable, redact_credentials};

/// Recent warning lines a report quotes.
pub const DIAGNOSTIC_WARNING_LINES: usize = 50;
/// A quoted line is cut here.
pub const DIAGNOSTIC_LINE_CHARS: usize = 1_000;

/// What the host knows about itself when it writes a report.
#[derive(Clone, Debug)]
pub struct DiagnosticsFacts<'a> {
    /// The release-stamped product version.
    pub version: &'a str,
    /// `macOS 26.1 (25B78)`, when the system said.
    pub os: Option<&'a str>,
    /// `std::env::consts::ARCH`.
    pub arch: &'a str,
    pub data_root: &'a Path,
    /// Shown as `~` wherever it appears.
    pub home: Option<&'a Path>,
    /// Each provider's last probe, in the app's provider order.
    pub providers: &'a [EngineProbe],
    /// Recent WARN and ERROR lines from `host.log`, oldest first.
    pub warnings: &'a [String],
}

/// The report, every line filtered.
#[must_use]
pub fn render_diagnostics(facts: &DiagnosticsFacts<'_>) -> String {
    let home = facts.home;
    let mut out = String::new();
    let _ = writeln!(out, "Interface diagnostics");
    let _ = writeln!(out, "Version: {}", filter_text(facts.version, home));
    let _ = writeln!(
        out,
        "macOS: {}",
        facts
            .os
            .map_or_else(|| "unknown".to_owned(), |os| filter_text(os, home))
    );
    let _ = writeln!(out, "Architecture: {}", display_arch(facts.arch));
    let _ = writeln!(
        out,
        "Data folder: {}",
        filter_text(&facts.data_root.to_string_lossy(), home)
    );
    out.push('\n');
    let _ = writeln!(out, "Providers");
    if facts.providers.is_empty() {
        let _ = writeln!(out, "- none answered");
    }
    for probe in facts.providers {
        let _ = writeln!(out, "- {}", provider_line(probe, home));
    }
    out.push('\n');
    if facts.warnings.is_empty() {
        let _ = writeln!(out, "Recent warnings: none");
    } else {
        let _ = writeln!(
            out,
            "Recent warnings (newest last, up to {DIAGNOSTIC_WARNING_LINES})"
        );
        let skip = facts
            .warnings
            .len()
            .saturating_sub(DIAGNOSTIC_WARNING_LINES);
        for line in facts.warnings.iter().skip(skip) {
            let _ = writeln!(out, "{}", filter_text(line, home));
        }
    }
    out
}

/// A provider's name the way the app shows it.
#[must_use]
pub fn provider_name(probe: &EngineProbe) -> String {
    if let Some(name) = probe
        .info
        .as_ref()
        .and_then(|info| info.display_name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    match probe.engine.as_str() {
        "claude" => "Claude".to_owned(),
        "codex" => "Codex".to_owned(),
        "acp:grok" => "Grok".to_owned(),
        "acp:qwen" => "Qwen".to_owned(),
        other => other.to_owned(),
    }
}

fn provider_line(probe: &EngineProbe, home: Option<&Path>) -> String {
    let name = filter_text(&provider_name(probe), home);
    let version = probe
        .version
        .as_deref()
        .or_else(|| probe.info.as_ref().map(|info| info.version.as_str()))
        .map(str::trim)
        .filter(|version| !version.is_empty());
    if probe.ok {
        let mut line = format!("{name}: Ready");
        if let Some(version) = version {
            let _ = write!(line, " · {}", filter_text(version, home));
        }
        return line;
    }
    let error = probe
        .error
        .as_deref()
        .map(str::trim)
        .filter(|error| !error.is_empty())
        .unwrap_or("did not answer");
    format!("{name}: Unavailable · {}", filter_text(error, home))
}

fn display_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        other => other,
    }
}

static ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)=(\S+)").expect("a valid pattern"));
/// The value of a `…_token`, `…_secret`, `…_key` or `…_password` field
/// (`refresh_token=`, `?access_token=`, `X-Auth-Token:`, `"client_secret": "`),
/// which [`redact_credentials`] misses: it needs a word boundary before
/// `token`. Kept out of that shared filter on purpose: the commit panel
/// refuses any patch it changes, and source code names fields like these all
/// the time (`primary_key=True`).
static CREDENTIAL_FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b([a-z0-9_-]*[_-](?:token|secret|key|password|passwd)"?\s*[=:]\s*"?)[^\s'",;]+"#,
    )
    .expect("a valid pattern")
});
static JWT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*")
        .expect("a valid pattern")
});
/// Long runs of token characters; [`secret_like`] decides which are secrets.
static LONG_RUN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9_+/=-]{32,}").expect("a valid pattern"));
/// An absolute or home-relative path, up to whitespace or a quote.
static PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?:~|\B)/[^\s'"`<>|]+"#).expect("a valid pattern"));

/// One free-form line made safe to share: credentials redacted, credential
/// paths hidden, the home folder shown as `~`, and cut to
/// [`DIAGNOSTIC_LINE_CHARS`].
#[must_use]
pub fn filter_text(text: &str, home: Option<&Path>) -> String {
    let mut text = text.replace(['\r', '\n'], " ");
    if let Some(home) = home
        .map(|home| home.to_string_lossy().trim_end_matches('/').to_owned())
        .filter(|home| home.len() > 1)
    {
        text = text.replace(&home, "~");
    }
    let text = redact_credentials(&text);
    let text = CREDENTIAL_FIELD.replace_all(&text, "${1}[redacted]");
    let text = JWT.replace_all(&text, "[redacted]");
    let text = ASSIGNMENT.replace_all(&text, |captures: &regex::Captures<'_>| {
        if credential_variable(&captures[1]) {
            format!("{}=[redacted]", &captures[1])
        } else {
            captures[0].to_owned()
        }
    });
    let text = LONG_RUN.replace_all(&text, |captures: &regex::Captures<'_>| {
        if secret_like(&captures[0]) {
            "[redacted]".to_owned()
        } else {
            captures[0].to_owned()
        }
    });
    let text = PATH.replace_all(&text, |captures: &regex::Captures<'_>| {
        let path = &captures[0];
        if path
            .split('/')
            .any(|part| !part.is_empty() && is_sensitive_name(&part.to_ascii_lowercase()))
        {
            "[credential path]".to_owned()
        } else {
            path.to_owned()
        }
    });
    truncate_chars(&text, DIAGNOSTIC_LINE_CHARS)
}

/// A long token mixing upper case, lower case and digits: an API key,
/// session token or signature. Hex digests and UUIDs (one case) stay, so do
/// paths and ordinary words.
fn secret_like(run: &str) -> bool {
    if run.contains('/') && !run.ends_with('=') {
        // A path (or URL path); credential paths are handled on their own.
        return false;
    }
    let upper = run.chars().filter(char::is_ascii_uppercase).count();
    let lower = run.chars().filter(char::is_ascii_lowercase).count();
    let digits = run.chars().filter(char::is_ascii_digit).count();
    upper >= 2 && lower >= 2 && digits >= 2
}

fn truncate_chars(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use interface_contracts::engine::{EngineCapabilities, EngineId, EngineInfo, EngineModels};

    use super::*;

    fn info(version: &str) -> EngineInfo {
        EngineInfo {
            auth: None,
            binary_path: "/Users/me/.local/bin/claude".into(),
            version: version.into(),
            login_identity: Some("person@example.test".into()),
            capabilities: EngineCapabilities {
                streaming: true,
                approvals: true,
                resume: true,
                fork: true,
                cloud_sessions: false,
                models: EngineModels::Dynamic("dynamic".into()),
                effort_levels: Vec::new(),
                permission_modes: Vec::new(),
                consultation_safety: None,
                steer: None,
            },
            display_name: None,
            model_catalog: None,
        }
    }

    #[test]
    fn copy_diagnostics_names_the_build_and_providers_and_never_carries_tokens() {
        let home = PathBuf::from("/Users/me");
        let data_root = home.join("Library/Application Support/Interface");
        let providers = [
            EngineProbe {
                engine: EngineId::new("claude").expect("engine"),
                ok: true,
                version: None,
                info: Some(info("2.1.300")),
                error: None,
            },
            EngineProbe {
                engine: EngineId::new("codex").expect("engine"),
                ok: false,
                version: None,
                info: None,
                error: Some(
                    "initialize: Codex app-server closed stdout (OPENAI_API_KEY=sk-live-abcdefghijklmnopqrstuvwx)"
                        .into(),
                ),
            },
        ];
        let warnings = [
            "2026-10-14T12:00:00Z  WARN interface_host: refresh failed token=ghp_abcdefghijklmnopqrstuvwxyz0123".to_owned(),
            "2026-10-14T12:00:01Z  WARN interface_host: Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJl".to_owned(),
            "2026-10-14T12:00:02Z ERROR interface_host: could not read /Users/me/.ssh/id_ed25519 or /Users/me/project/.env.local".to_owned(),
            "2026-10-14T12:00:03Z  WARN interface_host: session 1f0e9c2a-1111-4111-8111-111111111111 kept AbCdEfGh12345678IjKlMnOp90123456QrSt".to_owned(),
            "2026-10-14T12:00:04Z  WARN interface_host: ANTHROPIC_API_KEY=abc CLAUDE_CODE_OAUTH_TOKEN=xyz PATH=/usr/bin".to_owned(),
            "2026-10-14T12:00:05Z  WARN interface_host: refresh_token=0f3c9a8b7e6d5c4b3a2f1e0d9c8b7a6f5e4d3c2b rejected".to_owned(),
            "2026-10-14T12:00:06Z  WARN interface_host: GET https://x.test/cb?access_token=9e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b&state=1".to_owned(),
            r#"2026-10-14T12:00:07Z  WARN interface_host: body {"client_secret":"abcdefabcdefabcdef","x":1}"#.to_owned(),
            "2026-10-14T12:00:08Z  WARN interface_host: keychain locked; the tokenizer kept 3 keys for keyboard input".to_owned(),
        ];
        let report = render_diagnostics(&DiagnosticsFacts {
            version: "0.3.158",
            os: Some("macOS 26.1 (25B78)"),
            arch: "aarch64",
            data_root: &data_root,
            home: Some(&home),
            providers: &providers,
            warnings: &warnings,
        });
        assert!(report.contains("Version: 0.3.158"), "{report}");
        assert!(report.contains("macOS: macOS 26.1 (25B78)"), "{report}");
        assert!(report.contains("Architecture: arm64"), "{report}");
        assert!(
            report.contains("Data folder: ~/Library/Application Support/Interface"),
            "{report}"
        );
        assert!(report.contains("- Claude: Ready · 2.1.300"), "{report}");
        assert!(
            report.contains("- Codex: Unavailable · initialize: Codex app-server closed stdout"),
            "{report}"
        );
        assert!(
            report.contains("session 1f0e9c2a-1111-4111-8111-111111111111"),
            "UUIDs stay: {report}"
        );
        assert!(report.contains("PATH=/usr/bin"), "{report}");
        assert!(
            report.contains("keychain locked; the tokenizer kept 3 keys for keyboard input"),
            "prose stays: {report}"
        );
        assert!(
            report.contains("refresh_token=[redacted] rejected"),
            "{report}"
        );
        for secret in [
            "sk-live-abcdefghijklmnopqrstuvwx",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "eyJhbGciOiJIUzI1NiJ9",
            "AbCdEfGh12345678IjKlMnOp90123456QrSt",
            "ANTHROPIC_API_KEY=abc",
            "CLAUDE_CODE_OAUTH_TOKEN=xyz",
            "0f3c9a8b7e6d5c4b3a2f1e0d9c8b7a6f5e4d3c2b",
            "9e8d7c6b5a4f3e2d1c0b9a8f7e6d5c4b",
            "abcdefabcdefabcdef",
            "id_ed25519",
            ".env.local",
            "person@example.test",
            "/Users/me",
        ] {
            assert!(!report.contains(secret), "{secret} leaked:\n{report}");
        }
        assert!(report.contains("[credential path]"), "{report}");
    }

    #[test]
    fn a_report_without_warnings_or_providers_says_so_and_keeps_only_the_newest_lines() {
        let data_root = PathBuf::from("/data");
        let empty = render_diagnostics(&DiagnosticsFacts {
            version: "1",
            os: None,
            arch: "x86_64",
            data_root: &data_root,
            home: None,
            providers: &[],
            warnings: &[],
        });
        assert!(empty.contains("macOS: unknown"));
        assert!(empty.contains("- none answered"));
        assert!(empty.contains("Recent warnings: none"));
        let many = (0..60)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        let report = render_diagnostics(&DiagnosticsFacts {
            version: "1",
            os: None,
            arch: "x86_64",
            data_root: &data_root,
            home: None,
            providers: &[],
            warnings: &many,
        });
        assert!(!report.contains("line 9\n"));
        assert!(report.contains("line 10\n"));
        assert!(report.contains("line 59\n"));
        assert_eq!(filter_text(&"x".repeat(1_200), None).chars().count(), 1_001);
    }
}
