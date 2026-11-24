//! A size-capped, owner-private append log with one rotated predecessor.
//!
//! The log is diagnostic, not authoritative: an append that fails is dropped
//! (the caller still has stderr), and nothing here fsyncs. What it does
//! guarantee is that the file is a regular, owner-only (`0600`) file in a
//! private directory, never a link, and that the pair never holds more than
//! twice `max_bytes`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::OWNER_FILE_MODE;
use crate::error::{Result, StorageError};
use crate::secure::PrivateDirectory;

/// Suffix of the one rotated predecessor: `host.log` becomes `host.log.1`.
pub const ROTATED_LOG_SUFFIX: &str = ".1";

/// An append-only log file that starts over (keeping one predecessor) once
/// the next append would take it past `max_bytes`.
#[derive(Debug)]
pub struct RotatingLogFile {
    directory: PathBuf,
    name: String,
    max_bytes: u64,
    file: Option<File>,
    written: u64,
    owner_uid: u32,
}

impl RotatingLogFile {
    /// Opens (creating when absent) `name` in `directory` for appending.
    ///
    /// # Errors
    ///
    /// Refuses an invalid name, a link or non-regular file in its place, a
    /// file owned by someone else, and any open failure.
    pub fn open(directory: &PrivateDirectory, name: &str, max_bytes: u64) -> Result<Self> {
        let path = directory.entry_path(name)?;
        let owner_uid = directory.owner_uid();
        let file = open_append(&path, owner_uid)?;
        let written = file
            .metadata()
            .map_err(|error| StorageError::io("inspect log file", &path, error))?
            .len();
        Ok(Self {
            directory: directory.path().to_path_buf(),
            name: name.to_owned(),
            max_bytes: max_bytes.max(1),
            file: Some(file),
            written,
            owner_uid,
        })
    }

    /// The live file's path.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.directory.join(&self.name)
    }

    /// The rotated predecessor's path.
    #[must_use]
    pub fn rotated_path(&self) -> PathBuf {
        self.directory
            .join(format!("{}{ROTATED_LOG_SUFFIX}", self.name))
    }

    /// Appends `bytes` whole. When they would take the file past its cap
    /// and it already holds something, the file becomes the predecessor
    /// first (replacing the old one), so one append never straddles two files.
    ///
    /// # Errors
    ///
    /// The rotation or write failed; the next append tries again.
    pub fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        let incoming = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.written > 0 && self.written.saturating_add(incoming) > self.max_bytes {
            self.rotate()?;
        }
        if self.file.is_none() {
            let path = self.path();
            let file = open_append(&path, self.owner_uid).map_err(io::Error::other)?;
            self.written = file.metadata()?.len();
            self.file = Some(file);
        }
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file closed"))?;
        match file.write_all(bytes) {
            Ok(()) => {
                self.written = self.written.saturating_add(incoming);
                Ok(())
            }
            Err(error) => {
                // Reopen next time: the file may have been removed or replaced.
                self.file = None;
                Err(error)
            }
        }
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        fs::rename(self.path(), self.rotated_path())?;
        self.written = 0;
        Ok(())
    }
}

fn open_append(path: &Path, owner_uid: u32) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(OWNER_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| StorageError::io("open log file", path, error))?;
    let metadata = file
        .metadata()
        .map_err(|error| StorageError::io("inspect log file", path, error))?;
    if !metadata.is_file() {
        return Err(StorageError::unsafe_at(
            path,
            "log file is not a regular file",
        ));
    }
    if metadata.uid() != owner_uid {
        return Err(StorageError::unsafe_at(
            path,
            "log file belongs to another user",
        ));
    }
    if metadata.mode() & 0o7777 != OWNER_FILE_MODE {
        // Through the open descriptor, never by name.
        file.set_permissions(fs::Permissions::from_mode(OWNER_FILE_MODE))
            .map_err(|error| StorageError::io("make log file owner-only", path, error))?;
    }
    Ok(file)
}

/// The last `max_lines` lines of `name` in `directory`, continuing into its
/// rotated predecessor when the live file holds fewer. Reads at most
/// `window` bytes from the end of each file; a line cut by that window is
/// dropped. Missing or unreadable files read as empty: this is for showing
/// someone recent trouble, never for deciding anything.
#[must_use]
pub fn read_log_tail(directory: &Path, name: &str, max_lines: usize, window: u64) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let mut lines = tail_lines(&directory.join(name), max_lines, window);
    if lines.len() < max_lines {
        let mut older = tail_lines(
            &directory.join(format!("{name}{ROTATED_LOG_SUFFIX}")),
            max_lines - lines.len(),
            window,
        );
        older.append(&mut lines);
        lines = older;
    }
    lines
}

fn tail_lines(path: &Path, max_lines: usize, window: u64) -> Vec<String> {
    let Ok(mut file) = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    else {
        return Vec::new();
    };
    let Ok(metadata) = file.metadata() else {
        return Vec::new();
    };
    if !metadata.is_file() {
        return Vec::new();
    }
    let start = metadata.len().saturating_sub(window);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(window).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines().collect::<Vec<_>>();
    if start > 0 && !lines.is_empty() {
        // The window began inside a line.
        lines.remove(0);
    }
    let skip = lines.len().saturating_sub(max_lines);
    lines
        .into_iter()
        .skip(skip)
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn private_root() -> (tempfile::TempDir, PrivateDirectory) {
        let temporary = tempfile::tempdir().expect("temporary root");
        let path = temporary.path().join("data");
        let directory = PrivateDirectory::open_or_create(&path).expect("private directory");
        (temporary, directory)
    }

    #[test]
    fn host_log_rotates_at_its_cap_and_keeps_one_owner_only_predecessor() {
        let (_temporary, directory) = private_root();
        let mut log = RotatingLogFile::open(&directory, "host.log", 32).expect("open log");
        log.append(b"first line of warnings\n").expect("append");
        log.append(b"second line\n").expect("append");
        // 23 + 12 > 32: the second append started the file over.
        let live = fs::read_to_string(log.path()).expect("live");
        let rotated = fs::read_to_string(log.rotated_path()).expect("rotated");
        assert_eq!(live, "second line\n");
        assert_eq!(rotated, "first line of warnings\n");
        log.append(b"third\n").expect("append");
        log.append(b"fourth line that overflows\n").expect("append");
        assert_eq!(
            fs::read_to_string(log.rotated_path()).expect("rotated"),
            "second line\nthird\n",
            "the older predecessor is replaced, never a third file"
        );
        for path in [log.path(), log.rotated_path()] {
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} is owner-only", path.display());
        }
        let names = fs::read_dir(directory.path())
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            names,
            ["host.log", "host.log.1"]
                .map(str::to_owned)
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn an_existing_log_is_appended_to_and_made_owner_only() {
        let (_temporary, directory) = private_root();
        let path = directory.path().join("host.log");
        fs::write(&path, "earlier\n").expect("seed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
        let mut log = RotatingLogFile::open(&directory, "host.log", 1024).expect("open");
        log.append(b"later\n").expect("append");
        assert_eq!(fs::read_to_string(&path).expect("read"), "earlier\nlater\n");
        assert_eq!(
            fs::metadata(&path).expect("meta").permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_link_in_the_logs_place_is_refused() {
        let (temporary, directory) = private_root();
        let elsewhere = temporary.path().join("elsewhere.log");
        fs::write(&elsewhere, "").expect("target");
        std::os::unix::fs::symlink(&elsewhere, directory.path().join("host.log")).expect("link");
        assert!(RotatingLogFile::open(&directory, "host.log", 1024).is_err());
    }

    #[test]
    fn the_tail_reads_the_newest_lines_across_the_rotation() {
        let (_temporary, directory) = private_root();
        fs::write(directory.path().join("host.log.1"), "a\nb\nc\n").expect("rotated");
        fs::write(directory.path().join("host.log"), "d\ne\n").expect("live");
        assert_eq!(
            read_log_tail(directory.path(), "host.log", 3, 1024),
            ["c", "d", "e"]
        );
        assert_eq!(
            read_log_tail(directory.path(), "host.log", 2, 1024),
            ["d", "e"]
        );
        assert_eq!(
            read_log_tail(directory.path(), "host.log", 10, 1024),
            ["a", "b", "c", "d", "e"]
        );
        // A window that starts mid-line drops the cut line.
        fs::write(directory.path().join("host.log"), "0123456789\nlast\n").expect("live");
        fs::remove_file(directory.path().join("host.log.1")).expect("remove");
        assert_eq!(read_log_tail(directory.path(), "host.log", 5, 8), ["last"]);
        assert!(read_log_tail(directory.path(), "missing.log", 5, 1024).is_empty());
    }
}
