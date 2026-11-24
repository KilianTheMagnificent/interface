use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use interface_contracts::engine::EngineId;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{
    private_write::{ensure_private_directory, open_owner_append},
    sanitize::for_display,
};

#[derive(Clone, Debug)]
pub struct PromptHistory {
    file: Arc<PathBuf>,
    append_lock: Arc<Mutex<()>>,
}

#[derive(Serialize)]
struct PromptHistoryEntry<'a> {
    at: &'a str,
    engine: &'a EngineId,
    cwd: String,
    text: String,
}

impl PromptHistory {
    /// Opens the exact append-only legacy prompt-history path without reading it.
    ///
    /// # Errors
    ///
    /// Returns an error when `file` is not absolute or its owner-private parent
    /// directory cannot be created.
    pub fn new(file: impl Into<PathBuf>) -> Result<Self, String> {
        let file = file.into();
        if !file.is_absolute() {
            return Err("CLI history path must be absolute".into());
        }
        let parent = file
            .parent()
            .ok_or_else(|| "CLI history path has no parent".to_owned())?;
        ensure_private_directory(parent)?;
        Ok(Self {
            file: Arc::new(file),
            append_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Serializes and appends one sanitized NDJSON record.
    ///
    /// # Errors
    ///
    /// Returns an error when the append-only file cannot be opened without
    /// following symlinks, written, or synchronized.
    pub async fn append(
        &self,
        at: &str,
        engine: &EngineId,
        cwd: &str,
        text: &str,
    ) -> Result<(), String> {
        let mut line = serde_json::to_vec(&PromptHistoryEntry {
            at,
            engine,
            cwd: for_display(cwd),
            text: for_display(text),
        })
        .map_err(|error| error.to_string())?;
        line.push(b'\n');
        // The lock is held across the await so appends stay serialized while the
        // blocking open/write/fsync runs off the reactor.
        let _guard = self.append_lock.lock().await;
        let file = Arc::clone(&self.file);
        tokio::task::spawn_blocking(move || append_record(&file, &line))
            .await
            .unwrap_or_else(|error| Err(error.to_string()))
    }

    /// Waits until every preceding append has released the serialized writer.
    pub async fn flush(&self) {
        drop(self.append_lock.lock().await);
    }
}

fn append_record(file: &Path, line: &[u8]) -> Result<(), String> {
    let mut handle = open_owner_append(file)?;
    handle.write_all(line).map_err(|error| error.to_string())?;
    handle.sync_data().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::MetadataExt};

    use super::*;

    #[tokio::test]
    async fn serializes_appends_with_owner_only_modes_and_no_terminal_controls() {
        let root = tempfile::tempdir().expect("temp");
        let file = root.path().join("nested/history");
        let history = PromptHistory::new(&file).expect("history");
        let claude = EngineId::new("claude").expect("claude");
        let codex = EngineId::new("codex").expect("codex");
        let (first, second) = tokio::join!(
            history.append(
                "2026-08-20T00:00:00.000Z",
                &claude,
                "/work",
                "one\u{1b}]52;c;payload\u{7}"
            ),
            history.append("2026-08-20T00:00:01.000Z", &codex, "/work", "two")
        );
        first.expect("first append");
        second.expect("second append");
        history.flush().await;

        let source = fs::read_to_string(&file).expect("source");
        assert!(!source.contains(['\u{1b}', '\u{7}']));
        assert_eq!(
            source
                .lines()
                .map(
                    |line| serde_json::from_str::<serde_json::Value>(line).expect("json")["text"]
                        .as_str()
                        .expect("text")
                        .to_owned()
                )
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(fs::metadata(&file).expect("metadata").mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(file.parent().expect("parent"))
                .expect("parent metadata")
                .mode()
                & 0o777,
            0o700
        );
    }

    #[tokio::test]
    async fn refuses_to_follow_a_history_symlink() {
        let root = tempfile::tempdir().expect("temp");
        let target = root.path().join("target");
        fs::write(&target, b"outside\n").expect("target");
        let file = root.path().join("history");
        std::os::unix::fs::symlink(&target, &file).expect("symlink");
        let history = PromptHistory::new(&file).expect("history");
        let error = history
            .append(
                "2026-08-20T00:00:00.000Z",
                &EngineId::new("claude").expect("engine"),
                "/work",
                "one",
            )
            .await
            .expect_err("symlink refused");
        assert!(!error.is_empty());
        assert_eq!(fs::read(&target).expect("target unchanged"), b"outside\n");
    }
}
