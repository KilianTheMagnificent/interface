//! The journal's one writer: segment files, rotation, retention, the
//! directory bounds, quarantine and the restart marker.
//!
//! An append is a plain write; [`ThreadJournal::sync`] makes a thread's
//! appends durable with a plain `fsync(2)` (the host calls it at a turn's
//! end, at most 2 s after an unsynced append and in the shutdown drain). A
//! rotation drains the drive cache for the finished segment, as the agent
//! decision log does.

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use interface_contracts::{
    thread_journal::{
        JOURNAL_FORMAT_VERSION, JournalEntry, JournalHeader, JournalLine, TurnOrigin,
    },
    threads::RunOutcome,
};
use interface_storage::{PrivateDirectory, WriterLock};

use super::{
    ENTRY_LIMIT, JOURNAL_DIR, MAX_THREADS, MAX_TOTAL_BYTES, QUARANTINE_DIR, RETAINED_SEGMENTS,
    SEGMENT_LIMIT, bounds::bounded_line_within, import::IMPORT_REASON, thread_hash,
};
use crate::SettingsError;

const LOCK_NAME: &str = "journal.lock";
/// In a thread's directory: its journal was imported from the provider's
/// whole history (or checked against it), so it is never rebuilt again.
pub(super) const WHOLE_IMPORT_MARKER: &str = "imported-whole";
/// A thread directory being rebuilt is staged as `<h>` with this suffix.
const STAGING_SUFFIX: &str = ".rebuild";
/// The restart marker's `reason`.
pub(super) const RESTART_REASON: &str = "restart";

/// The journal's bounds; tests shrink them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JournalLimits {
    pub entry_bytes: usize,
    pub segment_bytes: usize,
    pub retained_segments: usize,
    pub max_threads: usize,
    pub max_total_bytes: u64,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            entry_bytes: ENTRY_LIMIT,
            segment_bytes: SEGMENT_LIMIT,
            retained_segments: RETAINED_SEGMENTS,
            max_threads: MAX_THREADS,
            max_total_bytes: MAX_TOTAL_BYTES,
        }
    }
}

/// The writer. Only the process holding `host.lock` opens one; it also
/// holds the journal directory's own writer lock.
pub struct ThreadJournal {
    root: PathBuf,
    directory: PrivateDirectory,
    _lock: WriterLock,
    limits: JournalLimits,
    threads: HashMap<String, Thread>,
    total_bytes: u64,
    quarantined: Vec<PathBuf>,
}

#[derive(Debug)]
struct Segment {
    first_seq: u64,
    len: u64,
}

#[derive(Debug)]
struct Thread {
    hash: String,
    /// Oldest first.
    segments: Vec<Segment>,
    last_seq: u64,
    turn: Option<u32>,
    /// A `turnStarted` with no `turnEnded` or `turnRetracted` after it.
    open: bool,
    last_at: u64,
    /// The newest segment, open for appending.
    file: Option<fs::File>,
    dirty: bool,
    /// Whether every line so far came from a provider history import.
    import: Imported,
}

/// How much of a thread's journal came from a provider history import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Imported {
    /// Every line so far: imported turns and their entries (and, before the
    /// first of them, only what an import that began mid-turn left).
    Inside,
    /// Every line, the last an imported turn's end: anything after it but
    /// another imported turn was written live.
    Ended,
    /// A line was written live.
    No,
}

impl Thread {
    fn new(hash: String) -> Self {
        Self {
            hash,
            segments: Vec::new(),
            last_seq: 0,
            turn: None,
            open: false,
            last_at: 0,
            file: None,
            dirty: false,
            import: Imported::Inside,
        }
    }

    fn note(&mut self, line: &JournalLine) {
        self.last_seq = self.last_seq.max(line.seq);
        self.turn = line.turn.or(self.turn);
        self.last_at = self.last_at.max(line.at);
        match line.entry {
            JournalEntry::TurnStarted { .. } => self.open = true,
            JournalEntry::TurnEnded { .. } | JournalEntry::TurnRetracted {} => self.open = false,
            _ => {}
        }
        if self.import == Imported::No {
            return;
        }
        self.import = match &line.entry {
            JournalEntry::TurnStarted { origin, .. } if *origin == TurnOrigin::Import => {
                Imported::Inside
            }
            JournalEntry::TurnEnded { reason, .. } if reason.as_deref() == Some(IMPORT_REASON) => {
                Imported::Ended
            }
            JournalEntry::TurnStarted { .. }
            | JournalEntry::TurnEnded { .. }
            | JournalEntry::TurnRetracted {}
            | JournalEntry::SessionClosed {}
            | JournalEntry::Visual { .. } => Imported::No,
            _ if self.import == Imported::Ended => Imported::No,
            _ => self.import,
        };
    }

    fn imported_only(&self) -> bool {
        self.import != Imported::No
    }

    fn bytes(&self) -> u64 {
        self.segments.iter().map(|segment| segment.len).sum()
    }
}

/// What one segment held at open.
enum Loaded {
    /// Its header and every line parsed; `valid` bytes of it are whole
    /// lines (a torn last line past them is cut off).
    Whole {
        key: String,
        lines: Vec<JournalLine>,
        valid: u64,
    },
    /// Not even its header was written whole.
    Empty,
    /// It does not parse: its key when its header did, and the lines that
    /// did before the first that did not.
    Corrupt {
        key: Option<String>,
        lines: Vec<JournalLine>,
    },
}

impl ThreadJournal {
    /// Opens (creating) `<data_root>/thread-journal`, takes its writer lock,
    /// reads every thread's segments, cuts torn last lines off and moves
    /// segments that do not parse to `thread-journal/quarantine/`.
    ///
    /// # Errors
    /// A relative path, an unsafe or unreadable directory, or a lock another
    /// writer holds.
    pub fn open(data_root: &Path) -> Result<Self, SettingsError> {
        Self::open_with(data_root, JournalLimits::default())
    }

    /// [`ThreadJournal::open`] with other bounds.
    ///
    /// # Errors
    /// As [`ThreadJournal::open`].
    pub fn open_with(data_root: &Path, limits: JournalLimits) -> Result<Self, SettingsError> {
        if !data_root.is_absolute() {
            return Err(SettingsError::InvalidPath);
        }
        let root = data_root.join(JOURNAL_DIR);
        let directory = PrivateDirectory::open_or_create(&root)?;
        let lock = WriterLock::acquire(&directory, LOCK_NAME)?;
        let mut journal = Self {
            root,
            directory,
            _lock: lock,
            limits,
            threads: HashMap::new(),
            total_bytes: 0,
            quarantined: Vec::new(),
        };
        journal.scan()?;
        Ok(journal)
    }

    fn scan(&mut self) -> Result<(), SettingsError> {
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(hash) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            // A rebuild that stopped before (or just after) its swap: the
            // thread's own directory is whole either way.
            if hash
                .strip_suffix(STAGING_SUFFIX)
                .is_some_and(is_thread_hash)
            {
                let _ = remove_staging(&entry.path());
                continue;
            }
            if !is_thread_hash(&hash) || !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some((key, thread)) = self.load_thread(&hash)? {
                self.total_bytes += thread.bytes();
                self.threads.insert(key, thread);
            } else {
                // Nothing readable is left in it.
                let _ = fs::remove_dir(self.root.join(&hash));
            }
        }
        Ok(())
    }

    fn load_thread(&mut self, hash: &str) -> Result<Option<(String, Thread)>, SettingsError> {
        let dir = self.root.join(hash);
        let mut thread = Thread::new(hash.to_owned());
        let mut key: Option<String> = None;
        for id in segment_ids(&dir)? {
            let path = segment_path(&dir, id);
            let loaded = match read_file(&path, self.limits.segment_bytes.saturating_mul(2)) {
                Ok(bytes) => parse_segment(&bytes, hash, id),
                Err(_) => Loaded::Corrupt {
                    key: None,
                    lines: Vec::new(),
                },
            };
            match loaded {
                Loaded::Whole {
                    key: found,
                    lines,
                    valid,
                } if key.as_ref().is_none_or(|key| *key == found)
                    && lines.first().is_none_or(|line| line.seq > thread.last_seq) =>
                {
                    let file_len = fs::metadata(&path)?.len();
                    if valid < file_len {
                        // A torn last line: cut it off so the next append
                        // starts on a line of its own.
                        let file = OpenOptions::new()
                            .write(true)
                            .custom_flags(libc::O_NOFOLLOW)
                            .open(&path)?;
                        file.set_len(valid)?;
                        file.sync_data()?;
                    }
                    for line in &lines {
                        thread.note(line);
                    }
                    key = Some(found);
                    thread.segments.push(Segment {
                        first_seq: id,
                        len: valid,
                    });
                }
                Loaded::Empty => fs::remove_file(&path)?,
                Loaded::Whole {
                    key: found, lines, ..
                }
                | Loaded::Corrupt {
                    key: Some(found),
                    lines,
                } => {
                    // The thread continues after the last readable seq.
                    for line in &lines {
                        thread.last_seq = thread.last_seq.max(line.seq);
                    }
                    key.get_or_insert(found);
                    self.quarantine(hash, id, &path)?;
                }
                Loaded::Corrupt { key: None, .. } => {
                    self.quarantine(hash, id, &path)?;
                }
            }
        }
        Ok(key.map(|key| (key, thread)))
    }

    fn quarantine(&mut self, hash: &str, id: u64, path: &Path) -> Result<(), SettingsError> {
        let quarantine = self.directory.open_or_create_child(QUARANTINE_DIR)?;
        let base = format!("{hash}-segment-{id:020}");
        let mut target = quarantine.path().join(format!("{base}.jsonl"));
        let mut n = 1_u32;
        while fs::symlink_metadata(&target).is_ok() {
            n += 1;
            target = quarantine.path().join(format!("{base}-{n}.jsonl"));
        }
        fs::rename(path, &target)?;
        quarantine.sync()?;
        self.quarantined.push(target);
        Ok(())
    }

    /// Segments moved to quarantine since the last call, for the store
    /// notices.
    pub fn take_quarantined(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.quarantined)
    }

    /// The journal's directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `key` has a journal.
    #[must_use]
    pub fn has_journal(&self, key: &str) -> bool {
        self.threads.contains_key(key)
    }

    /// Journals that hold only an import from before imports read the whole
    /// history (no live line, at least one line, not yet checked): each may
    /// be shorter than its provider's history.
    #[must_use]
    pub fn short_import_candidates(&self) -> Vec<String> {
        let mut keys = self
            .threads
            .iter()
            .filter(|(_, thread)| {
                thread.imported_only()
                    && thread.last_seq > 0
                    && fs::symlink_metadata(self.root.join(&thread.hash).join(WHOLE_IMPORT_MARKER))
                        .is_err()
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.sort();
        keys
    }

    /// Whether every line of `key`'s journal came from an import.
    #[must_use]
    pub fn imported_only(&self, key: &str) -> bool {
        self.threads.get(key).is_some_and(Thread::imported_only)
    }

    /// Marks `key`'s journal as holding (or checked against) its provider's
    /// whole history, so it is never rebuilt.
    ///
    /// # Errors
    /// The marker could not be written.
    pub fn mark_whole_import(&mut self, key: &str) -> Result<(), SettingsError> {
        let Some(thread) = self.threads.get(key) else {
            return Ok(());
        };
        write_marker(&self.root.join(&thread.hash))
    }

    /// Replaces `key`'s journal with `entries` (an import of its provider's
    /// whole history): written beside it, then swapped in atomically. On any
    /// error before the swap the old journal stays as it was.
    ///
    /// # Errors
    /// Why the new journal was not swapped in.
    pub fn replace_with_import(
        &mut self,
        key: &str,
        entries: Vec<(u64, JournalEntry)>,
    ) -> Result<(), SettingsError> {
        let Some(thread) = self.threads.get_mut(key) else {
            return Ok(());
        };
        let hash = thread.hash.clone();
        let live = self.root.join(&hash);
        let staging_name = format!("{hash}{STAGING_SUFFIX}");
        let staging = self.root.join(&staging_name);
        let swapped = (|| {
            remove_staging(&staging)?;
            let dir = self.directory.open_or_create_child(&staging_name)?;
            write_staged(dir.path(), key, entries, self.limits)?;
            write_marker(dir.path())?;
            dir.sync()?;
            // The old segment's handle goes before the swap; appends reopen
            // the new newest segment.
            if let Some(thread) = self.threads.get_mut(key) {
                thread.file = None;
            }
            rustix::fs::renameat_with(
                rustix::fs::CWD,
                &staging,
                rustix::fs::CWD,
                &live,
                rustix::fs::RenameFlags::EXCHANGE,
            )
            .map_err(std::io::Error::from)?;
            self.directory.sync()?;
            Ok::<_, SettingsError>(())
        })();
        if let Err(error) = swapped {
            // The old journal was never touched.
            let _ = remove_staging(&staging);
            return Err(error);
        }
        // The staging directory now holds the old journal.
        let _ = remove_staging(&staging);
        if let Some(old) = self.threads.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.bytes());
        }
        if let Some((found, thread)) = self.load_thread(&hash)? {
            self.total_bytes += thread.bytes();
            self.threads.insert(found, thread);
        }
        self.enforce_bounds(key)
    }

    /// Whether `key`'s journal has had a `turnStarted`.
    #[must_use]
    pub fn has_turn(&self, key: &str) -> bool {
        self.threads
            .get(key)
            .is_some_and(|thread| thread.turn.is_some())
    }

    /// Whether `key`'s journal ends inside a turn.
    #[must_use]
    pub fn turn_open(&self, key: &str) -> bool {
        self.threads.get(key).is_some_and(|thread| thread.open)
    }

    /// `key`'s newest seq (0 for none).
    #[must_use]
    pub fn last_seq(&self, key: &str) -> u64 {
        self.threads.get(key).map_or(0, |thread| thread.last_seq)
    }

    /// Every journalled key.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.threads.keys().cloned().collect()
    }

    /// Bytes on disk across every thread.
    #[must_use]
    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Starts `key`'s journal with no entries (a header-only segment), so
    /// it counts as having one. Nothing when it has one.
    ///
    /// # Errors
    /// Any filesystem failure.
    pub fn begin(&mut self, key: &str) -> Result<(), SettingsError> {
        if self.threads.contains_key(key) {
            return Ok(());
        }
        self.add_thread(key)?;
        self.rotate(key, 1)?;
        self.sync(key)
    }

    /// Appends one entry to `key`'s journal (starting it), stamping its
    /// seq and turn, and answers the line as written.
    ///
    /// # Errors
    /// An entry that cannot be bounded, or any filesystem failure; the seq
    /// is not used then.
    ///
    /// # Panics
    /// Never: the thread is added before it is read.
    pub fn append(
        &mut self,
        key: &str,
        at: u64,
        entry: JournalEntry,
    ) -> Result<JournalLine, SettingsError> {
        if !self.threads.contains_key(key) {
            self.add_thread(key)?;
        }
        let thread = self.threads.get(key).expect("added above");
        let turn = match entry {
            JournalEntry::TurnStarted { .. } => {
                Some(thread.turn.map_or(0, |turn| turn.saturating_add(1)))
            }
            _ => thread.turn,
        };
        let mut line = JournalLine {
            seq: thread.last_seq + 1,
            at,
            turn,
            entry,
        };
        let bytes = bounded_line_within(&mut line, self.limits.entry_bytes);
        if bytes.len() > self.limits.entry_bytes {
            return Err(SettingsError::InvalidAgent(
                "A journal entry is too large to record.".into(),
            ));
        }
        let rotate = thread.segments.last().is_none_or(|segment| {
            usize::try_from(segment.len)
                .unwrap_or(usize::MAX)
                .saturating_add(bytes.len())
                > self.limits.segment_bytes
        });
        if rotate {
            self.rotate(key, line.seq)?;
        } else {
            self.ensure_open(key)?;
        }
        let thread = self.threads.get_mut(key).expect("added above");
        let written = match thread.file.as_mut() {
            Some(file) => file.write_all(&bytes),
            None => Err(std::io::Error::other("the journal segment is not open")),
        };
        if let Err(error) = written {
            // Reopen and recheck the tail next time.
            thread.file = None;
            return Err(error.into());
        }
        let grew = bytes.len() as u64;
        if let Some(segment) = thread.segments.last_mut() {
            segment.len += grew;
        }
        thread.note(&line);
        thread.dirty = true;
        self.total_bytes += grew;
        if rotate {
            self.enforce_bounds(key)?;
        }
        Ok(line)
    }

    /// Makes `key`'s appends durable.
    ///
    /// # Errors
    /// The fsync failed.
    pub fn sync(&mut self, key: &str) -> Result<(), SettingsError> {
        let Some(thread) = self.threads.get_mut(key) else {
            return Ok(());
        };
        if let (true, Some(file)) = (thread.dirty, &thread.file) {
            hand_off(file)?;
        }
        thread.dirty = false;
        Ok(())
    }

    /// Makes every thread's appends durable. Answers the first failure
    /// after trying them all.
    ///
    /// # Errors
    /// An fsync failed.
    pub fn sync_all(&mut self) -> Result<(), SettingsError> {
        let mut first = None;
        for key in self.keys() {
            if let Err(error) = self.sync(&key) {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Whether any thread has appends not yet synced.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.threads.values().any(|thread| thread.dirty)
    }

    /// The restart marker: every journal that ends inside a turn gets
    /// `turnEnded {outcome: interrupted, reason: "restart"}`, synced.
    /// Answers those keys.
    ///
    /// # Errors
    /// The first append or sync that failed (the rest are still tried).
    pub fn close_open_turns(&mut self, at: u64) -> Result<Vec<String>, SettingsError> {
        let mut open = self
            .threads
            .iter()
            .filter(|(_, thread)| thread.open)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        open.sort();
        let mut first = None;
        for key in &open {
            let marked = self
                .append(
                    key,
                    at,
                    JournalEntry::TurnEnded {
                        outcome: RunOutcome::Interrupted,
                        reason: Some(RESTART_REASON.into()),
                    },
                )
                .and_then(|_| self.sync(key));
            if let Err(error) = marked {
                first.get_or_insert(error);
            }
        }
        first.map_or(Ok(open), Err)
    }

    /// Removes `key`'s journal (a deleted or forgotten session).
    ///
    /// # Errors
    /// The directory could not be removed.
    pub fn remove(&mut self, key: &str) -> Result<(), SettingsError> {
        if let Some(thread) = self.threads.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(thread.bytes());
        }
        match fs::remove_dir_all(self.root.join(thread_hash(key))) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    fn add_thread(&mut self, key: &str) -> Result<(), SettingsError> {
        while self.threads.len() >= self.limits.max_threads {
            let Some(idlest) = self.idlest(None) else {
                break;
            };
            self.remove(&idlest)?;
        }
        let hash = thread_hash(key);
        self.directory.open_or_create_child(&hash)?;
        self.threads.insert(key.to_owned(), Thread::new(hash));
        Ok(())
    }

    /// The thread idle longest, other than `except`, that has segments.
    fn idlest(&self, except: Option<&str>) -> Option<String> {
        self.threads
            .iter()
            .filter(|(key, thread)| Some(key.as_str()) != except && !thread.segments.is_empty())
            .min_by(|(left_key, left), (right_key, right)| {
                left.last_at
                    .cmp(&right.last_at)
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(key, _)| key.clone())
    }

    /// Starts a new segment for `key` whose first entry is `first_seq`.
    fn rotate(&mut self, key: &str, first_seq: u64) -> Result<(), SettingsError> {
        let thread = self.threads.get_mut(key).expect("known thread");
        if let Some(file) = thread.file.take() {
            // The finished segment is power-loss durable.
            file.sync_data()?;
        }
        let path = segment_path(&self.root.join(&thread.hash), first_seq);
        let mut file = OpenOptions::new()
            .read(true)
            .append(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        let mut header = serde_json::to_vec(&JournalHeader {
            v: JOURNAL_FORMAT_VERSION,
            key: key.to_owned(),
            first_seq,
        })
        .expect("headers always encode");
        header.push(b'\n');
        file.write_all(&header)?;
        thread.segments.push(Segment {
            first_seq,
            len: header.len() as u64,
        });
        thread.file = Some(file);
        thread.dirty = true;
        self.total_bytes += header.len() as u64;
        Ok(())
    }

    /// Retention after `key` rotated: its newest segments only, then the
    /// directory's byte bound, oldest segments of the threads idle longest
    /// first.
    fn enforce_bounds(&mut self, key: &str) -> Result<(), SettingsError> {
        let retained = self.limits.retained_segments.max(1);
        while self
            .threads
            .get(key)
            .is_some_and(|thread| thread.segments.len() > retained)
        {
            self.drop_oldest_segment(key)?;
        }
        while self.total_bytes > self.limits.max_total_bytes {
            let victim = self.idlest(Some(key)).or_else(|| {
                self.threads
                    .get(key)
                    .is_some_and(|thread| thread.segments.len() > 1)
                    .then(|| key.to_owned())
            });
            let Some(victim) = victim else {
                break;
            };
            self.drop_oldest_segment(&victim)?;
        }
        Ok(())
    }

    fn drop_oldest_segment(&mut self, key: &str) -> Result<(), SettingsError> {
        let Some(thread) = self.threads.get_mut(key) else {
            return Ok(());
        };
        if thread.segments.len() <= 1 && thread.file.is_some() {
            // The segment being written goes with its whole thread.
            return self.remove(key);
        }
        let segment = thread.segments.remove(0);
        fs::remove_file(segment_path(
            &self.root.join(&thread.hash),
            segment.first_seq,
        ))?;
        self.total_bytes = self.total_bytes.saturating_sub(segment.len);
        if thread.segments.is_empty() {
            return self.remove(key);
        }
        Ok(())
    }
}

/// Opens a segment for appending, ending a line an earlier writer left
/// open, and answers its length.
fn open_for_append(path: &Path) -> std::io::Result<(fs::File, u64)> {
    let mut file = OpenOptions::new()
        .read(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut len = file.metadata()?.len();
    if len > 0 {
        let mut last = [0_u8; 1];
        file.seek(SeekFrom::Start(len - 1))?;
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
            len += 1;
        }
    }
    Ok((file, len))
}

impl ThreadJournal {
    /// Opens `key`'s newest segment for appending when it is not open.
    fn ensure_open(&mut self, key: &str) -> Result<(), SettingsError> {
        let Some(thread) = self.threads.get_mut(key) else {
            return Ok(());
        };
        if thread.file.is_some() {
            return Ok(());
        }
        let Some(segment) = thread.segments.last_mut() else {
            return Ok(());
        };
        let path = segment_path(&self.root.join(&thread.hash), segment.first_seq);
        let (file, len) = open_for_append(&path)?;
        self.total_bytes = (self.total_bytes + len).saturating_sub(segment.len);
        segment.len = len;
        thread.file = Some(file);
        Ok(())
    }
}

/// A plain `fsync(2)`, retried when interrupted.
fn hand_off(file: &fs::File) -> std::io::Result<()> {
    loop {
        match rustix::fs::fsync(file) {
            Err(rustix::io::Errno::INTR) => {}
            result => return result.map_err(Into::into),
        }
    }
}

/// Removes a staging directory (never following a link out of the journal).
fn remove_staging(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Writes `entries` as `key`'s segments in `dir`, numbered from seq 1 and
/// bounded as appends are, keeping the newest retained segments.
fn write_staged(
    dir: &Path,
    key: &str,
    entries: Vec<(u64, JournalEntry)>,
    limits: JournalLimits,
) -> Result<(), SettingsError> {
    let start = |first_seq: u64| -> Result<(fs::File, usize), SettingsError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(segment_path(dir, first_seq))?;
        let mut header = serde_json::to_vec(&JournalHeader {
            v: JOURNAL_FORMAT_VERSION,
            key: key.to_owned(),
            first_seq,
        })
        .expect("headers always encode");
        header.push(b'\n');
        file.write_all(&header)?;
        Ok((file, header.len()))
    };
    let mut seq = 0_u64;
    let mut turn: Option<u32> = None;
    let mut current: Option<(fs::File, usize)> = None;
    let mut segments = Vec::new();
    for (at, entry) in entries {
        let line_turn = match entry {
            JournalEntry::TurnStarted { .. } => Some(turn.map_or(0, |turn| turn.saturating_add(1))),
            _ => turn,
        };
        let mut line = JournalLine {
            seq: seq + 1,
            at,
            turn: line_turn,
            entry,
        };
        let bytes = bounded_line_within(&mut line, limits.entry_bytes);
        if bytes.len() > limits.entry_bytes {
            // As an append refuses it: the seq is not used.
            continue;
        }
        seq += 1;
        turn = line_turn;
        if current
            .as_ref()
            .is_none_or(|(_, len)| len.saturating_add(bytes.len()) > limits.segment_bytes)
        {
            if let Some((file, _)) = current.take() {
                file.sync_data()?;
            }
            current = Some(start(seq)?);
            segments.push(seq);
        }
        let (file, len) = current.as_mut().expect("started above");
        file.write_all(&bytes)?;
        *len += bytes.len();
    }
    if let Some((file, _)) = current.take() {
        file.sync_data()?;
    }
    let retained = limits.retained_segments.max(1);
    while segments.len() > retained {
        fs::remove_file(segment_path(dir, segments.remove(0)))?;
    }
    Ok(())
}

fn write_marker(dir: &Path) -> Result<(), SettingsError> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(WHOLE_IMPORT_MARKER))?;
    Ok(())
}

pub(super) fn is_thread_hash(name: &str) -> bool {
    name.len() == 16 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub(super) fn segment_path(dir: &Path, first_seq: u64) -> PathBuf {
    dir.join(format!("segment-{first_seq:020}.jsonl"))
}

/// A thread directory's segment ids (first seqs), oldest first.
pub(super) fn segment_ids(dir: &Path) -> std::io::Result<Vec<u64>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        let name = entry?.file_name();
        let id = name
            .to_str()
            .and_then(|n| n.strip_prefix("segment-"))
            .and_then(|n| n.strip_suffix(".jsonl"))
            .filter(|n| n.len() == 20 && n.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|n| n.parse::<u64>().ok());
        ids.extend(id);
    }
    ids.sort_unstable();
    Ok(ids)
}

/// At most `limit` bytes of `path`, never following a link.
pub(super) fn read_file(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Parses one segment for the writer: its header must name a key whose hash
/// is `hash` and `first_seq` its file's; its lines must parse with rising
/// seqs. Only the last line may be torn.
fn parse_segment(bytes: &[u8], hash: &str, first_seq: u64) -> Loaded {
    let mut chunks = bytes.split(|b| *b == b'\n').collect::<Vec<_>>();
    // The empty remainder after a final newline is not a line.
    if chunks.last().is_some_and(|chunk| chunk.is_empty()) {
        chunks.pop();
    }
    let count = chunks.len();
    let mut offset = 0_u64;
    let mut key = None;
    let mut lines: Vec<JournalLine> = Vec::new();
    for (index, chunk) in chunks.into_iter().enumerate() {
        let last = index + 1 == count;
        let end = offset + chunk.len() as u64 + 1;
        if index == 0 {
            match serde_json::from_slice::<JournalHeader>(chunk) {
                Ok(header)
                    if header.v == JOURNAL_FORMAT_VERSION
                        && header.first_seq == first_seq
                        && thread_hash(&header.key) == hash =>
                {
                    key = Some(header.key);
                }
                Err(_) if last => return Loaded::Empty,
                _ => return Loaded::Corrupt { key: None, lines },
            }
        } else {
            match serde_json::from_slice::<JournalLine>(chunk) {
                Ok(line)
                    if line.seq >= first_seq
                        && lines.last().is_none_or(|previous| line.seq > previous.seq) =>
                {
                    lines.push(line);
                }
                Err(_) if last => {
                    return Loaded::Whole {
                        key: key.expect("header read"),
                        lines,
                        valid: offset,
                    };
                }
                _ => {
                    return Loaded::Corrupt {
                        key: key.take(),
                        lines,
                    };
                }
            }
        }
        offset = end;
    }
    match key {
        Some(key) => Loaded::Whole {
            key,
            lines,
            // A whole last line without its newline keeps it; the reopen
            // ends the line before the next append.
            valid: (bytes.len() as u64).min(offset),
        },
        None => Loaded::Empty,
    }
}

#[cfg(test)]
impl ThreadJournal {
    pub(super) fn segment_count(&self, key: &str) -> usize {
        self.threads
            .get(key)
            .map_or(0, |thread| thread.segments.len())
    }

    pub(super) fn thread_count(&self) -> usize {
        self.threads.len()
    }
}
