//! Bounded, read-only Grok task names. A title never establishes session authority.
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use crate::provider_files::safe_file;

use serde_json::Value;

const CACHE_ENTRIES: usize = 128;
// Between one and two title-poll periods: skip one retry without a boundary
// race postponing newly created sessions to the third poll.
const MISS_TTL: Duration = Duration::from_secs(3);

const READ_LIMIT: u64 = 4 * 1024 * 1024;
const SUMMARY_LIMIT: u64 = 256 * 1024;

#[derive(Default)]
pub(crate) struct GrokNames {
    root: Option<PathBuf>,
    missing_dirs: BTreeMap<(PathBuf, String), Instant>,
    missing_prompts: BTreeMap<String, Instant>,
    dirs: BTreeMap<String, PathBuf>,
    prompts: BTreeMap<String, String>,
    summaries: BTreeMap<String, (u64, SystemTime, Option<String>)>,
}

impl GrokNames {
    /// The pane label for one already-bound Grok session, if Grok has a name
    /// or a real first request. `resume` is ignored: Interface's saved flag
    /// stays false, while the minted id is still the chat Grok stored.
    pub(crate) fn lookup(&mut self, sessions: &Path, cwd: &Path, id: &str) -> Option<String> {
        self.lookup_at(sessions, cwd, id, Instant::now())
    }

    fn lookup_at(&mut self, sessions: &Path, cwd: &Path, id: &str, now: Instant) -> Option<String> {
        if self.root.as_deref() != Some(sessions) {
            *self = Self {
                root: Some(sessions.to_owned()),
                ..Self::default()
            };
        }
        if !session_id_ok(id) {
            return None;
        }
        let dir = self.session_dir(sessions, cwd, id, now)?;
        if let Some(title) = self.generated_title(sessions, &dir, id) {
            return Some(title);
        }
        if !self.prompts.contains_key(id)
            && self
                .missing_prompts
                .get(id)
                .is_none_or(|at| now.saturating_duration_since(*at) >= MISS_TTL)
        {
            if let Some(prompt) = first_prompt(sessions, &dir) {
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

    fn session_dir(&mut self, root: &Path, cwd: &Path, id: &str, now: Instant) -> Option<PathBuf> {
        if let Some(dir) = self.dirs.get(id).cloned() {
            if contained_dir(root, &dir) {
                return Some(dir);
            }
            self.dirs.remove(id);
            self.prompts.remove(id);
            self.summaries.remove(id);
            self.missing_prompts.remove(id);
        }
        let key = (cwd.to_owned(), id.to_owned());
        if self
            .missing_dirs
            .get(&key)
            .is_some_and(|at| now.saturating_duration_since(*at) < MISS_TTL)
        {
            return None;
        }
        let direct = cwd
            .to_str()
            .map(|cwd| root.join(percent_encode(cwd)).join(id))
            .filter(|dir| contained_dir(root, dir));
        let found = direct.or_else(|| {
            fs::read_dir(root)
                .ok()?
                .take(4096)
                .flatten()
                .map(|entry| entry.path().join(id))
                .find(|dir| contained_dir(root, dir))
        });
        if let Some(dir) = found {
            self.missing_dirs.remove(&key);
            cache(&mut self.dirs, id.to_owned(), dir.clone());
            Some(dir)
        } else {
            cache(&mut self.missing_dirs, key, now);
            None
        }
    }

    fn generated_title(&mut self, root: &Path, dir: &Path, id: &str) -> Option<String> {
        let path = dir.join("summary.json");
        let file = safe_file(root, &path)?;
        let Ok(meta) = file.metadata() else {
            return None;
        };
        if meta.len() > SUMMARY_LIMIT {
            return None;
        }
        let stamp = (
            meta.len(),
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        );
        if let Some((len, modified, cached)) = self.summaries.get(id)
            && *len == stamp.0
            && *modified == stamp.1
        {
            return cached.clone();
        }
        let mut bytes = Vec::new();
        if file.take(SUMMARY_LIMIT).read_to_end(&mut bytes).is_err() {
            return None;
        }
        let title = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .as_ref()
            .and_then(generated_label)
            .filter(|title| !title.is_empty());
        cache(
            &mut self.summaries,
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

fn session_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn generated_label(summary: &Value) -> Option<String> {
    let title = summary["generated_title"].as_str()?.trim();
    if title.is_empty() || title.len() > 4096 || title.eq_ignore_ascii_case("grok") {
        return None;
    }
    let label = crate::task_title::bounded_codex_name(title);
    (!label.is_empty()).then_some(label)
}

fn contained_dir(root: &Path, dir: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return false;
    }
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let Ok(canonical) = dir.canonicalize() else {
        return false;
    };
    canonical.starts_with(&root)
}

/// Grok percent-encodes the session cwd for `sessions/<cwd>/<id>/`.
fn percent_encode(value: &str) -> String {
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

fn first_prompt(root: &Path, dir: &Path) -> Option<String> {
    let file = safe_file(root, &dir.join("chat_history.jsonl"))?;
    for line in BufReader::new(file.take(READ_LIMIT))
        .lines()
        .map_while(Result::ok)
    {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value["type"] != "user" {
            continue;
        }
        if value["synthetic_reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
        {
            continue;
        }
        if let Some(prompt) = user_query(&value) {
            return Some(prompt);
        }
    }
    None
}

fn user_query(value: &Value) -> Option<String> {
    let raw = flatten_content(&value["content"])?;
    if let Some(start) = raw.find("<user_query>") {
        let body = &raw[start + "<user_query>".len()..];
        let query = body
            .split_once("</user_query>")
            .map_or(body, |(query, _)| query)
            .trim();
        if !query.is_empty() {
            return Some(query.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    if raw.contains("<user_info>") || raw.contains("<git_status>") {
        return None;
    }
    let trimmed = raw.trim();
    (!trimmed.is_empty()).then(|| trimmed.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn flatten_content(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let parts = value.as_array()?;
    let mut text = String::new();
    for part in parts {
        if part["type"] == "text" || part.get("type").is_none() {
            if let Some(piece) = part["text"].as_str().or_else(|| part.as_str()) {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(piece);
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "8b764f80-19ee-47c7-a9da-08088a6db888";

    fn write_session(root: &Path, cwd: &str, summary: &str, history: &str) {
        let dir = root.join(percent_encode(cwd)).join(ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("summary.json"), summary).unwrap();
        fs::write(dir.join("chat_history.jsonl"), history).unwrap();
    }

    #[test]
    fn generated_title_replaces_the_first_request_and_ignores_context() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = "/work/app";
        let history = concat!(
            "{\"type\":\"system\",\"content\":\"policy\"}\n",
            "{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"<user_info>os</user_info><user_query>I want you to fix pane names</user_query>\"}]}\n",
            "{\"type\":\"user\",\"synthetic_reason\":\"resume\",\"content\":\"ignored\"}\n",
        );
        write_session(
            temp.path(),
            cwd,
            r#"{"info":{"id":"8b764f80-19ee-47c7-a9da-08088a6db888","cwd":"/work/app"}}"#,
            history,
        );
        let mut names = GrokNames::default();
        assert_eq!(
            names.lookup(temp.path(), Path::new(cwd), ID).as_deref(),
            Some("Fix pane names")
        );
        fs::write(
            temp.path()
                .join(percent_encode(cwd))
                .join(ID)
                .join("summary.json"),
            r#"{"generated_title":"Name Grok panes","info":{"id":"8b764f80-19ee-47c7-a9da-08088a6db888"}}"#,
        )
        .unwrap();
        assert_eq!(
            names.lookup(temp.path(), Path::new(cwd), ID).as_deref(),
            Some("Name Grok panes")
        );
        assert!(names.lookup(temp.path(), Path::new(cwd), "..").is_none());
        assert!(names.lookup(temp.path(), Path::new(cwd), "other").is_none());
    }

    #[test]
    fn a_renamed_title_is_read_and_a_symlink_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = "/work/app";
        write_session(
            temp.path(),
            cwd,
            r#"{"generated_title":"Grok","title_is_manual":false}"#,
            "{\"type\":\"user\",\"content\":\"<user_query>Keep the numbered label</user_query>\"}\n",
        );
        let mut names = GrokNames::default();
        assert_eq!(
            names.lookup(temp.path(), Path::new(cwd), ID).as_deref(),
            Some("Keep the numbered label"),
            "the app name is not a task"
        );
        let summary = temp
            .path()
            .join(percent_encode(cwd))
            .join(ID)
            .join("summary.json");
        fs::write(
            &summary,
            r#"{"generated_title":"Manual pane name","title_is_manual":true}"#,
        )
        .unwrap();
        assert_eq!(
            names.lookup(temp.path(), Path::new(cwd), ID).as_deref(),
            Some("Manual pane name")
        );
        let outside = temp.path().join("outside.json");
        fs::write(&outside, r#"{"generated_title":"Escaped"}"#).unwrap();
        fs::remove_file(&summary).unwrap();
        std::os::unix::fs::symlink(&outside, &summary).unwrap();
        names.summaries.remove(ID);
        assert_eq!(
            names.lookup(temp.path(), Path::new(cwd), ID).as_deref(),
            Some("Keep the numbered label")
        );
    }

    #[test]
    fn a_mismatched_cwd_folder_still_finds_the_session() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("not-the-encoding").join(ID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("summary.json"),
            r#"{"generated_title":"Found by id"}"#,
        )
        .unwrap();
        let mut names = GrokNames::default();
        assert_eq!(
            names
                .lookup(temp.path(), Path::new("/somewhere/else"), ID)
                .as_deref(),
            Some("Found by id")
        );
    }
    #[test]
    fn misses_expire_and_a_changed_root_never_reuses_a_cached_name() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let cwd = Path::new("/project");
        let now = Instant::now();
        let mut names = GrokNames::default();
        assert!(names.lookup_at(first.path(), cwd, ID, now).is_none());
        write_session(
            first.path(),
            "/project",
            r#"{"generated_title":"First root"}"#,
            "",
        );
        assert!(
            names
                .lookup_at(first.path(), cwd, ID, now + Duration::from_secs(2))
                .is_none()
        );
        // The next two-second poll can arrive just before an exact four-second
        // expiry because the service stamps its throttle before filesystem work.
        assert_eq!(
            names
                .lookup_at(first.path(), cwd, ID, now + Duration::from_millis(3999))
                .as_deref(),
            Some("First root")
        );
        write_session(
            second.path(),
            "/project",
            r#"{"generated_title":"Second root"}"#,
            "",
        );
        assert_eq!(
            names
                .lookup_at(second.path(), cwd, ID, now + Duration::from_millis(3999))
                .as_deref(),
            Some("Second root")
        );
    }

    #[test]
    fn missing_prompts_retry_and_every_cache_has_a_count_bound() {
        let root = tempfile::tempdir().unwrap();
        let cwd = Path::new("/project");
        write_session(root.path(), "/project", "{}", "");
        let now = Instant::now();
        let mut names = GrokNames::default();
        assert!(names.lookup_at(root.path(), cwd, ID, now).is_none());
        write_session(
            root.path(),
            "/project",
            "{}",
            "{\"type\":\"user\",\"content\":\"Fix panes\"}\n",
        );
        assert!(
            names
                .lookup_at(root.path(), cwd, ID, now + Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(
            names
                .lookup_at(root.path(), cwd, ID, now + MISS_TTL)
                .as_deref(),
            Some("Fix panes")
        );
        for index in 0..CACHE_ENTRIES * 2 {
            let id = format!("session-{index}");
            assert!(names.lookup_at(root.path(), cwd, &id, now).is_none());
            let dir = root.path().join(percent_encode("/project")).join(&id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("summary.json"), "{}").unwrap();
            assert!(
                names
                    .lookup_at(root.path(), cwd, &id, now + MISS_TTL)
                    .is_none()
            );
        }
        assert!(names.missing_dirs.len() <= CACHE_ENTRIES);
        assert!(names.missing_prompts.len() <= CACHE_ENTRIES);
        assert!(names.dirs.len() <= CACHE_ENTRIES);
        assert!(names.prompts.len() <= CACHE_ENTRIES);
        assert!(names.summaries.len() <= CACHE_ENTRIES);
    }
}
