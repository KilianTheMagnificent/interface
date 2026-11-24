//! `host.log`: the host's WARN and ERROR lines, kept where a user can reach
//! them. Electron forwards the host's stderr only to its own console, which a
//! packaged app shows nobody, so every warning used to vanish there.
//!
//! The file lives in the private data root, owner-only, and rotates at
//! [`HOST_LOG_MAX_BYTES`] keeping one predecessor (`host.log.1`): at most two
//! files of 5 MB. It attaches once the data root is known; anything logged
//! before that still reaches stderr. Help → Reveal Logs shows it and Copy
//! Diagnostics quotes its tail (`crate::diagnostics`).

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use interface_storage::{PrivateDirectory, RotatingLogFile, StorageError, read_log_tail};
use tracing_subscriber::fmt::MakeWriter;

pub const HOST_LOG_FILE_NAME: &str = "host.log";
pub const HOST_LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;
/// How far back from the end of each file a tail read looks.
const TAIL_WINDOW_BYTES: u64 = 256 * 1024;

/// The file half of the host's log writer. Cheap to clone; every clone
/// writes to the one file once [`Self::attach`] has run.
#[derive(Clone, Default)]
pub struct HostLogSink {
    file: Arc<OnceLock<Mutex<RotatingLogFile>>>,
}

impl HostLogSink {
    /// Starts writing to `host.log` in `data_root`, creating the folder's
    /// last component when it is missing. Only the first call attaches.
    ///
    /// # Errors
    ///
    /// The data root or the file is unsafe or could not be opened.
    pub fn attach(&self, data_root: &Path) -> Result<PathBuf, StorageError> {
        let directory = PrivateDirectory::open_or_create(data_root)?;
        let file = RotatingLogFile::open(&directory, HOST_LOG_FILE_NAME, HOST_LOG_MAX_BYTES)?;
        let path = file.path();
        let _ = self.file.set(Mutex::new(file));
        Ok(path)
    }

    fn append(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if let Some(file) = self.file.get() {
            // A full disk or a removed file must never take a log call down.
            let _ = interface_common::sync::lock(file).append(bytes);
        }
    }
}

/// One event's bytes, appended whole when the formatter drops it, so a
/// rotation never splits a line.
pub struct HostLogWriter {
    sink: HostLogSink,
    buffer: Vec<u8>,
}

impl Write for HostLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for HostLogWriter {
    fn drop(&mut self) {
        self.sink.append(&self.buffer);
    }
}

impl<'a> MakeWriter<'a> for HostLogSink {
    type Writer = HostLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        HostLogWriter {
            sink: self.clone(),
            buffer: Vec::new(),
        }
    }
}

/// The newest `lines` lines of `host.log` (and its predecessor) in `data_root`.
#[must_use]
pub fn recent_host_log_lines(data_root: &Path, lines: usize) -> Vec<String> {
    read_log_tail(data_root, HOST_LOG_FILE_NAME, lines, TAIL_WINDOW_BYTES)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tracing_subscriber::fmt::writer::MakeWriterExt;

    use super::*;

    #[test]
    fn host_log_keeps_warnings_and_errors_but_not_info() {
        let temporary = tempfile::tempdir().expect("temporary");
        let data_root = temporary.path().join("Interface");
        let sink = HostLogSink::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(io::sink.and(sink.clone().with_max_level(tracing::Level::WARN)))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("before the data root is known");
            let path = sink.attach(&data_root).expect("attach");
            assert_eq!(path, data_root.join("host.log"));
            tracing::info!("an ordinary step");
            tracing::warn!(store = "threads", "a store was set aside");
            tracing::error!("the broker did not open");
        });
        let text = std::fs::read_to_string(data_root.join("host.log")).expect("log");
        assert!(!text.contains("before the data root"), "{text}");
        assert!(!text.contains("an ordinary step"), "{text}");
        assert!(
            text.contains("WARN") && text.contains("a store was set aside"),
            "{text}"
        );
        assert!(text.contains("store=\"threads\""), "{text}");
        assert!(
            text.contains("ERROR") && text.contains("the broker did not open"),
            "{text}"
        );
        let mode = std::fs::metadata(data_root.join("host.log"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let recent = recent_host_log_lines(&data_root, 1);
        assert_eq!(recent.len(), 1);
        assert!(recent[0].contains("the broker did not open"));
    }
}
