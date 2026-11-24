//! Owner-private CLI diagnostics and bounded context telemetry.
//!
//! Frozen 0.1.169 installs this sink before provider construction. Diagnostic
//! text is sanitized before it reaches either the in-process ring or disk, and
//! context telemetry stores only bounded identifiers and token counts.

use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex, mpsc},
};

use interface_common::sync::lock;
use interface_contracts::canonical_timestamp;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::{
    private_write::{ensure_private_directory, open_owner_append},
    sanitize::for_display,
};

pub const CONTEXT_TELEMETRY_LOG: &str = "context-telemetry.jsonl";
const DEFAULT_CAPACITY: usize = 500;
const DEFAULT_MAX_ENTRY_CHARS: usize = 16_384;
const DEFAULT_MAX_FILE_BYTES: u64 = 1_048_576;
const DEFAULT_MAX_LOG_FILES: usize = 10;
const DEFAULT_MAX_TELEMETRY_RECORDS: usize = 200;
const MAX_TELEMETRY_READ_BYTES: u64 = 262_144;
const TELEMETRY_FIELD_CHARS: usize = 120;
const TELEMETRY_CATEGORY_LIMIT: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticLevel {
    Log,
    Info,
    Warn,
    Error,
}

impl DiagnosticLevel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Log => "LOG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEntry {
    pub timestamp: String,
    pub level: DiagnosticLevel,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextTelemetryPhase {
    Startup,
    Turn,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextTelemetryCategory {
    pub name: String,
    pub tokens: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextTelemetrySample {
    pub session_key: String,
    pub engine: String,
    pub model: Option<String>,
    pub scope: Option<String>,
    pub used: u64,
    pub size: u64,
    pub percent: Option<f64>,
    pub categories: Option<Vec<ContextTelemetryCategory>>,
    pub phase: ContextTelemetryPhase,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ContextTelemetryRecord {
    pub v: u8,
    pub at: String,
    pub key: String,
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub used: u64,
    pub size: u64,
    pub percent: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<ContextTelemetryCategory>>,
    pub phase: ContextTelemetryPhase,
}

#[derive(Clone)]
pub struct DiagnosticsOptions {
    pub capacity: usize,
    pub max_entry_chars: usize,
    pub max_file_bytes: u64,
    pub max_log_files: usize,
    pub max_telemetry_records: usize,
    pub now: Arc<dyn Fn() -> String + Send + Sync>,
}

impl Default for DiagnosticsOptions {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            max_entry_chars: DEFAULT_MAX_ENTRY_CHARS,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_log_files: DEFAULT_MAX_LOG_FILES,
            max_telemetry_records: DEFAULT_MAX_TELEMETRY_RECORDS,
            now: Arc::new(|| canonical_timestamp(chrono::Utc::now())),
        }
    }
}

#[derive(Clone)]
pub struct DiagnosticsSink {
    inner: Arc<DiagnosticsInner>,
}

struct DiagnosticsInner {
    entries: Mutex<VecDeque<DiagnosticEntry>>,
    reserved_file_bytes: Mutex<u64>,
    sender: mpsc::Sender<WriterCommand>,
    capacity: usize,
    max_entry_chars: usize,
    max_file_bytes: u64,
    now: Arc<dyn Fn() -> String + Send + Sync>,
}

enum WriterCommand {
    Append(String),
    AppendTelemetry(ContextTelemetryRecord),
    Flush(oneshot::Sender<()>),
    Close(oneshot::Sender<()>),
}

impl DiagnosticsSink {
    /// Installs the process sink and opens the telemetry writer before any
    /// provider can emit or launch.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, invalid bounds, or a log open failure.
    pub fn install(logs_directory: &Path) -> Result<Self, String> {
        Self::install_with_options(logs_directory, DiagnosticsOptions::default())
    }

    fn install_with_options(
        logs_directory: &Path,
        options: DiagnosticsOptions,
    ) -> Result<Self, String> {
        validate_options(&options)?;
        ensure_private_directory(logs_directory)?;
        prune_settled_logs(logs_directory, options.max_log_files);
        let now = (options.now)();
        let stamp = now.replace([':', '.'], "-");
        let log_file = logs_directory.join(format!("iface-{stamp}-{}.log", std::process::id()));
        let diagnostic = open_owner_append(&log_file)?;
        let reserved_file_bytes = diagnostic
            .metadata()
            .map_err(|error| error.to_string())?
            .len();
        let telemetry_file = logs_directory.join(CONTEXT_TELEMETRY_LOG);
        let (mut telemetry, whole_file) = read_context_telemetry(logs_directory);
        if telemetry.len() > options.max_telemetry_records {
            telemetry.drain(..telemetry.len() - options.max_telemetry_records);
        }
        if !telemetry.is_empty() {
            let _ = persist_telemetry(&telemetry_file, &telemetry, whole_file.as_deref());
        }
        let telemetry_handle = open_owner_append(&telemetry_file)?;
        let (sender, receiver) = mpsc::channel();
        // The writer owns std::fs::File handles: keep it on a plain thread so the
        // fsyncs in Flush/Close never block a tokio worker.
        std::thread::spawn(move || writer_loop(receiver, diagnostic, telemetry_handle));
        Ok(Self {
            inner: Arc::new(DiagnosticsInner {
                entries: Mutex::new(VecDeque::new()),
                reserved_file_bytes: Mutex::new(reserved_file_bytes),
                sender,
                capacity: options.capacity,
                max_entry_chars: options.max_entry_chars,
                max_file_bytes: options.max_file_bytes,
                now: options.now,
            }),
        })
    }

    pub fn record(&self, level: DiagnosticLevel, text: impl AsRef<str>) {
        let timestamp = (self.inner.now)();
        let text = bounded_text(&for_display(text.as_ref()), self.inner.max_entry_chars);
        let entry = DiagnosticEntry {
            timestamp: timestamp.clone(),
            level,
            text: text.clone(),
        };
        let mut entries = lock(&self.inner.entries);
        entries.push_back(entry);
        while entries.len() > self.inner.capacity {
            entries.pop_front();
        }
        drop(entries);
        let line = format!(
            "{timestamp} {} {}\n",
            level.label(),
            persistence_text(&text)
        );
        let line_bytes = u64::try_from(line.len()).unwrap_or(u64::MAX);
        let mut reserved = lock(&self.inner.reserved_file_bytes);
        if reserved.saturating_add(line_bytes) <= self.inner.max_file_bytes {
            *reserved = reserved.saturating_add(line_bytes);
            let _ = self.inner.sender.send(WriterCommand::Append(line));
        }
    }

    #[must_use]
    pub fn entries(&self) -> Vec<DiagnosticEntry> {
        lock(&self.inner.entries).iter().cloned().collect()
    }

    pub fn record_context(&self, sample: ContextTelemetrySample) {
        let Some(record) = context_record(sample, &(self.inner.now)()) else {
            return;
        };
        let _ = self
            .inner
            .sender
            .send(WriterCommand::AppendTelemetry(record));
    }

    pub async fn flush(&self) {
        let (done, wait) = oneshot::channel();
        if self.inner.sender.send(WriterCommand::Flush(done)).is_ok() {
            let _ = wait.await;
        }
    }

    pub async fn restore(&self) {
        let (done, wait) = oneshot::channel();
        if self.inner.sender.send(WriterCommand::Close(done)).is_ok() {
            let _ = wait.await;
        }
    }
}

fn writer_loop(receiver: mpsc::Receiver<WriterCommand>, mut diagnostic: File, mut telemetry: File) {
    for command in receiver {
        match command {
            WriterCommand::Append(line) => {
                let _ = diagnostic.write_all(line.as_bytes());
            }
            WriterCommand::AppendTelemetry(record) => {
                let _ = append_telemetry(&mut telemetry, &record);
            }
            WriterCommand::Flush(done) => {
                let _ = diagnostic.sync_all();
                let _ = telemetry.sync_all();
                let _ = done.send(());
            }
            WriterCommand::Close(done) => {
                let _ = diagnostic.sync_all();
                let _ = telemetry.sync_all();
                let _ = done.send(());
                break;
            }
        }
    }
}

fn context_record(
    sample: ContextTelemetrySample,
    fallback_at: &str,
) -> Option<ContextTelemetryRecord> {
    let key = telemetry_field(&sample.session_key)?;
    let engine = telemetry_field(&sample.engine)?;
    if sample.size == 0 {
        return None;
    }
    let categories = sample.categories.and_then(|rows| {
        let selected = rows
            .into_iter()
            .take(TELEMETRY_CATEGORY_LIMIT)
            .filter_map(|row| {
                Some(ContextTelemetryCategory {
                    name: telemetry_field(&row.name)?,
                    tokens: row.tokens,
                })
            })
            .collect::<Vec<_>>();
        (!selected.is_empty()).then_some(selected)
    });
    Some(ContextTelemetryRecord {
        v: 1,
        at: telemetry_field(fallback_at)?,
        key,
        engine,
        model: sample.model.as_deref().and_then(telemetry_field),
        scope: sample.scope.as_deref().and_then(telemetry_field),
        used: sample.used,
        size: sample.size,
        percent: sample
            .percent
            .and_then(|value| rounded_u64(value.clamp(0.0, 100.0))),
        categories,
        phase: sample.phase,
    })
}

/// The valid records in the newest `MAX_TELEMETRY_READ_BYTES` of the log and,
/// when that read began at the start of the file, the bytes it read.
fn read_context_telemetry(logs_directory: &Path) -> (Vec<ContextTelemetryRecord>, Option<Vec<u8>>) {
    let path = logs_directory.join(CONTEXT_TELEMETRY_LOG);
    if path
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return (Vec::new(), None);
    }
    let Ok(mut file) = File::open(path) else {
        return (Vec::new(), None);
    };
    let Ok(length) = file.metadata().map(|metadata| metadata.len()) else {
        return (Vec::new(), None);
    };
    let offset = length.saturating_sub(MAX_TELEMETRY_READ_BYTES);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return (Vec::new(), None);
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_TELEMETRY_READ_BYTES)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return (Vec::new(), None);
    }
    let records = String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(parse_context_telemetry_line)
        .collect();
    (records, (offset == 0).then_some(bytes))
}

fn valid_context_record(value: &serde_json::Value) -> Option<ContextTelemetryRecord> {
    let object = value.as_object()?;
    let key = telemetry_field(object.get("key")?.as_str()?)?;
    let engine = telemetry_field(object.get("engine")?.as_str()?)?;
    let at = telemetry_field(object.get("at")?.as_str()?)?;
    let used = json_tokens(object.get("used")?)?;
    let size = json_tokens(object.get("size")?)?;
    if size == 0 {
        return None;
    }
    let categories = object.get("categories").and_then(|value| {
        let rows = value
            .as_array()?
            .iter()
            .take(TELEMETRY_CATEGORY_LIMIT)
            .filter_map(|row| {
                let row = row.as_object()?;
                Some(ContextTelemetryCategory {
                    name: telemetry_field(row.get("name")?.as_str()?)?,
                    tokens: json_tokens(row.get("tokens")?)?,
                })
            })
            .collect::<Vec<_>>();
        (!rows.is_empty()).then_some(rows)
    });
    Some(ContextTelemetryRecord {
        v: 1,
        at,
        key,
        engine,
        model: object
            .get("model")
            .and_then(serde_json::Value::as_str)
            .and_then(telemetry_field),
        scope: object
            .get("scope")
            .and_then(serde_json::Value::as_str)
            .and_then(telemetry_field),
        used,
        size,
        percent: object
            .get("percent")
            .and_then(serde_json::Value::as_f64)
            .and_then(|value| rounded_u64(value.clamp(0.0, 100.0))),
        categories,
        phase: if object.get("phase").and_then(serde_json::Value::as_str) == Some("startup") {
            ContextTelemetryPhase::Startup
        } else {
            ContextTelemetryPhase::Turn
        },
    })
}

fn json_tokens(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_f64().and_then(rounded_u64))
}

pub(crate) fn rounded_u64(value: f64) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let rounded = value.round();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss
    )]
    {
        (rounded <= u64::MAX as f64).then_some(rounded as u64)
    }
}

/// Rewrites the log as exactly `records`, unless `current` (the whole file as
/// read) already is those bytes: only a trim, a dropped line or a partial
/// tail rewrites it.
fn persist_telemetry(
    telemetry_file: &Path,
    records: &[ContextTelemetryRecord],
    current: Option<&[u8]>,
) -> Result<(), String> {
    let mut bytes = Vec::new();
    for record in records {
        serde_json::to_writer(&mut bytes, record).map_err(|error| error.to_string())?;
        bytes.extend_from_slice(b"\n");
    }
    if current == Some(bytes.as_slice()) {
        return Ok(());
    }
    crate::private_write::write_private_bytes(telemetry_file, &bytes)
}

fn append_telemetry(file: &mut File, record: &ContextTelemetryRecord) -> Result<(), String> {
    serde_json::to_writer(&mut *file, record).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())
}

pub(crate) fn parse_context_telemetry_line(line: &str) -> Option<ContextTelemetryRecord> {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| valid_context_record(&value))
}

fn validate_options(options: &DiagnosticsOptions) -> Result<(), String> {
    if options.capacity == 0
        || options.max_entry_chars == 0
        || options.max_file_bytes == 0
        || options.max_log_files == 0
        || options.max_telemetry_records == 0
    {
        return Err("Diagnostics bounds must be positive integers".into());
    }
    Ok(())
}

fn prune_settled_logs(directory: &Path, maximum: usize) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let pid = diagnostic_log_pid(&name)?;
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then_some((entry.path(), pid, metadata.modified().ok()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.2.cmp(&left.2));
    let mut retained = 0usize;
    for (file, pid, _) in candidates {
        if retained < maximum.saturating_sub(1) || crate::background::process_is_alive(pid) {
            retained = retained.saturating_add(1);
        } else {
            let _ = fs::remove_file(file);
        }
    }
}

fn diagnostic_log_pid(name: &str) -> Option<u32> {
    let stem = name.strip_prefix("iface-")?.strip_suffix(".log")?;
    stem.rsplit_once('-')?.1.parse().ok()
}

fn telemetry_field(value: &str) -> Option<String> {
    let safe = bounded_text(for_display(value).trim(), TELEMETRY_FIELD_CHARS);
    if safe.is_empty() {
        return None;
    }
    if (safe.contains(':') || safe.contains('=')) && credential_shaped(&safe) {
        return Some("redacted".into());
    }
    Some(safe)
}

fn credential_shaped(value: &str) -> bool {
    value
        .to_ascii_lowercase()
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '-'
        })
        .map(|word| word.replace(['_', '-'], ""))
        .any(|word| {
            matches!(
                word.as_str(),
                "apikey"
                    | "authorization"
                    | "password"
                    | "secret"
                    | "token"
                    | "credential"
                    | "privatekey"
            )
        })
}

fn bounded_text(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    if maximum == 1 {
        return "…".into();
    }
    value
        .chars()
        .take(maximum - 1)
        .chain(std::iter::once('…'))
        .collect()
}

fn persistence_text(value: &str) -> String {
    value.replace('\t', "  ").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn options(now: &str) -> DiagnosticsOptions {
        let now = now.to_owned();
        DiagnosticsOptions {
            capacity: 2,
            max_entry_chars: 12,
            max_file_bytes: 1_024,
            max_log_files: 2,
            max_telemetry_records: 2,
            now: Arc::new(move || now.clone()),
        }
    }

    #[tokio::test]
    async fn diagnostics_and_telemetry_are_sanitized_bounded_and_owner_private() {
        let temp = tempfile::tempdir().expect("temp");
        let logs = temp.path().join("logs");
        let sink =
            DiagnosticsSink::install_with_options(&logs, options("2026-08-27T00:00:00.000Z"))
                .expect("sink");
        sink.record(DiagnosticLevel::Warn, "first\nline\u{1b}[31m");
        sink.record_context(ContextTelemetrySample {
            session_key: "claude#one".into(),
            engine: "claude".into(),
            model: Some("api_key=do-not-store".into()),
            scope: Some("project".into()),
            used: 120,
            size: 1_000,
            percent: Some(12.4),
            categories: Some(vec![ContextTelemetryCategory {
                name: "system".into(),
                tokens: 20,
            }]),
            phase: ContextTelemetryPhase::Startup,
        });
        sink.flush().await;

        assert_eq!(sink.entries().len(), 1);
        assert!(!sink.entries()[0].text.contains('\u{1b}'));
        let (records, _) = read_context_telemetry(&logs);
        assert_eq!(records[0].model.as_deref(), Some("redacted"));
        assert_eq!(records[0].percent, Some(12));
        let log_file = logs.join(format!(
            "iface-2026-08-27T00-00-00-000Z-{}.log",
            std::process::id()
        ));
        assert_eq!(
            fs::metadata(&log_file)
                .expect("diagnostic metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&logs)
                .expect("logs metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let persisted =
            fs::read_to_string(logs.join(CONTEXT_TELEMETRY_LOG)).expect("persisted telemetry");
        assert!(!persisted.contains("do-not-store"));
        sink.restore().await;
    }

    #[tokio::test]
    async fn telemetry_rewrites_atomically_and_retains_the_newest_records() {
        let temp = tempfile::tempdir().expect("temp");
        let sink = DiagnosticsSink::install_with_options(
            &temp.path().join("logs"),
            options("2026-08-27T00:00:00.000Z"),
        )
        .expect("sink");
        for index in 0..3 {
            sink.record_context(ContextTelemetrySample {
                session_key: format!("claude#{index}"),
                engine: "claude".into(),
                model: None,
                scope: None,
                used: index,
                size: 100,
                percent: None,
                categories: None,
                phase: ContextTelemetryPhase::Turn,
            });
        }
        sink.flush().await;
        sink.restore().await;

        // Retention is applied to the durable file on install, and that rewrite is
        // atomic: no temporary is left behind.
        let reopened = DiagnosticsSink::install_with_options(
            &temp.path().join("logs"),
            options("2026-08-27T00:00:00.000Z"),
        )
        .expect("reopened sink");
        let (records, _) = read_context_telemetry(&temp.path().join("logs"));
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].key, "claude#1");
        assert_eq!(records[1].key, "claude#2");
        assert!(
            fs::read_dir(temp.path().join("logs"))
                .expect("logs")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
        reopened.restore().await;
    }

    #[tokio::test]
    async fn telemetry_already_within_retention_is_not_rewritten_on_launch() {
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().expect("temp");
        let logs = temp.path().join("logs");
        let sink =
            DiagnosticsSink::install_with_options(&logs, options("2026-08-27T00:00:00.000Z"))
                .expect("sink");
        for index in 0..2 {
            sink.record_context(ContextTelemetrySample {
                session_key: format!("claude#{index}"),
                engine: "claude".into(),
                model: None,
                scope: None,
                used: index,
                size: 100,
                percent: Some(12.4),
                categories: None,
                phase: ContextTelemetryPhase::Turn,
            });
        }
        sink.flush().await;
        sink.restore().await;
        let path = logs.join(CONTEXT_TELEMETRY_LOG);
        let identity = || {
            let metadata = fs::metadata(&path).expect("telemetry metadata");
            (
                metadata.ino(),
                metadata.modified().expect("telemetry mtime"),
            )
        };
        let before = (identity(), fs::read(&path).expect("telemetry bytes"));

        let reopened =
            DiagnosticsSink::install_with_options(&logs, options("2026-08-27T00:00:00.000Z"))
                .expect("reopened sink");
        assert_eq!(
            (identity(), fs::read(&path).expect("telemetry bytes")),
            before
        );
        let (records, whole_file) = read_context_telemetry(&logs);
        assert_eq!(records.len(), 2);
        assert_eq!(whole_file.as_deref(), Some(before.1.as_slice()));
        reopened.restore().await;
    }

    #[test]
    fn rounded_u64_rejects_non_finite_and_rounds_half_away_from_zero() {
        assert_eq!(rounded_u64(12.4), Some(12));
        assert_eq!(rounded_u64(2.5), Some(3));
        assert_eq!(rounded_u64(0.0), Some(0));
        assert_eq!(rounded_u64(-1.0), None);
        assert_eq!(rounded_u64(f64::NAN), None);
        assert_eq!(rounded_u64(f64::INFINITY), None);
    }

    #[tokio::test]
    async fn written_percent_is_an_integer_not_null() {
        let temp = tempfile::tempdir().expect("temp");
        let logs = temp.path().join("logs");
        let sink =
            DiagnosticsSink::install_with_options(&logs, options("2026-08-27T00:00:00.000Z"))
                .expect("sink");
        sink.record_context(ContextTelemetrySample {
            session_key: "claude#percent".into(),
            engine: "claude".into(),
            model: None,
            scope: None,
            used: 42,
            size: 100,
            percent: Some(40.6),
            categories: None,
            phase: ContextTelemetryPhase::Turn,
        });
        sink.flush().await;
        let persisted =
            fs::read_to_string(logs.join(CONTEXT_TELEMETRY_LOG)).expect("persisted telemetry");
        assert!(
            persisted.contains("\"percent\":41"),
            "percent should round to an integer: {persisted}"
        );
        assert!(
            !persisted.contains("\"percent\":null"),
            "percent must not be null: {persisted}"
        );
        sink.restore().await;
    }

    #[tokio::test]
    async fn symlinked_diagnostics_directory_is_refused_without_following_it() {
        let temp = tempfile::tempdir().expect("temp");
        let target = temp.path().join("target");
        fs::create_dir(&target).expect("target");
        let link = temp.path().join("logs");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let result =
            DiagnosticsSink::install_with_options(&link, options("2026-08-27T00:00:00.000Z"));

        assert!(result.is_err());
        assert!(
            fs::read_dir(target)
                .expect("target untouched")
                .next()
                .is_none()
        );
    }
}
