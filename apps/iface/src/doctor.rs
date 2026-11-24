use std::path::Path;

use interface_contracts::engine::EngineAuth;
use interface_contracts::ipc::{CustomizationInventory, CustomizationKind, CustomizationScope};
use serde::Serialize;

use crate::{
    argv::OutputFormat,
    diagnostics::{
        ContextTelemetryCategory, ContextTelemetryPhase, ContextTelemetryRecord,
        parse_context_telemetry_line,
    },
    sanitize::for_display,
    secure_tail::read_private_tail,
};

const CONTEXT_TELEMETRY_MAX_CHARACTERS: usize = 262_144;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DoctorStatus {
    Ready,
    Pending,
    Failed,
    NeedsAuth,
    Unknown,
}

impl DoctorStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::NeedsAuth => "needs-auth",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorEngineRow {
    pub engine: String,
    pub status: DoctorStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorInstructionRow {
    pub name: String,
    pub scope: CustomizationScope,
    pub status: DoctorStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorExtensionRow {
    pub kind: CustomizationKind,
    pub name: String,
    pub scope: CustomizationScope,
    pub status: DoctorStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorContextTelemetry {
    pub source: &'static str,
    pub sessions: Vec<DoctorContextRow>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorContextRow {
    pub session_key: String,
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_size: Option<u64>,
    pub used: u64,
    pub size: u64,
    pub percent: Option<u64>,
    pub recorded_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<ContextTelemetryCategory>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorSnapshot {
    pub schema_version: u8,
    pub cwd: String,
    pub engines: Vec<DoctorEngineRow>,
    pub instructions: Vec<DoctorInstructionRow>,
    pub extensions: Vec<DoctorExtensionRow>,
    pub context_telemetry: DoctorContextTelemetry,
    pub truncated: bool,
}

#[must_use]
pub fn snapshot_from_inventory(
    cwd: &Path,
    inventory: CustomizationInventory,
    telemetry: Vec<DoctorContextRow>,
) -> DoctorSnapshot {
    let engines = ["claude", "codex", "acp:grok", "acp:qwen"]
        .into_iter()
        .map(|engine| DoctorEngineRow {
            engine: engine.to_owned(),
            status: DoctorStatus::Unknown,
            detail: "not probed".to_owned(),
        })
        .collect();
    let mut instructions = Vec::new();
    let mut extensions = Vec::new();
    for entry in inventory.entries {
        let status = if entry.error.as_ref().is_some_and(|error| !error.is_empty()) {
            DoctorStatus::Failed
        } else if entry.kind == CustomizationKind::Mcp {
            DoctorStatus::Pending
        } else {
            DoctorStatus::Unknown
        };
        let detail = if entry.kind == CustomizationKind::Hook && entry.error.is_none() {
            "configured; command and arguments hidden".to_owned()
        } else {
            safe_detail(entry.error.as_deref().unwrap_or(&entry.detail))
        };
        if entry.kind == CustomizationKind::Rules {
            instructions.push(DoctorInstructionRow {
                name: for_display(&entry.name),
                scope: entry.scope,
                status,
                detail,
            });
        } else {
            extensions.push(DoctorExtensionRow {
                kind: entry.kind,
                name: for_display(&entry.name),
                scope: entry.scope,
                status,
                detail,
            });
        }
    }
    DoctorSnapshot {
        schema_version: 1,
        cwd: for_display(&cwd.to_string_lossy()),
        engines,
        instructions,
        extensions,
        context_telemetry: DoctorContextTelemetry {
            source: "recorded-log",
            sessions: telemetry,
        },
        truncated: inventory.truncated,
    }
}

/// One engine's row from whether its CLI is installed and what its own
/// status command (or app-server) says about signing in.
#[must_use]
pub fn engine_auth_row(
    engine: &str,
    installed: bool,
    auth: Option<EngineAuth>,
    sign_in_command: &str,
) -> DoctorEngineRow {
    let (status, detail) = match (installed, auth) {
        (false, _) => (DoctorStatus::Failed, "not installed".to_owned()),
        (true, Some(EngineAuth::SignedIn)) => (DoctorStatus::Ready, "signed in".to_owned()),
        (true, Some(EngineAuth::SignedOut)) => (
            DoctorStatus::NeedsAuth,
            format!("not signed in; run `{sign_in_command}`"),
        ),
        (true, None) => (
            DoctorStatus::Unknown,
            "sign-in state not reported".to_owned(),
        ),
    };
    DoctorEngineRow {
        engine: engine.to_owned(),
        status,
        detail,
    }
}

/// Replaces the snapshot's rows for the engines in `rows`, in place.
pub fn apply_engine_rows(snapshot: &mut DoctorSnapshot, rows: Vec<DoctorEngineRow>) {
    for row in rows {
        if let Some(existing) = snapshot
            .engines
            .iter_mut()
            .find(|existing| existing.engine == row.engine)
        {
            *existing = row;
        }
    }
}

/// The CLI each engine signs in with, as a user would type it.
#[must_use]
pub fn sign_in_command(engine: &str) -> Option<&'static str> {
    match engine {
        "claude" => Some("claude auth login"),
        "codex" => Some("codex login"),
        "acp:grok" => Some("grok login"),
        _ => None,
    }
}

/// An engine row from a live probe (the TUI holds them): what its CLI said
/// about signing in, or why it did not answer.
#[must_use]
pub fn engine_probe_row(
    engine: &str,
    auth: Option<EngineAuth>,
    answered: bool,
    error: Option<&str>,
) -> DoctorEngineRow {
    if !answered {
        return DoctorEngineRow {
            engine: engine.to_owned(),
            status: DoctorStatus::Failed,
            detail: safe_detail(error.unwrap_or("did not answer")),
        };
    }
    engine_auth_row(
        engine,
        true,
        auth,
        sign_in_command(engine).unwrap_or("its sign-in"),
    )
}

#[must_use]
pub fn render(snapshot: &DoctorSnapshot, format: OutputFormat) -> String {
    if format == OutputFormat::Json {
        let mut output = serde_json::to_string(snapshot).unwrap_or_else(|_| "{}".to_owned());
        output.push('\n');
        return output;
    }
    let mut lines = vec![format!("cwd {}", snapshot.cwd), "engines:".to_owned()];
    lines.extend(snapshot.engines.iter().map(|row| {
        format!("  {} {} {}", row.engine, row.status.as_str(), row.detail)
            .trim_end()
            .to_owned()
    }));
    lines.push("instructions:".to_owned());
    if snapshot.instructions.is_empty() {
        lines.push("  (none)".to_owned());
    } else {
        lines.extend(snapshot.instructions.iter().map(|row| {
            format!(
                "  {}/{} {} {}",
                scope(row.scope),
                row.name,
                row.status.as_str(),
                row.detail
            )
            .trim_end()
            .to_owned()
        }));
    }
    lines.push("extensions:".to_owned());
    if snapshot.extensions.is_empty() {
        lines.push("  (none)".to_owned());
    } else {
        lines.extend(snapshot.extensions.iter().map(|row| {
            format!(
                "  {}/{} {} {} {}",
                scope(row.scope),
                kind(row.kind),
                row.name,
                row.status.as_str(),
                row.detail
            )
            .trim_end()
            .to_owned()
        }));
    }
    lines.push("context telemetry (recorded snapshots; not a live probe):".to_owned());
    if snapshot.context_telemetry.sessions.is_empty() {
        lines.push("  (none recorded)".to_owned());
    } else {
        lines.extend(
            snapshot
                .context_telemetry
                .sessions
                .iter()
                .flat_map(context_row_lines),
        );
    }
    if snapshot.truncated {
        lines.push("inventory truncated at configured safety limits".to_owned());
    }
    format!("{}\n", lines.join("\n"))
}

/// Reads only the bounded, owner-private context telemetry log. Invalid lines
/// are skipped exactly like the legacy reader; the operation never probes a
/// provider or rewrites the log.
#[must_use]
pub fn read_context_telemetry(logs: &Path) -> Vec<DoctorContextRow> {
    let Ok(directory) = interface_storage::PrivateDirectory::open(logs) else {
        return Vec::new();
    };
    let Ok(Some(text)) = read_private_tail(
        &directory,
        "context-telemetry.jsonl",
        CONTEXT_TELEMETRY_MAX_CHARACTERS,
    ) else {
        return Vec::new();
    };
    let mut groups: Vec<(
        String,
        Option<ContextTelemetryRecord>,
        ContextTelemetryRecord,
    )> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let Some(record) = parse_context_telemetry_line(line) else {
            continue;
        };
        let previous = groups
            .iter()
            .position(|(key, _, _)| key == &record.key)
            .map(|index| groups.remove(index));
        let startup = previous
            .as_ref()
            .and_then(|(_, startup, _)| startup.clone())
            .or_else(|| (record.phase == ContextTelemetryPhase::Startup).then(|| record.clone()));
        groups.push((record.key.clone(), startup, record));
    }
    groups
        .into_iter()
        .rev()
        .take(5)
        .map(|(_, startup, latest)| DoctorContextRow {
            session_key: latest.key,
            engine: latest.engine,
            model: latest
                .model
                .clone()
                .or_else(|| startup.as_ref().and_then(|record| record.model.clone())),
            scope: latest
                .scope
                .clone()
                .or_else(|| startup.as_ref().and_then(|record| record.scope.clone())),
            startup_used: startup.as_ref().map(|record| record.used),
            startup_size: startup.as_ref().map(|record| record.size),
            used: latest.used,
            size: latest.size,
            percent: latest.percent,
            recorded_at: latest.at,
            categories: latest.categories,
        })
        .collect()
}

fn context_row_lines(row: &DoctorContextRow) -> Vec<String> {
    let mut parts = vec![format!("  {}", row.session_key)];
    if let (Some(used), Some(size)) = (row.startup_used, row.startup_size) {
        parts.push(format!("startup {used}/{size}"));
    }
    parts.push(format!("latest {}/{}", row.used, row.size));
    if let Some(percent) = row.percent {
        parts.push(format!("{percent}%"));
    }
    if let Some(model) = &row.model {
        parts.push(format!("model {model}"));
    }
    parts.push(format!(
        "scope {}",
        row.scope.as_deref().unwrap_or("unrecorded")
    ));
    let mut lines = vec![parts.join(" ")];
    if let Some(categories) = &row.categories {
        lines.push(format!(
            "    categories: {}",
            categories
                .iter()
                .map(|category| format!("{} {}", category.name, category.tokens))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    lines
}

fn safe_detail(value: &str) -> String {
    let cleaned = for_display(value);
    let lower = cleaned.to_ascii_lowercase();
    let credential_like = [
        "api_key",
        "api-key",
        "authorization",
        "password",
        "secret",
        "token",
        "credential",
        "private_key",
        "private-key",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    if credential_like && (cleaned.contains(':') || cleaned.contains('=')) {
        "redacted".to_owned()
    } else {
        cleaned
    }
}

const fn scope(scope: CustomizationScope) -> &'static str {
    match scope {
        CustomizationScope::Project => "project",
        CustomizationScope::User => "user",
    }
}

const fn kind(kind: CustomizationKind) -> &'static str {
    match kind {
        CustomizationKind::Rules => "rules",
        CustomizationKind::Skill => "skill",
        CustomizationKind::Hook => "hook",
        CustomizationKind::Mcp => "mcp",
        CustomizationKind::Command => "command",
    }
}

#[cfg(test)]
mod tests {
    use interface_contracts::ipc::{
        CustomizationEntry, CustomizationInventory, CustomizationKind, CustomizationScope,
    };

    use super::*;

    #[test]
    fn golden_text_and_json_match_legacy_wire() {
        let inventory = CustomizationInventory {
            root: "/repo".into(),
            entries: vec![
                CustomizationEntry {
                    kind: CustomizationKind::Rules,
                    name: "AGENTS.md".into(),
                    detail: "Project rules".into(),
                    scope: CustomizationScope::Project,
                    source_path: "/repo/AGENTS.md".into(),
                    error: None,
                },
                CustomizationEntry {
                    kind: CustomizationKind::Hook,
                    name: "PreToolUse".into(),
                    detail: "must stay hidden".into(),
                    scope: CustomizationScope::User,
                    source_path: "/user/settings.json".into(),
                    error: None,
                },
            ],
            truncated: false,
        };
        let snapshot = snapshot_from_inventory(Path::new("/repo"), inventory, Vec::new());
        assert_eq!(
            render(&snapshot, OutputFormat::Text),
            "cwd /repo\nengines:\n  claude unknown not probed\n  codex unknown not probed\n  acp:grok unknown not probed\n  acp:qwen unknown not probed\ninstructions:\n  project/AGENTS.md unknown Project rules\nextensions:\n  user/hook PreToolUse unknown configured; command and arguments hidden\ncontext telemetry (recorded snapshots; not a live probe):\n  (none recorded)\n"
        );
        let json = render(&snapshot, OutputFormat::Json);
        assert!(json.starts_with("{\"schemaVersion\":1,\"cwd\":\"/repo\""));
        assert!(json.ends_with('\n'));
    }

    #[test]
    fn doctor_reports_needs_auth_for_a_signed_out_cli() {
        let mut snapshot = snapshot_from_inventory(
            Path::new("/repo"),
            CustomizationInventory {
                root: "/repo".into(),
                entries: Vec::new(),
                truncated: false,
            },
            Vec::new(),
        );
        apply_engine_rows(
            &mut snapshot,
            vec![
                engine_auth_row(
                    "claude",
                    true,
                    Some(EngineAuth::SignedOut),
                    "claude auth login",
                ),
                engine_auth_row("codex", true, Some(EngineAuth::SignedIn), "codex login"),
                engine_auth_row("acp:grok", false, None, "grok login"),
            ],
        );
        let text = render(&snapshot, OutputFormat::Text);
        assert!(
            text.contains("  claude needs-auth not signed in; run `claude auth login`\n"),
            "{text}"
        );
        assert!(text.contains("  codex ready signed in\n"), "{text}");
        assert!(text.contains("  acp:grok failed not installed\n"), "{text}");
        assert!(text.contains("  acp:qwen unknown not probed\n"), "{text}");
        let json = render(&snapshot, OutputFormat::Json);
        assert!(
            json.contains("\"engine\":\"claude\",\"status\":\"needs-auth\""),
            "{json}"
        );
        assert_eq!(
            engine_auth_row("codex", true, None, "codex login").status,
            DoctorStatus::Unknown
        );
        assert_eq!(sign_in_command("acp:qwen"), None);
    }

    #[test]
    fn credential_shaped_detail_is_redacted() {
        assert_eq!(safe_detail("token=oops"), "redacted");
    }

    #[cfg(unix)]
    #[test]
    fn context_log_is_bounded_grouped_and_read_only() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("tempdir");
        let logs = temporary.path().join("logs");
        fs::create_dir(&logs).expect("logs");
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700)).expect("logs mode");
        let path = logs.join("context-telemetry.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"v\":1,\"at\":\"a\",\"key\":\"claude#1\",\"engine\":\"claude\",\"used\":1,\"size\":10,\"percent\":10,\"phase\":\"startup\"}\n",
                "not-json\n",
                "{\"v\":1,\"at\":\"b\",\"key\":\"claude#1\",\"engine\":\"claude\",\"model\":\"safe\",\"used\":4,\"size\":10,\"percent\":40,\"phase\":\"turn\"}\n"
            ),
        )
        .expect("telemetry");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("file mode");
        let before = fs::read(&path).expect("before");
        let rows = read_context_telemetry(&logs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].startup_used, Some(1));
        assert_eq!(rows[0].used, 4);
        assert_eq!(rows[0].model.as_deref(), Some("safe"));
        assert_eq!(fs::read(path).expect("after"), before);
    }

    #[cfg(unix)]
    #[test]
    fn context_log_reads_the_tail_when_the_file_exceeds_the_character_window() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("tempdir");
        let logs = temporary.path().join("logs");
        fs::create_dir(&logs).expect("logs");
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700)).expect("logs mode");
        let path = logs.join("context-telemetry.jsonl");
        let content = format!(
            "{}\n{{\"v\":1,\"at\":\"tail\",\"key\":\"codex#tail\",\"engine\":\"codex\",\"used\":9,\"size\":10,\"percent\":90,\"phase\":\"turn\"}}\n",
            "x".repeat(CONTEXT_TELEMETRY_MAX_CHARACTERS + 4_096)
        );
        fs::write(&path, content).expect("telemetry");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("file mode");

        let rows = read_context_telemetry(&logs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_key, "codex#tail");
        assert_eq!(rows[0].used, 9);
    }

    #[cfg(unix)]
    #[test]
    fn doctor_redacts_word_split_credentials_not_substrings() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("tempdir");
        let logs = temporary.path().join("logs");
        fs::create_dir(&logs).expect("logs");
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700)).expect("logs mode");
        let path = logs.join("context-telemetry.jsonl");
        fs::write(
            &path,
            concat!(
                "{\"v\":1,\"at\":\"a\",\"key\":\"claude#api\",\"engine\":\"claude\",\"model\":\"apiKey: sk-secret\",\"used\":1,\"size\":10,\"percent\":10,\"phase\":\"turn\"}\n",
                "{\"v\":1,\"at\":\"b\",\"key\":\"claude#tok\",\"engine\":\"claude\",\"model\":\"mytokenizer: x\",\"used\":2,\"size\":10,\"percent\":20,\"phase\":\"turn\"}\n"
            ),
        )
        .expect("telemetry");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("file mode");

        let rows = read_context_telemetry(&logs);
        let api = rows
            .iter()
            .find(|row| row.session_key == "claude#api")
            .expect("api row");
        let tokenizer = rows
            .iter()
            .find(|row| row.session_key == "claude#tok")
            .expect("tokenizer row");
        assert_eq!(api.model.as_deref(), Some("redacted"));
        assert_eq!(tokenizer.model.as_deref(), Some("mytokenizer: x"));
    }

    #[cfg(unix)]
    #[test]
    fn doctor_reads_float_percent_from_the_jsonl_file() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().expect("tempdir");
        let logs = temporary.path().join("logs");
        fs::create_dir(&logs).expect("logs");
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700)).expect("logs mode");
        let path = logs.join("context-telemetry.jsonl");
        fs::write(
            &path,
            "{\"v\":1,\"at\":\"a\",\"key\":\"claude#1\",\"engine\":\"claude\",\"used\":1,\"size\":10,\"percent\":40.6,\"phase\":\"turn\"}\n",
        )
        .expect("telemetry");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("file mode");

        let rows = read_context_telemetry(&logs);
        assert_eq!(rows[0].percent, Some(41));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn doctor_reads_percent_written_by_the_diagnostics_sink() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use crate::diagnostics::{ContextTelemetryPhase, ContextTelemetrySample, DiagnosticsSink};

        let temporary = tempfile::tempdir().expect("tempdir");
        let logs = temporary.path().join("logs");
        fs::create_dir(&logs).expect("logs");
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o700)).expect("logs mode");
        let sink = DiagnosticsSink::install(&logs).expect("sink");
        sink.record_context(ContextTelemetrySample {
            session_key: "claude#round".into(),
            engine: "claude".into(),
            model: None,
            scope: None,
            used: 42,
            size: 100,
            percent: Some(12.4),
            categories: None,
            phase: ContextTelemetryPhase::Turn,
        });
        sink.flush().await;

        let rows = read_context_telemetry(&logs);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].percent, Some(12));
        assert_eq!(rows[0].used, 42);
        sink.restore().await;
    }
}
