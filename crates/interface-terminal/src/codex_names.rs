//! Bounded, read-only task-name lookup. A title never establishes session authority.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use crate::provider_files::safe_file;

const READ_LIMIT: u64 = 4 * 1024 * 1024;
/// How long a thread whose rollout has no user request yet waits before its
/// day directories are scanned again. Lookups are already throttled per pane.
const MISSING_PROMPT_RETRY: Duration = Duration::from_secs(4);
const MAX_CACHED_THREADS: usize = 128;

#[derive(Default)]
pub(crate) struct CodexNames {
    index_stamp: Option<IndexStamp>,
    index: BTreeMap<String, String>,
    prompts: BTreeMap<String, String>,
    missing_prompts: BTreeMap<String, Instant>,
}

/// What the parsed `index` covers: the exact file, its size and time when
/// read, where the retained window starts, where the last complete line ends
/// and the bytes just before that end, which an append must leave unchanged.
/// An append then costs only the new lines.
#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexStamp {
    device: u64,
    inode: u64,
    len: u64,
    modified: SystemTime,
    window_start: u64,
    parsed_to: u64,
    tail: Vec<u8>,
}
const TAIL_CHECK_BYTES: usize = 256;

impl CodexNames {
    pub(crate) fn is_automatic_title(&self, id: &str, title: &str) -> bool {
        let title = title.trim().trim_end_matches('…');
        self.index.get(id).is_some_and(|name| name == title)
            || self.prompts.get(id).is_some_and(|prompt| {
                prompt.starts_with(title)
                    || crate::task_title::task_title(prompt).trim_end_matches('…') == title
                    || crate::task_title::codex_request_title(prompt).trim_end_matches('…') == title
            })
    }
    pub(crate) fn lookup(&mut self, sessions: &Path, id: &str) -> Option<String> {
        if !crate::launch::codex_thread_id(id) {
            return None;
        }
        self.refresh_index(sessions.parent()?);
        let retry_due = self
            .missing_prompts
            .get(id)
            .is_none_or(|missed| missed.elapsed() >= MISSING_PROMPT_RETRY);
        if !self.prompts.contains_key(id) && retry_due {
            if let Some(prompt) = first_request(sessions, id) {
                self.missing_prompts.remove(id);
                if self.prompts.len() >= MAX_CACHED_THREADS {
                    self.prompts.pop_first();
                }
                self.prompts.insert(id.to_owned(), prompt);
            } else {
                if self.missing_prompts.len() >= MAX_CACHED_THREADS {
                    self.missing_prompts.pop_first();
                }
                self.missing_prompts.insert(id.to_owned(), Instant::now());
            }
        }
        let prompt = self.prompts.get(id);
        let name = self.index.get(id).filter(|name| {
            // Codex also files a clipped first-message preview in this index.
            // Prefer the complete request over that provisional truncation.
            prompt.is_none_or(|prompt| !prompt.starts_with(name.trim_end_matches('…')))
                && !name.starts_with("[ ! ]")
                && !name.starts_with("[ . ]")
                && !name.starts_with("Action Required | ")
                && !name.starts_with("codex | ")
        });
        name.map(|name| crate::task_title::bounded_codex_name(name))
            .or_else(|| prompt.map(|text| crate::task_title::codex_request_title(text)))
    }

    fn refresh_index(&mut self, root: &Path) {
        let path = root.join("session_index.jsonl");
        let Some(mut file) = safe_file(root, &path) else {
            return;
        };
        let Ok(meta) = file.metadata() else { return };
        let (device, inode, len) = (meta.dev(), meta.ino(), meta.len());
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if self.index_stamp.as_ref().is_some_and(|stamp| {
            (stamp.device, stamp.inode, stamp.len, stamp.modified) == (device, inode, len, modified)
        }) {
            return;
        }
        // Codex appends to this index. When the same file only grew and the
        // bytes before the last parsed line end are unchanged, parse just the
        // new lines; anything else (a new file, a shrink, a rewrite, or a
        // window grown past twice the read limit) re-reads the newest
        // READ_LIMIT bytes from scratch.
        let appended = self
            .index_stamp
            .take()
            .filter(|stamp| {
                stamp.device == device
                    && stamp.inode == inode
                    && len > stamp.len
                    && len - stamp.window_start <= 2 * READ_LIMIT
            })
            .and_then(|stamp| {
                let read_from = stamp.parsed_to - stamp.tail.len() as u64;
                let bytes = read_range(&mut file, read_from, len)?;
                bytes
                    .starts_with(&stamp.tail)
                    .then_some((stamp.window_start, read_from, bytes))
            });
        let appending = appended.is_some();
        let Some((window_start, read_from, bytes)) = appended.or_else(|| {
            let start = len.saturating_sub(READ_LIMIT);
            read_range(&mut file, start, len).map(|bytes| (start, start, bytes))
        }) else {
            return;
        };
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |newline| newline + 1);
        let mut names = if appending {
            std::mem::take(&mut self.index)
        } else {
            BTreeMap::new()
        };
        // Re-parsing the verified tail line is harmless: it inserts the same
        // name again before any newer line. A trailing line without its
        // newline is parsed if it is already whole, and read again with its
        // completion next time.
        for line in bytes.split(|byte| *byte == b'\n') {
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if let (Some(id), Some(name)) = (value["id"].as_str(), value["thread_name"].as_str())
                && crate::launch::codex_thread_id(id)
                && !name.trim().is_empty()
                && name.len() <= 4096
            {
                names.insert(id.to_owned(), name.to_owned());
            }
        }
        self.index = names;
        self.index_stamp = Some(IndexStamp {
            device,
            inode,
            len,
            modified,
            window_start,
            parsed_to: read_from + complete as u64,
            tail: bytes[complete.saturating_sub(TAIL_CHECK_BYTES)..complete].to_vec(),
        });
    }
}

/// The bytes of `file` in `start..end`, or `None` when they cannot be read.
fn read_range(file: &mut File, start: u64, end: u64) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(end.saturating_sub(start))
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// The entries that may be Codex thread `id`'s rollout: `*-<id>.jsonl` under
/// `sessions/YYYY/MM/DD/`. Day folders are local time, so the UTC day in the
/// v7 id's timestamp and its two neighbours are read, at most `per_day`
/// entries of each. `id` is a validated canonical UUID.
pub(crate) fn codex_rollout_candidates(
    sessions: &Path,
    id: &str,
    per_day: usize,
) -> impl Iterator<Item = fs::DirEntry> {
    let millis = i64::from_str_radix(&format!("{}{}", &id[..8], &id[9..13]), 16).ok();
    let suffix = format!("-{id}.jsonl");
    let sessions = sessions.to_path_buf();
    millis
        .into_iter()
        .flat_map(|millis| {
            let day = millis.div_euclid(86_400_000);
            day - 1..=day + 1
        })
        .filter_map(move |day| {
            let (year, month, day) = interface_common::time::civil_from_days(day);
            fs::read_dir(sessions.join(format!("{year:04}/{month:02}/{day:02}"))).ok()
        })
        .flat_map(move |entries| entries.take(per_day).flatten())
        .filter(move |entry| entry.file_name().to_string_lossy().ends_with(&suffix))
}

fn first_request(sessions: &Path, id: &str) -> Option<String> {
    codex_rollout_candidates(sessions, id, 4096).find_map(|entry| {
        let file = safe_file(sessions, &entry.path())?;
        request_in_rollout(file, id)
    })
}

fn request_in_rollout(file: File, id: &str) -> Option<String> {
    let mut confirmed = false;
    for line in BufReader::new(file.take(READ_LIMIT))
        .lines()
        .map_while(Result::ok)
    {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value["type"] == "session_meta" {
            if value["payload"]["id"].as_str() != Some(id) {
                return None;
            }
            confirmed = true;
        }
        // Recent Codex CLI rollouts retain user messages as response items
        // without a separate user_message event. Ignore injected context and
        // attachments; they are not the user's task.
        if confirmed
            && value["type"] == "response_item"
            && value["payload"]["type"] == "message"
            && value["payload"]["role"] == "user"
            && let Some(content) = value["payload"]["content"].as_array()
        {
            let request = content
                .iter()
                .filter_map(|part| {
                    (part["type"] == "input_text")
                        .then(|| part["text"].as_str())
                        .flatten()
                })
                .filter(|text| !is_context(text))
                .collect::<Vec<_>>()
                .join(" ");
            if !request.trim().is_empty() {
                return Some(request.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
        if confirmed
            && value["type"] == "event_msg"
            && value["payload"]["type"] == "user_message"
            && let Some(message) = value["payload"]["message"].as_str()
            && !message.trim().is_empty()
            && !is_context(message)
        {
            return Some(message.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }
    None
}

fn is_context(message: &str) -> bool {
    let message = message.trim();
    if message == "</image>" {
        return true;
    }
    if message.starts_with("[Image ") && message.ends_with(']') && !message.contains("] ") {
        return true;
    }
    [
        "# AGENTS.md instructions",
        "<environment_context>",
        "<INSTRUCTIONS>",
        "<permissions instructions>",
        "<image ",
    ]
    .iter()
    .any(|prefix| message.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "01a08e06-c060-78a2-a66d-3a43629736be";

    #[test]
    fn current_cli_response_items_skip_instructions_and_images() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rollout.jsonl");
        let rows = [
            serde_json::json!({"type":"session_meta","payload":{"id":ID}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[
                {"type":"input_text","text":"# AGENTS.md instructions\n<INSTRUCTIONS>policy</INSTRUCTIONS>"},
                {"type":"input_text","text":"<environment_context>cwd</environment_context>"}
            ]}}),
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[
                {"type":"input_text","text":"[Image #1]"},{"type":"input_image","image_url":"not-text"},
                {"type":"input_text","text":"Fix workspace names across restarts"}
            ]}}),
        ];
        fs::write(
            &path,
            rows.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert_eq!(
            request_in_rollout(File::open(&path).unwrap(), ID).as_deref(),
            Some("Fix workspace names across restarts")
        );
        assert!(request_in_rollout(File::open(&path).unwrap(), "different-id").is_none());
    }

    #[test]
    fn uses_the_first_real_request_and_prefers_a_saved_rename() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        let millis = i64::from_str_radix("01a08e06c060", 16).unwrap();
        let (y, m, d) = interface_common::time::civil_from_days(millis / 86_400_000);
        let day = sessions.join(format!("{y:04}/{m:02}/{d:02}"));
        fs::create_dir_all(&day).unwrap();
        let rollout = day.join(format!("rollout-{ID}.jsonl"));
        let prompt = "I want you to fix workspace naming across app restarts";
        let rows = [
            serde_json::json!({"type":"session_meta","payload":{"id":ID}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"# AGENTS.md instructions for project"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":prompt}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"Now do something unrelated"}}),
        ];
        fs::write(
            &rollout,
            rows.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let index = temp.path().join("session_index.jsonl");
        fs::write(
            &index,
            serde_json::json!({"id":ID,"thread_name":"I want you to fix workspace"}).to_string(),
        )
        .unwrap();
        let mut names = CodexNames::default();
        assert_eq!(
            names.lookup(&sessions, ID).as_deref(),
            Some("Fix workspace naming across app restarts")
        );
        fs::write(
            &index,
            serde_json::json!({"id":ID,"thread_name":"Reliable pane names"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            names.lookup(&sessions, ID).as_deref(),
            Some("Reliable pane names")
        );
        assert!(names.lookup(&sessions, "../../outside").is_none());
        fs::remove_file(&rollout).unwrap();
        std::os::unix::fs::symlink(&index, &rollout).unwrap();
        assert!(first_request(&sessions, ID).is_none());
    }

    #[test]
    fn an_appended_index_parses_only_new_lines_and_finishes_a_partial_one() {
        use std::io::Write as _;
        const OTHER: &str = "01a08e06-c060-78a2-a66d-3a43629736bf";
        let temp = tempfile::tempdir().unwrap();
        let index = temp.path().join("session_index.jsonl");
        let line = |id: &str, name: &str| {
            format!("{}\n", serde_json::json!({"id": id, "thread_name": name}))
        };
        fs::write(&index, line(ID, "First name")).unwrap();
        let mut names = CodexNames::default();
        names.refresh_index(temp.path());
        assert_eq!(names.index[ID], "First name");
        let first = names.index_stamp.clone().unwrap();

        let renamed = line(ID, "Renamed");
        let partial = line(OTHER, "Other thread");
        let (whole, rest) = partial.split_at(partial.len() / 2);
        let mut file = fs::OpenOptions::new().append(true).open(&index).unwrap();
        file.write_all(renamed.as_bytes()).unwrap();
        file.write_all(whole.as_bytes()).unwrap();
        file.sync_all().unwrap();
        names.refresh_index(temp.path());
        let appended = names.index_stamp.clone().unwrap();
        assert_eq!(appended.window_start, first.window_start);
        assert_eq!(appended.parsed_to, first.parsed_to + renamed.len() as u64);
        assert_eq!(names.index[ID], "Renamed");
        assert!(!names.index.contains_key(OTHER));

        file.write_all(rest.as_bytes()).unwrap();
        file.sync_all().unwrap();
        names.refresh_index(temp.path());
        assert_eq!(names.index[ID], "Renamed");
        assert_eq!(names.index[OTHER], "Other thread");

        // Rewriting the same file longer is not mistaken for an append.
        drop(file);
        let rewritten = format!(
            "{}{}",
            line(OTHER, &"Rewritten in place with a longer name ".repeat(8)),
            line(OTHER, "Rewritten twice")
        );
        assert!(rewritten.len() as u64 > fs::metadata(&index).unwrap().len());
        fs::write(&index, rewritten).unwrap();
        names.refresh_index(temp.path());
        assert!(!names.index.contains_key(ID));
        assert_eq!(names.index[OTHER], "Rewritten twice");

        // A replaced file is read from scratch.
        fs::remove_file(&index).unwrap();
        fs::write(&index, line(ID, "Only survivor")).unwrap();
        names.refresh_index(temp.path());
        assert!(!names.index.contains_key(OTHER));
        assert_eq!(names.index[ID], "Only survivor");
    }

    #[test]
    fn a_missing_first_request_is_not_rescanned_until_its_retry_is_due() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        let millis = i64::from_str_radix("01a08e06c060", 16).unwrap();
        let (y, m, d) = interface_common::time::civil_from_days(millis / 86_400_000);
        let day = sessions.join(format!("{y:04}/{m:02}/{d:02}"));
        fs::create_dir_all(&day).unwrap();
        let mut names = CodexNames::default();
        assert!(names.lookup(&sessions, ID).is_none());

        let rows = [
            serde_json::json!({"type":"session_meta","payload":{"id":ID}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"Name the panes well"}}),
        ];
        fs::write(
            day.join(format!("rollout-{ID}.jsonl")),
            rows.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        assert!(
            names.lookup(&sessions, ID).is_none(),
            "a recent miss is not rescanned"
        );
        let due = Instant::now()
            .checked_sub(MISSING_PROMPT_RETRY)
            .expect("monotonic clock has run long enough");
        names.missing_prompts.insert(ID.to_owned(), due);
        assert_eq!(
            names.lookup(&sessions, ID).as_deref(),
            Some("Name the panes well")
        );
        assert!(names.missing_prompts.is_empty());
    }
}
