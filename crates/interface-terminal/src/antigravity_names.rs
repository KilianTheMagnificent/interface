//! Bounded, read-only Antigravity task names. A title never establishes session authority.
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use interface_contracts::terminal::TerminalLaunchProfileId;
use serde_json::Value;

use crate::provider_files::safe_file;

const CACHE_ENTRIES: usize = 128;
// Between one and two title-poll periods: skip one retry without a boundary
// race postponing newly created sessions to the third poll.
const MISS_TTL: Duration = Duration::from_secs(3);

const READ_LIMIT: u64 = 4 * 1024 * 1024;
const ANNOTATION_LIMIT: u64 = 256 * 1024;

#[derive(Default)]
pub(crate) struct AntigravityNames {
    root: Option<PathBuf>,
    missing_prompts: BTreeMap<String, Instant>,
    prompts: BTreeMap<String, String>,
    annotations: BTreeMap<String, (u64, SystemTime, Option<String>)>,
}

impl AntigravityNames {
    /// The pane label for one already-bound Antigravity (`agy-<uuid>`) session,
    /// if Antigravity has written its annotation title or first user request.
    pub(crate) fn lookup(&mut self, root: &Path, binding: &str) -> Option<String> {
        self.lookup_at(root, binding, Instant::now())
    }

    fn lookup_at(&mut self, root: &Path, binding: &str, now: Instant) -> Option<String> {
        if self.root.as_deref() != Some(root) {
            *self = Self {
                root: Some(root.to_owned()),
                ..Self::default()
            };
        }
        let id = crate::native_checkpoint::session_id(TerminalLaunchProfileId::Gemini, binding)?;
        if let Some(title) = self.generated_title(root, id) {
            return Some(title);
        }
        if !self.prompts.contains_key(id)
            && self
                .missing_prompts
                .get(id)
                .is_none_or(|at| now.saturating_duration_since(*at) >= MISS_TTL)
        {
            if let Some(prompt) = first_prompt(root, id) {
                self.missing_prompts.remove(id);
                cache(&mut self.prompts, id.to_owned(), prompt);
            } else {
                cache(&mut self.missing_prompts, id.to_owned(), now);
            }
        }
        self.prompts
            .get(id)
            .map(|text| crate::task_title::codex_request_title(text))
            .filter(|title| !title.is_empty())
    }

    fn generated_title(&mut self, root: &Path, id: &str) -> Option<String> {
        let path = root.join("annotations").join(format!("{id}.pbtxt"));
        let file = safe_file(root, &path)?;
        let Ok(meta) = file.metadata() else {
            return None;
        };
        if meta.len() > ANNOTATION_LIMIT {
            return None;
        }
        let stamp = (
            meta.len(),
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        );
        if let Some((len, modified, cached)) = self.annotations.get(id)
            && *len == stamp.0
            && *modified == stamp.1
        {
            return cached.clone();
        }
        let mut bytes = Vec::new();
        if file.take(ANNOTATION_LIMIT).read_to_end(&mut bytes).is_err() {
            return None;
        }
        let title = std::str::from_utf8(&bytes)
            .ok()
            .and_then(annotation_label)
            .filter(|title| !title.is_empty());
        cache(
            &mut self.annotations,
            id.to_owned(),
            (stamp.0, stamp.1, title.clone()),
        );
        title
    }
}

fn cache<K: Ord, V>(entries: &mut BTreeMap<K, V>, key: K, value: V) {
    if entries.len() >= CACHE_ENTRIES && !entries.contains_key(&key) {
        entries.pop_first();
    }
    entries.insert(key, value);
}

fn annotation_label(text: &str) -> Option<String> {
    let title = text.lines().find_map(parse_pbtxt_title)?;
    let title = title.trim();
    if title.is_empty()
        || title.len() > 4096
        || matches!(
            title.to_ascii_lowercase().as_str(),
            "antigravity" | "agy" | "gemini"
        )
    {
        return None;
    }
    let label = crate::task_title::bounded_codex_name(title);
    (!label.is_empty()).then_some(label)
}

fn parse_pbtxt_title(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("title:")?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return String::from_utf8(out).ok(),
            b'\\' => {
                index += 1;
                let escaped = *bytes.get(index)?;
                match escaped {
                    b'n' | b'r' | b't' => {
                        out.push(b' ');
                        index += 1;
                    }
                    b'0'..=b'7' => {
                        let end = (index + 3).min(bytes.len());
                        let digits = bytes[index..end]
                            .iter()
                            .take_while(|byte| (b'0'..=b'7').contains(byte))
                            .count();
                        let octal = std::str::from_utf8(&bytes[index..index + digits]).ok()?;
                        out.push(u8::from_str_radix(octal, 8).ok()?);
                        index += digits;
                    }
                    b'x' => {
                        index += 1;
                        let end = (index + 2).min(bytes.len());
                        let digits = bytes[index..end]
                            .iter()
                            .take_while(|byte| byte.is_ascii_hexdigit())
                            .count();
                        if digits == 0 {
                            return None;
                        }
                        let hex = std::str::from_utf8(&bytes[index..index + digits]).ok()?;
                        out.push(u8::from_str_radix(hex, 16).ok()?);
                        index += digits;
                    }
                    other => {
                        out.push(other);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    None
}

fn first_prompt(root: &Path, id: &str) -> Option<String> {
    let path = root
        .join("brain")
        .join(id)
        .join(".system_generated/logs/transcript.jsonl");
    let file = safe_file(root, &path)?;
    for line in BufReader::new(file.take(READ_LIMIT))
        .lines()
        .map_while(Result::ok)
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["type"] != "USER_INPUT" || value["source"] != "USER_EXPLICIT" {
            continue;
        }
        if let Some(prompt) = user_request(&value) {
            return Some(prompt);
        }
    }
    None
}

fn user_request(value: &Value) -> Option<String> {
    let raw = value["content"].as_str()?;
    let body = if let Some(start) = raw.find("<USER_REQUEST>") {
        let after = &raw[start + "<USER_REQUEST>".len()..];
        after
            .split_once("</USER_REQUEST>")
            .map_or(after, |(request, _)| request)
    } else if raw.contains("<ADDITIONAL_METADATA>") || raw.contains("<CONTEXT_SUMMARY>") {
        return None;
    } else {
        raw
    };
    let trimmed = body.trim();
    if trimmed.is_empty()
        || trimmed.contains("INTERFACE_UPDATE_READY:")
        || (trimmed.starts_with('/') && !trimmed.contains(char::is_whitespace))
    {
        return None;
    }
    Some(trimmed.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const ID: &str = "8b764f80-19ee-47c7-a9da-08088a6db888";
    const BINDING: &str = "agy-8b764f80-19ee-47c7-a9da-08088a6db888";

    fn write_conversation(root: &Path, id: &str, annotation: Option<&str>, transcript: &str) {
        let annotations = root.join("annotations");
        let logs = root.join("brain").join(id).join(".system_generated/logs");
        fs::create_dir_all(&annotations).unwrap();
        fs::create_dir_all(&logs).unwrap();
        if let Some(annotation) = annotation {
            fs::write(annotations.join(format!("{id}.pbtxt")), annotation).unwrap();
        }
        fs::write(logs.join("transcript.jsonl"), transcript).unwrap();
    }

    #[test]
    fn generated_annotation_replaces_the_first_request_and_ignores_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = concat!(
            "{\"step_index\":0,\"source\":\"SYSTEM\",\"type\":\"USER_INPUT\",\"content\":\"ignored\"}\n",
            "{\"step_index\":1,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"content\":\"<USER_REQUEST>\\nI want you to give Antigravity panes real titles\\n</USER_REQUEST>\\n<ADDITIONAL_METADATA>time</ADDITIONAL_METADATA>\"}\n",
        );
        write_conversation(temp.path(), ID, None, transcript);
        let mut names = AntigravityNames::default();
        assert_eq!(
            names.lookup(temp.path(), BINDING).as_deref(),
            Some("Give Antigravity panes real titles")
        );
        fs::write(
            temp.path().join("annotations").join(format!("{ID}.pbtxt")),
            "title:\"Customizing Anti-Gravity Terminal Titles\"\n",
        )
        .unwrap();
        assert_eq!(
            names.lookup(temp.path(), BINDING).as_deref(),
            Some("Customizing Anti-Gravity Terminal Titles")
        );
        assert!(names.lookup(temp.path(), ID).is_none());
        assert!(names.lookup(temp.path(), "agy-..").is_none());
        assert!(
            names
                .lookup(temp.path(), "muse-01a15b88-9662-7ce0-b98e-0c2b512a773a")
                .is_none()
        );
    }

    #[test]
    fn brand_names_and_symlinks_are_rejected_while_pbtxt_escapes_decode() {
        let temp = tempfile::tempdir().unwrap();
        write_conversation(
            temp.path(),
            ID,
            Some("title:\"Antigravity\"\n"),
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"content\":\"<USER_REQUEST>Keep the first request</USER_REQUEST>\"}\n",
        );
        let mut names = AntigravityNames::default();
        assert_eq!(
            names.lookup(temp.path(), BINDING).as_deref(),
            Some("Keep the first request")
        );
        let annotation = temp.path().join("annotations").join(format!("{ID}.pbtxt"));
        fs::write(
            &annotation,
            "title: \"Fix \\\"agy\\\" \\342\\200\\224 pane titles\"\n",
        )
        .unwrap();
        assert_eq!(
            names.lookup(temp.path(), BINDING).as_deref(),
            Some("Fix \"agy\" — pane titles")
        );
        let outside = temp.path().join("outside.pbtxt");
        fs::write(&outside, "title:\"Escaped\"\n").unwrap();
        fs::remove_file(&annotation).unwrap();
        std::os::unix::fs::symlink(&outside, &annotation).unwrap();
        names.annotations.remove(ID);
        assert_eq!(
            names.lookup(temp.path(), BINDING).as_deref(),
            Some("Keep the first request")
        );
    }

    #[test]
    fn missing_prompts_retry_and_every_cache_has_a_count_bound() {
        let root = tempfile::tempdir().unwrap();
        write_conversation(root.path(), ID, None, "");
        let now = Instant::now();
        let mut names = AntigravityNames::default();
        assert!(names.lookup_at(root.path(), BINDING, now).is_none());
        write_conversation(
            root.path(),
            ID,
            None,
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"content\":\"<USER_REQUEST>Fix panes</USER_REQUEST>\"}\n",
        );
        assert!(
            names
                .lookup_at(root.path(), BINDING, now + Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(
            names
                .lookup_at(root.path(), BINDING, now + MISS_TTL)
                .as_deref(),
            Some("Fix panes")
        );
        for index in 0..CACHE_ENTRIES * 2 {
            let id = format!("8b764f80-19ee-47c7-a9da-{index:012x}");
            let binding = format!("agy-{id}");
            write_conversation(root.path(), &id, Some("title:\"\"\n"), "");
            assert!(
                names
                    .lookup_at(root.path(), &binding, now + MISS_TTL)
                    .is_none()
            );
        }
        assert!(names.missing_prompts.len() <= CACHE_ENTRIES);
        assert!(names.prompts.len() <= CACHE_ENTRIES);
        assert!(names.annotations.len() <= CACHE_ENTRIES);
    }
}
