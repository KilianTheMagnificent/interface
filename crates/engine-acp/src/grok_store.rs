use std::{
    borrow::Cow,
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use interface_common::time::{IsoFraction, days_from_civil, unix_millis_iso};
use interface_contracts::{
    engine::{
        EngineEvent, EngineEventKind, EngineId, EngineUsage, SessionOrigin, SessionStatus,
        SessionSummary, UsageWindow,
    },
    ipc::{HistoryPage, unavailable_history},
};
use interface_engine::FileVersion;
use serde_json::{Map, Value, json};

const MAX_TITLE_CHARS: usize = 80;
const MAX_SUMMARY_BYTES: u64 = 1024 * 1024;
const MAX_HISTORY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

#[must_use]
pub fn default_sessions_root() -> PathBuf {
    let home = std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".grok"))
        })
        .unwrap_or_else(|| PathBuf::from(".grok"));
    home.join("sessions")
}

/// The fields a listing row derives from one `summary.json`, parsed once per
/// file version. The clock fallback for a missing activity time is applied
/// per listing, never cached.
#[derive(Clone)]
struct SummaryFields {
    title: Option<String>,
    id: Option<String>,
    cwd: Option<String>,
    last_activity: Option<String>,
    first_seen: Option<String>,
    model: Option<String>,
    title_from_summary: bool,
}

/// Parsed `summary.json` fields keyed by file identity, so a listing that
/// finds a file unchanged skips its read and parse. Holds only the paths
/// seen by the latest listing.
#[derive(Default)]
pub struct GrokListingCache {
    entries: std::collections::HashMap<PathBuf, (FileVersion, Option<SummaryFields>)>,
}

/// # Errors
///
/// Fails only when the sessions root itself cannot be listed.
pub fn list_sessions_cached(
    root: &Path,
    engine: &EngineId,
    cache: &mut GrokListingCache,
) -> std::io::Result<Vec<SessionSummary>> {
    let projects = match safe_directories(root) {
        Ok(projects) => projects,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            cache.entries.clear();
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    let mut previous = std::mem::take(&mut cache.entries);
    let mut output = Vec::new();
    for project in projects {
        for session in safe_directories(&project).unwrap_or_default() {
            let path = session.join("summary.json");
            let Ok(metadata) = safe_file_metadata(&path) else {
                continue;
            };
            let version = FileVersion::from_metadata(&metadata);
            let fields = match previous.remove(&path) {
                Some((cached, fields)) if cached == version => fields,
                _ => match read_summary(&path, &metadata) {
                    Ok(summary) => summary.map(|summary| summary_fields(&summary)),
                    // Unreadable right now: skip it and read again next time.
                    Err(()) => continue,
                },
            };
            cache.entries.insert(path, (version, fields.clone()));
            let Some(fields) = fields else {
                continue;
            };
            let Some(title) = fields.title else {
                continue;
            };
            let session_id = fields
                .id
                .as_deref()
                .or_else(|| session.file_name().and_then(|name| name.to_str()))
                .unwrap_or_default()
                .to_owned();
            if !usable_session_id(&session_id) {
                continue;
            }
            let cwd = fields
                .cwd
                .or_else(|| {
                    project
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(percent_decode)
                })
                .unwrap_or_default();
            let last_activity = fields.last_activity.unwrap_or_else(now_iso);
            output.push(SessionSummary {
                engine: engine.clone(),
                session_id,
                title,
                cwd,
                status: SessionStatus::Idle,
                cloud: false,
                model: fields.model,
                effort: None,
                last_activity: last_activity.clone(),
                first_seen: fields.first_seen.unwrap_or(last_activity),
                archived: false,
                size: None,
                title_from_summary: Some(fields.title_from_summary),
                origin: Some(SessionOrigin::Store),
                project_root: None,
                worktree_home: None,
                worktree_path: None,
                setting_sources: None,
                disallowed_tools: None,
                kind: None,
                permission_mode: None,
            });
        }
    }
    output.sort_by(|left, right| right.last_activity.cmp(&left.last_activity));
    Ok(output)
}

fn summary_fields(summary: &Map<String, Value>) -> SummaryFields {
    let info = summary.get("info").and_then(Value::as_object);
    SummaryFields {
        title: usable_title(summary),
        id: info
            .and_then(|info| nonempty(info.get("id")))
            .map(ToOwned::to_owned),
        cwd: info
            .and_then(|info| nonempty(info.get("cwd")))
            .map(ToOwned::to_owned),
        last_activity: nonempty(summary.get("last_active_at"))
            .or_else(|| nonempty(summary.get("updated_at")))
            .and_then(normalize_iso),
        first_seen: nonempty(summary.get("created_at")).and_then(normalize_iso),
        model: nonempty(summary.get("current_model_id")).map(ToOwned::to_owned),
        title_from_summary: nonempty(summary.get("generated_title")).is_some()
            || nonempty(summary.get("session_summary")).is_some(),
    }
}

pub fn history(root: &Path, session_id: &str, cwd: &str, limit: usize) -> HistoryPage {
    let Some(directory) = find_session_dir(root, session_id, cwd) else {
        return HistoryPage {
            availability: None,
            reason: None,
            events: Vec::new(),
            messages: Some(0),
            older_messages: 0,
        };
    };
    let Ok((records, total_messages)) =
        read_history_records(&directory.join("chat_history.jsonl"), limit)
    else {
        // An unreadable transcript is reported, never rendered as an empty
        // session: a resume that silently drops the history looks identical to
        // a session that never had one.
        return unavailable_history("Grok could not read the stored transcript.");
    };
    let keep = if limit == 0 {
        total_messages
    } else {
        total_messages.min(limit)
    };
    HistoryPage {
        availability: None,
        reason: None,
        events: history_events(&records),
        messages: Some(keep as u64),
        older_messages: total_messages.saturating_sub(keep) as u64,
    }
}

fn history_events(records: &[Value]) -> Vec<EngineEvent> {
    let mut output = Vec::new();
    for record in records.iter().filter_map(Value::as_object) {
        match nonempty(record.get("type")) {
            Some("user") => {
                if nonempty(record.get("synthetic_reason")).is_some() {
                    continue;
                }
                if let Some(text) = user_text(record) {
                    output.push(event(EngineEventKind::UserMessage {
                        text: text.into_owned(),
                        agent_key: None,
                        images: None,
                        image_references: None,
                    }));
                }
            }
            Some("assistant") => {
                if let Some(text) = flatten_content(record.get("content")) {
                    output.push(event(EngineEventKind::MessageDelta {
                        text: text.into_owned(),
                        agent_key: None,
                    }));
                }
                for call in record
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_object)
                {
                    let (Some(call_id), Some(tool)) =
                        (nonempty(call.get("id")), nonempty(call.get("name")))
                    else {
                        continue;
                    };
                    let input = call
                        .get("arguments")
                        .map_or_else(|| json!({}), parse_arguments);
                    output.push(event(EngineEventKind::ToolCall {
                        call_id: call_id.into(),
                        tool: tool.into(),
                        summary: tool.into(),
                        input,
                        agent_key: None,
                    }));
                }
                output.push(event(EngineEventKind::ThinkingSignal {
                    active: false,
                    agent_key: None,
                }));
            }
            Some("tool_result") => {
                let Some(call_id) = nonempty(record.get("tool_call_id")) else {
                    continue;
                };
                let summary = flatten_content(record.get("content"))
                    .unwrap_or_default()
                    .chars()
                    .take(200)
                    .collect();
                output.push(event(EngineEventKind::ToolResult {
                    call_id: call_id.into(),
                    ok: record.get("is_error").and_then(Value::as_bool) != Some(true),
                    summary,
                    agent_key: None,
                }));
            }
            _ => {}
        }
    }
    output
}

fn event(event: EngineEventKind) -> EngineEvent {
    EngineEvent { at: None, event }
}

pub fn delete(root: &Path, session_id: &str, cwd: &str) -> std::io::Result<()> {
    let directory = find_session_dir(root, session_id, cwd).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Grok session was not found")
    })?;
    if !safe_session_directory(root, &directory, session_id) {
        return Err(std::io::Error::other(
            "Grok session path was not a safe directory",
        ));
    }
    let metadata = fs::symlink_metadata(&directory)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(
            "Grok session path was not a safe directory",
        ));
    }
    let canonical_root = fs::canonicalize(root)?;
    let canonical_directory = fs::canonicalize(&directory)?;
    if canonical_directory == canonical_root
        || !canonical_directory.starts_with(&canonical_root)
        || canonical_directory.parent().and_then(Path::parent) != Some(canonical_root.as_path())
    {
        return Err(std::io::Error::other(
            "Grok session path escaped the sessions root",
        ));
    }
    fs::remove_dir_all(directory)
}

#[must_use]
pub fn billing_usage(payload: &Value, now_ms: i64) -> EngineUsage {
    let root = payload.as_object();
    let billing = root
        .and_then(|root| root.get("result"))
        .and_then(Value::as_object)
        .or(root);
    let config = billing
        .and_then(|billing| billing.get("config"))
        .and_then(Value::as_object);
    // An ABSENT `creditUsagePercent` is zero used, not an unreadable account.
    // x.ai omits the field entirely while the current period has no spend yet
    // and starts sending it again after the first turn — observed flipping to
    // absent at the exact instant of the weekly rollover, with every sibling
    // field (`currentPeriod`, `onDemandUsed`, `subscription_tier`) intact. The
    // old reading blanked the whole Grok row for the first hours of every week
    // and called it "no current subscription usage window", which is the one
    // moment the number is most obviously knowable.
    //
    // ponytail: absent or null only. A field that IS there but unreadable
    // still reports unavailable rather than inventing a zero.
    let percent = match config.and_then(|config| config.get("creditUsagePercent")) {
        None | Some(Value::Null) => Some(0.0),
        Some(reported) => usage_percent(reported),
    };
    let period = config
        .and_then(|config| config.get("currentPeriod"))
        .and_then(Value::as_object);
    let start_ms = period
        .and_then(|period| nonempty(period.get("start")))
        .and_then(parse_iso_ms);
    let end_ms = period
        .and_then(|period| nonempty(period.get("end")))
        .and_then(parse_iso_ms);
    let valid = percent.is_some()
        && start_ms.is_some()
        && end_ms.is_some()
        && start_ms < end_ms
        && start_ms.is_some_and(|start| start <= now_ms)
        && end_ms.is_some_and(|end| end > now_ms);
    if !valid {
        return unavailable_usage("Grok returned no current subscription usage window.");
    }
    let period_type = period
        .and_then(|period| nonempty(period.get("type")))
        .unwrap_or_default()
        .to_ascii_uppercase();
    let label = if period_type.contains("WEEKLY") {
        "Shared weekly pool"
    } else if period_type.contains("MONTHLY") {
        "Shared monthly pool"
    } else {
        "Shared usage pool"
    };
    let plan = billing.and_then(|billing| {
        ["subscription_tier", "subscriptionTier"]
            .into_iter()
            .find_map(|key| nonempty(billing.get(key)))
            .or_else(|| {
                config.and_then(|config| {
                    ["subscription_tier", "subscriptionTier"]
                        .into_iter()
                        .find_map(|key| nonempty(config.get(key)))
                })
            })
    });
    EngineUsage {
        engine: EngineId::new("acp:grok").expect("built-in Grok id"),
        available: true,
        plan: plan.map(ToOwned::to_owned),
        windows: vec![UsageWindow {
            label: label.into(),
            utilization: percent,
            resets_at: end_ms.map(grok_iso),
        }],
        cost_usd: None,
        note: Some("Shared account usage across Grok products.".into()),
        account: None,
        account_label: None,
    }
}

#[must_use]
pub fn unavailable_usage(note: &str) -> EngineUsage {
    EngineUsage {
        engine: EngineId::new("acp:grok").expect("built-in Grok id"),
        available: false,
        plan: None,
        windows: Vec::new(),
        cost_usd: None,
        note: Some(note.into()),
        account: None,
        account_label: None,
    }
}

#[must_use]
pub fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn usage_percent(value: &Value) -> Option<f64> {
    if value.is_boolean() || value.is_null() || value.as_str() == Some("") {
        return None;
    }
    let percent = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))?;
    (percent.is_finite() && (0.0..=100.0).contains(&percent)).then_some(percent)
}

/// `Ok(None)` is a verdict on this file version (oversized or not a JSON
/// object) and may be cached; `Err` is a failed read and must not be.
fn read_summary(path: &Path, metadata: &fs::Metadata) -> Result<Option<Map<String, Value>>, ()> {
    if metadata.len() > MAX_SUMMARY_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|_| ())?;
    Ok(match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    })
}

fn usable_title(summary: &Map<String, Value>) -> Option<String> {
    ["generated_title", "session_summary", "last_turn_summary"]
        .into_iter()
        .find_map(|field| nonempty(summary.get(field)))
        .map(|title| title.chars().take(MAX_TITLE_CHARS).collect())
}

fn is_history_message(value: &Value) -> bool {
    let Some(record) = value.as_object() else {
        return false;
    };
    match nonempty(record.get("type")) {
        Some("assistant") => true,
        Some("user") => {
            nonempty(record.get("synthetic_reason")).is_none() && user_text(record).is_some()
        }
        _ => false,
    }
}

/// The user's own words, borrowed from the record where possible: the
/// window filter only asks whether there are any, and the history keeps one
/// owned copy.
fn user_text(record: &Map<String, Value>) -> Option<Cow<'_, str>> {
    let raw = flatten_content(record.get("content"))?;
    if let Some(range) = user_query_range(&raw) {
        return Some(match raw {
            Cow::Borrowed(text) => Cow::Borrowed(&text[range]),
            Cow::Owned(text) => Cow::Owned(text[range].to_owned()),
        });
    }
    if raw.contains("<user_info>") || raw.contains("<git_status>") {
        None
    } else {
        Some(raw)
    }
}

/// The trimmed, non-empty body of the first `<user_query>` element.
fn user_query_range(raw: &str) -> Option<std::ops::Range<usize>> {
    let begin = raw.find("<user_query>")? + "<user_query>".len();
    let end = begin + raw[begin..].find("</user_query>")?;
    let body = &raw[begin..end];
    let start = begin + (body.len() - body.trim_start().len());
    let finish = begin + body.trim_end().len();
    (start < finish).then_some(start..finish)
}

/// Borrows the common single-string content instead of copying it; only the
/// multi-block form has to build a new String.
fn flatten_content(value: Option<&Value>) -> Option<Cow<'_, str>> {
    if let Some(text) = value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        return Some(Cow::Borrowed(text));
    }
    let joined = value
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|item| nonempty(item.get("text")))
        .collect::<Vec<_>>()
        .join("\n\n");
    let joined = joined.trim();
    (!joined.is_empty()).then(|| Cow::Owned(joined.to_owned()))
}

fn parse_arguments(value: &Value) -> Value {
    value.as_str().map_or_else(
        || value.clone(),
        |value| serde_json::from_str(value).unwrap_or_else(|_| json!({"raw":value})),
    )
}

fn find_session_dir(root: &Path, session_id: &str, cwd: &str) -> Option<PathBuf> {
    if !usable_session_id(session_id) {
        return None;
    }
    if !cwd.is_empty() {
        let scoped = root.join(percent_encode(cwd)).join(session_id);
        if safe_session_directory(root, &scoped, session_id) {
            return Some(scoped);
        }
    }
    for project in safe_directories(root).ok()? {
        let candidate = project.join(session_id);
        if safe_session_directory(root, &candidate, session_id) {
            return Some(candidate);
        }
    }
    None
}

fn safe_session_directory(root: &Path, directory: &Path, session_id: &str) -> bool {
    if !usable_session_id(session_id)
        || directory.file_name().and_then(|name| name.to_str()) != Some(session_id)
    {
        return false;
    }
    let Ok(relative) = directory.strip_prefix(root) else {
        return false;
    };
    if relative.components().count() != 2
        || !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }
    let Some(project) = directory.parent() else {
        return false;
    };
    for candidate in [root, project, directory] {
        let Ok(metadata) = fs::symlink_metadata(candidate) else {
            return false;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return false;
        }
    }
    let (Ok(canonical_root), Ok(canonical_directory)) =
        (fs::canonicalize(root), fs::canonicalize(directory))
    else {
        return false;
    };
    canonical_directory != canonical_root
        && canonical_directory.starts_with(&canonical_root)
        && canonical_directory.parent().and_then(Path::parent) == Some(canonical_root.as_path())
}

fn safe_directories(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() && !file_type.is_symlink() {
            output.push(entry.path());
        }
    }
    Ok(output)
}

fn read_history_records(path: &Path, limit: usize) -> std::io::Result<(Vec<Value>, usize)> {
    let metadata = safe_file_metadata(path)?;
    if metadata.len() > MAX_HISTORY_BYTES {
        return Err(std::io::Error::other("Grok history is too large"));
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    let mut total_messages = 0usize;
    if limit == 0 {
        let mut records = Vec::new();
        while let Some(value) = read_jsonl_value(&mut reader, &mut line)? {
            if is_history_message(&value) {
                total_messages += 1;
            }
            records.push(value);
        }
        return Ok((records, total_messages));
    }
    let mut window = VecDeque::new();
    let mut window_messages = 0usize;
    while let Some(value) = read_jsonl_value(&mut reader, &mut line)? {
        let is_message = is_history_message(&value);
        if is_message {
            total_messages += 1;
            window_messages += 1;
        }
        window.push_back((value, is_message));
        while window_messages > limit {
            match window.pop_front() {
                Some((_, true)) => {
                    window_messages -= 1;
                    while window.front().is_some_and(|(_, is_message)| !*is_message) {
                        window.pop_front();
                    }
                }
                Some((_, false)) => {}
                None => break,
            }
        }
    }
    Ok((
        window.into_iter().map(|(value, _)| value).collect(),
        total_messages,
    ))
}

fn read_jsonl_value(
    reader: &mut BufReader<File>,
    line: &mut String,
) -> std::io::Result<Option<Value>> {
    loop {
        line.clear();
        // Stop one byte past the bound instead of materializing an arbitrarily
        // long record before rejecting it.
        let bytes = (&mut *reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_line(line)?;
        if bytes == 0 {
            return Ok(None);
        }
        if bytes > MAX_LINE_BYTES {
            return Err(std::io::Error::other(
                "Grok history contains an oversized record",
            ));
        }
        if let Ok(value) = serde_json::from_str(line.trim()) {
            return Ok(Some(value));
        }
    }
}

fn safe_file_metadata(path: &Path) -> std::io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        Ok(metadata)
    } else {
        Err(std::io::Error::other("path was not a safe regular file"))
    }
}

fn usable_session_id(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control)
}

fn nonempty(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn percent_encode(value: &str) -> String {
    if value == "." {
        return "%2E".to_owned();
    }
    if value == ".." {
        return "%2E%2E".to_owned();
    }
    let mut output = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                *byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            output.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(output, "%{byte:02X}");
        }
    }
    output
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).unwrap_or_else(|_| value.into())
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

/// Grok's stored form keeps the real milliseconds.
fn grok_iso(milliseconds: i64) -> String {
    unix_millis_iso(milliseconds, IsoFraction::Millis)
}

fn normalize_iso(value: &str) -> Option<String> {
    parse_iso_ms(value).map(grok_iso)
}

fn now_iso() -> String {
    grok_iso(current_time_ms())
}

fn parse_iso_ms(value: &str) -> Option<i64> {
    if value.len() < 20 || value.as_bytes().get(10) != Some(&b'T') {
        return None;
    }
    let year = value.get(0..4)?.parse::<i64>().ok()?;
    let month = value.get(5..7)?.parse::<i64>().ok()?;
    let day = value.get(8..10)?.parse::<i64>().ok()?;
    let hour = value.get(11..13)?.parse::<i64>().ok()?;
    let minute = value.get(14..16)?.parse::<i64>().ok()?;
    let second = value.get(17..19)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let mut cursor = 19;
    let mut millis = 0_i64;
    if value.as_bytes().get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while value.as_bytes().get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        let fraction = &value[start..cursor];
        let mut padded = fraction.chars().take(3).collect::<String>();
        while padded.len() < 3 {
            padded.push('0');
        }
        millis = padded.parse().ok()?;
    }
    let offset_seconds = match value.get(cursor..)? {
        "Z" => 0,
        offset if offset.len() == 6 && matches!(offset.as_bytes()[0], b'+' | b'-') => {
            let hours = offset.get(1..3)?.parse::<i64>().ok()?;
            let minutes = offset.get(4..6)?.parse::<i64>().ok()?;
            let magnitude = hours * 3_600 + minutes * 60;
            if offset.starts_with('+') {
                magnitude
            } else {
                -magnitude
            }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    Some(((days * 86_400 + hour * 3_600 + minute * 60 + second - offset_seconds) * 1_000) + millis)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn sanitized_store_fixture_lists_replays_and_deletes() {
        let root = tempdir().unwrap();
        let project = root.path().join(percent_encode("/work/a"));
        let session = project.join("grok-1");
        fs::create_dir_all(&session).unwrap();
        fs::write(
            session.join("summary.json"),
            r#"{
          "info":{"id":"grok-1","cwd":"/work/a"},
          "generated_title":"Fixture", "created_at":"2026-08-01T00:00:00Z",
          "last_active_at":"2026-08-02T00:00:00Z", "current_model_id":"grok-4.6"
        }"#,
        )
        .unwrap();
        fs::write(
            session.join("chat_history.jsonl"),
            concat!(
                "{\"type\":\"user\",\"content\":\"<user_query>hello</user_query>\"}\n",
                "{\"type\":\"assistant\",\"content\":\"world\"}\n"
            ),
        )
        .unwrap();
        let engine = EngineId::new("acp:grok").unwrap();
        let listed =
            list_sessions_cached(root.path(), &engine, &mut GrokListingCache::default()).unwrap();
        assert_eq!(listed[0].title, "Fixture");
        let page = history(root.path(), "grok-1", "/work/a", 500);
        assert_eq!(page.messages, Some(2));
        delete(root.path(), "grok-1", "/work/a").unwrap();
        assert!(!session.exists());
    }

    #[test]
    fn listing_reuses_parsed_summaries_until_the_file_changes() {
        let root = tempdir().unwrap();
        let session = root.path().join(percent_encode("/work/a")).join("grok-1");
        fs::create_dir_all(&session).unwrap();
        let summary = session.join("summary.json");
        fs::write(
            &summary,
            r#"{"info":{"id":"grok-1","cwd":"/work/a"},"generated_title":"First"}"#,
        )
        .unwrap();
        let engine = EngineId::new("acp:grok").unwrap();
        let mut cache = GrokListingCache::default();
        let first = list_sessions_cached(root.path(), &engine, &mut cache).unwrap();
        assert_eq!(first[0].title, "First");
        // Same file version: the cached fields are used, not the file.
        cache
            .entries
            .get_mut(&summary)
            .unwrap()
            .1
            .as_mut()
            .unwrap()
            .title = Some("From cache".into());
        let cached = list_sessions_cached(root.path(), &engine, &mut cache).unwrap();
        assert_eq!(cached[0].title, "From cache");
        // A rewritten summary is a new version and is read again.
        fs::write(
            &summary,
            r#"{"info":{"id":"grok-1","cwd":"/work/a"},"generated_title":"Second title"}"#,
        )
        .unwrap();
        let reread = list_sessions_cached(root.path(), &engine, &mut cache).unwrap();
        assert_eq!(reread[0].title, "Second title");
        // A deleted session leaves the cache on the next listing.
        fs::remove_dir_all(&session).unwrap();
        assert!(
            list_sessions_cached(root.path(), &engine, &mut cache)
                .unwrap()
                .is_empty()
        );
        assert!(cache.entries.is_empty());
    }

    fn write_history_fixture(root: &Path, body: &str) {
        let session = root.join(percent_encode("/work/a")).join("grok-1");
        fs::create_dir_all(&session).unwrap();
        fs::write(
            session.join("summary.json"),
            r#"{"info":{"id":"grok-1","cwd":"/work/a"},"generated_title":"Fixture"}"#,
        )
        .unwrap();
        fs::write(session.join("chat_history.jsonl"), body).unwrap();
    }

    #[test]
    fn history_tail_keeps_leading_non_messages_when_the_page_covers_all() {
        let root = tempdir().unwrap();
        write_history_fixture(
            root.path(),
            concat!(
                "{\"type\":\"tool_result\",\"tool_call_id\":\"t-1\",\"content\":\"lead\"}\n",
                "{\"type\":\"user\",\"content\":\"<user_query>hello</user_query>\"}\n",
                "{\"type\":\"assistant\",\"content\":\"world\"}\n"
            ),
        );
        let page = history(root.path(), "grok-1", "/work/a", 500);
        assert_eq!(page.messages, Some(2));
        assert_eq!(page.older_messages, 0);
        assert!(matches!(
            &page.events[0].event,
            EngineEventKind::ToolResult { call_id, .. } if call_id == "t-1"
        ));
        assert_eq!(
            history(root.path(), "grok-1", "/work/a", 0).messages,
            Some(2)
        );
    }

    #[test]
    fn unreadable_transcript_is_reported_unavailable_not_as_an_empty_session() {
        let root = tempdir().unwrap();
        write_history_fixture(root.path(), "{\"type\":\"user\",\"content\":\"one\"}\n");
        fs::remove_file(
            root.path()
                .join(percent_encode("/work/a"))
                .join("grok-1")
                .join("chat_history.jsonl"),
        )
        .unwrap();
        let page = history(root.path(), "grok-1", "/work/a", 500);
        assert_eq!(
            page.availability,
            Some(interface_contracts::ipc::HistoryAvailability::Unavailable)
        );
        assert_eq!(
            page.reason.as_deref(),
            Some("Grok could not read the stored transcript.")
        );
        assert!(page.events.is_empty());
    }

    #[test]
    fn history_tail_starts_on_the_kept_message_and_drops_prior_non_messages() {
        let root = tempdir().unwrap();
        write_history_fixture(
            root.path(),
            concat!(
                "{\"type\":\"tool_result\",\"tool_call_id\":\"t-1\",\"content\":\"lead\"}\n",
                "{\"type\":\"user\",\"content\":\"<user_query>one</user_query>\"}\n",
                "{\"type\":\"meta\"}\n",
                "{\"type\":\"user\",\"content\":\"<user_query>two</user_query>\"}\n",
                "{\"type\":\"assistant\",\"content\":\"done\"}\n"
            ),
        );
        let page = history(root.path(), "grok-1", "/work/a", 2);
        assert_eq!(page.messages, Some(2));
        assert_eq!(page.older_messages, 1);
        assert!(matches!(
            &page.events[0].event,
            EngineEventKind::UserMessage { text, .. } if text == "two"
        ));
        assert!(
            page.events
                .iter()
                .all(|event| { !matches!(event.event, EngineEventKind::ToolResult { .. }) })
        );
    }

    #[test]
    fn billing_requires_the_current_period_and_preserves_shared_semantics() {
        let payload = json!({"config": {
            "creditUsagePercent":"37.5",
            "currentPeriod":{"start":"2026-08-01T00:00:00Z","end":"2026-09-01T00:00:00Z","type":"MONTHLY"}
        }, "subscriptionTier":"SuperGrok"});
        let now = parse_iso_ms("2026-08-15T00:00:00Z").unwrap();
        let usage = billing_usage(&payload, now);
        assert!(usage.available);
        assert_eq!(usage.windows[0].label, "Shared monthly pool");
        assert_eq!(usage.windows[0].utilization, Some(37.5));
    }

    /// The live shape at a weekly rollover: every field but the percent.
    #[test]
    fn billing_reads_an_omitted_percent_as_a_fresh_period_at_zero() {
        let payload = json!({"config": {
            "currentPeriod":{"start":"2026-08-28T19:40:57.899750+00:00",
                "end":"2026-09-04T19:40:57.899750+00:00","type":"USAGE_PERIOD_TYPE_WEEKLY"},
            "onDemandUsed":{"val":0},
            "isUnifiedBillingUser": true
        }, "subscription_tier":"SuperGrok Plus"});
        let now = parse_iso_ms("2026-08-29T02:21:02Z").unwrap();
        let usage = billing_usage(&payload, now);
        assert!(usage.available);
        assert_eq!(usage.plan.as_deref(), Some("SuperGrok Plus"));
        assert_eq!(usage.windows[0].label, "Shared weekly pool");
        assert_eq!(usage.windows[0].utilization, Some(0.0));

        // A percent that IS reported but unreadable is still not a zero.
        let mut garbled = payload.clone();
        garbled["config"]["creditUsagePercent"] = json!("not a number");
        assert!(!billing_usage(&garbled, now).available);
    }

    #[test]
    fn grok_store_encodes_dot_cwds_and_rejects_traversal_and_symlinks() {
        assert_eq!(percent_encode("."), "%2E");
        assert_eq!(percent_encode(".."), "%2E%2E");

        let outer = tempdir().unwrap();
        let root = outer.path().join("sessions");
        let victim = outer.path().join("grok-1");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::write(
            victim.join("chat_history.jsonl"),
            "{\"type\":\"user\",\"content\":\"secret\"}\n",
        )
        .unwrap();

        // `cwd = ".."` must not resolve to `outer/grok-1` outside `sessions`.
        let page = history(&root, "grok-1", "..", 10);
        assert!(page.events.is_empty());
        assert!(delete(&root, "grok-1", "..").is_err());
        assert!(victim.exists(), "outside directory must remain untouched");

        // A symlinked project directory inside `sessions` must also be refused.
        let linked_project = root.join(percent_encode("/work/evil"));
        std::os::unix::fs::symlink(outer.path(), &linked_project).unwrap();
        let page = history(&root, "grok-1", "/work/evil", 10);
        assert!(page.events.is_empty());
        assert!(delete(&root, "grok-1", "/work/evil").is_err());
        assert!(victim.exists(), "symlink target must remain untouched");
    }
}
