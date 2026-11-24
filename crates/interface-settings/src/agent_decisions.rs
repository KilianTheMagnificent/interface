//! Per-agent approval decision log: append-only JSONL segments under the private data root.
//!
//! `<dir>/<hex(agent id)>/segment-<20-digit id>.jsonl`. A segment never grows past 256 KiB and
//! only the newest 16 are kept. A crash can tear the last line; readers skip unparsable lines.
//!
//! An append is a plain `fsync(2)`: it survives a process or OS crash, and only a power loss
//! can drop the newest lines. A segment rotation drains the drive cache (`F_FULLFSYNC` on
//! Apple), so a completed segment is power-loss durable. A failed append is never acted on.
use crate::SettingsError;
use interface_contracts::agents::AgentDecision;
use std::{
    collections::HashMap,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub const SEGMENT_LIMIT: usize = 256 * 1024;
const RETAINED_SEGMENTS: usize = 16;
const READ_LIMIT: usize = 200;

pub struct AgentDecisionLog {
    dir: PathBuf,
    /// Each agent's open segment after a successful append, so the next one
    /// is a write and an fsync rather than a directory scan and an open.
    active: HashMap<String, ActiveSegment>,
    /// Segments opened, for the handle-reuse tests.
    #[cfg(test)]
    opens: usize,
}

struct ActiveSegment {
    file: fs::File,
    id: u64,
    /// The length after this log's last append, which ended with a newline.
    len: u64,
}

impl ActiveSegment {
    /// Still the segment's file, untouched since this log's last append: an
    /// outside edit, truncation or removal sends the next append back to the
    /// directory.
    fn is_current(&self) -> bool {
        self.file
            .metadata()
            .is_ok_and(|metadata| metadata.nlink() > 0 && metadata.len() == self.len)
    }
}

impl AgentDecisionLog {
    /// # Errors
    /// Refuses a relative path or a directory that cannot be created.
    pub fn open(dir: &Path) -> Result<Self, SettingsError> {
        if !dir.is_absolute() {
            return Err(SettingsError::InvalidPath);
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            active: HashMap::new(),
            #[cfg(test)]
            opens: 0,
        })
    }

    /// # Errors
    /// Refuses an entry that cannot fit a segment, or any filesystem failure. The caller treats
    /// a failed append as a reason to ask the owner.
    pub fn append(&mut self, agent_id: &str, entry: &AgentDecision) -> Result<(), SettingsError> {
        let mut line = serde_json::to_vec(entry)
            .map_err(|_| SettingsError::InvalidAgent("Could not encode the decision.".into()))?;
        line.push(b'\n');
        if line.len() > SEGMENT_LIMIT {
            return Err(SettingsError::InvalidAgent(
                "The decision is too large to log.".into(),
            ));
        }
        let dir = self.dir.join(hex(agent_id));
        // Taken out for the append: any failure below leaves no cached handle,
        // so the next append rescans the directory and rechecks the tail.
        let cached = self
            .active
            .remove(agent_id)
            .filter(ActiveSegment::is_current);
        let reused = cached.is_some();
        let (mut id, mut file, mut len) = if let Some(segment) = cached {
            (segment.id, segment.file, segment.len)
        } else {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)?;
            let ids = segment_ids(&dir)?;
            let id = ids.last().copied().unwrap_or(1);
            let file = open_segment(&dir, id)?;
            #[cfg(test)]
            {
                self.opens += 1;
            }
            let len = file.metadata()?.len();
            (id, file, len)
        };
        let rotate = usize::try_from(len)
            .unwrap_or(usize::MAX)
            .saturating_add(line.len())
            > SEGMENT_LIMIT;
        if rotate {
            id = id.checked_add(1).ok_or_else(|| {
                SettingsError::InvalidAgent("Decision segments are exhausted.".into())
            })?;
            file = open_segment(&dir, id)?;
            #[cfg(test)]
            {
                self.opens += 1;
            }
            len = file.metadata()?.len();
        }
        // A reused handle's own last append ended with a newline.
        if len > 0 && (rotate || !reused) {
            // Terminate a torn line so it cannot swallow this entry.
            let mut last = [0_u8; 1];
            file.seek(SeekFrom::Start(len - 1))?;
            file.read_exact(&mut last)?;
            if last[0] != b'\n' {
                line.insert(0, b'\n');
            }
        }
        file.write_all(&line)?;
        if rotate {
            // The drive-cache flush also persists the finished segment's tail.
            file.sync_data()?;
            let ids = segment_ids(&dir)?;
            for old in &ids[..ids.len().saturating_sub(RETAINED_SEGMENTS)] {
                fs::remove_file(segment_path(&dir, *old))?;
            }
        } else {
            hand_off(&file)?;
        }
        let len = len + line.len() as u64;
        self.active
            .insert(agent_id.to_owned(), ActiveSegment { file, id, len });
        Ok(())
    }

    /// Newest first, at most `limit` (capped at 200). Unreadable segments and lines are skipped.
    #[must_use]
    pub fn read(&self, agent_id: &str, limit: usize) -> Vec<AgentDecision> {
        let limit = limit.min(READ_LIMIT);
        let dir = self.dir.join(hex(agent_id));
        let mut decisions = Vec::new();
        let Ok(ids) = segment_ids(&dir) else {
            return decisions;
        };
        for id in ids.into_iter().rev() {
            if decisions.len() >= limit {
                break;
            }
            let Ok(bytes) = read_segment(&segment_path(&dir, id)) else {
                continue;
            };
            decisions.extend(
                bytes
                    .split(|b| *b == b'\n')
                    .rev()
                    .filter_map(|line| serde_json::from_slice::<AgentDecision>(line).ok())
                    .take(limit - decisions.len()),
            );
        }
        decisions
    }
}

/// A plain `fsync(2)`, retried when interrupted as std's syncs are.
fn hand_off(file: &fs::File) -> std::io::Result<()> {
    loop {
        match rustix::fs::fsync(file) {
            Err(rustix::io::Errno::INTR) => {}
            result => return result.map_err(Into::into),
        }
    }
}

fn hex(agent_id: &str) -> String {
    agent_id.bytes().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn segment_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("segment-{id:020}.jsonl"))
}

fn segment_ids(dir: &Path) -> std::io::Result<Vec<u64>> {
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

fn open_segment(dir: &Path, id: u64) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(segment_path(dir, id))
}

fn read_segment(path: &Path) -> std::io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let mut bytes = Vec::new();
    // Segments are written below the limit; read a little past it so an outside edit stays bounded.
    file.take(u64::try_from(SEGMENT_LIMIT * 2).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::agents::{AgentDecisionKind, AgentDeliveryOutcome, ApprovalFacts};
    use std::fs;

    fn entry(at_ms: u64, summary: &str) -> AgentDecision {
        AgentDecision {
            at_ms,
            session_key: "claude#thread".into(),
            approval_id: format!("approval-{at_ms}"),
            tool: "Bash".into(),
            summary: summary.into(),
            facts: ApprovalFacts::Shell {
                command: "ls".into(),
                cwd: None,
            },
            decision: AgentDecisionKind::Allowed,
            reason: "inScopeRead".into(),
            outcome: AgentDeliveryOutcome::Sent,
        }
    }

    fn line_len(entry: &AgentDecision) -> usize {
        serde_json::to_vec(entry).unwrap().len() + 1
    }

    fn segments(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn at(entries: &[AgentDecision]) -> Vec<u64> {
        entries.iter().map(|e| e.at_ms).collect()
    }

    #[test]
    fn append_and_read_newest_first() {
        let temp = tempfile::tempdir().unwrap();
        let mut log = AgentDecisionLog::open(&temp.path().join("agents/decisions")).unwrap();
        for n in 1..=3 {
            log.append("research", &entry(n, "Run tests")).unwrap();
        }
        assert_eq!(
            log.read("research", 10),
            vec![
                entry(3, "Run tests"),
                entry(2, "Run tests"),
                entry(1, "Run tests")
            ]
        );
        assert_eq!(at(&log.read("research", 2)), vec![3, 2]);
        assert!(log.read("research", 0).is_empty());
        assert!(log.read("someone-else", 10).is_empty());
        for n in 4..=205 {
            log.append("research", &entry(n, "x")).unwrap();
        }
        let capped = log.read("research", 1000);
        assert_eq!(capped.len(), 200);
        assert_eq!(capped[0].at_ms, 205);
    }

    #[test]
    fn rotates_at_the_segment_size_boundary() {
        let temp = tempfile::tempdir().unwrap();
        let mut log = AgentDecisionLog::open(temp.path()).unwrap();
        let summary = "s".repeat(1000);
        let per_segment = SEGMENT_LIMIT / line_len(&entry(1_000_000, &summary));
        let total = u64::try_from(per_segment).unwrap() + 1;
        for n in 1..=total {
            log.append("research", &entry(1_000_000 + n, &summary))
                .unwrap();
        }
        let dir = temp.path().join(hex("research"));
        let names = segments(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        let first = fs::read(dir.join(&names[0])).unwrap();
        assert!(first.len() <= SEGMENT_LIMIT);
        assert_eq!(
            std::str::from_utf8(&first).unwrap().lines().count(),
            per_segment
        );
        assert_eq!(
            fs::read(dir.join(&names[1])).unwrap().len(),
            line_len(&entry(1_000_000, &summary))
        );
        let read = log.read("research", 200);
        assert_eq!(read.len(), usize::try_from(total).unwrap().min(200));
        assert_eq!(read[0].at_ms, 1_000_000 + total);
    }

    #[test]
    fn keeps_only_the_newest_sixteen_segments() {
        let temp = tempfile::tempdir().unwrap();
        let mut log = AgentDecisionLog::open(temp.path()).unwrap();
        // Each entry is over half a segment, so every append after the first rotates.
        let summary = "m".repeat(SEGMENT_LIMIT / 2 + 1);
        for n in 1..=20 {
            log.append("research", &entry(n, &summary)).unwrap();
        }
        let dir = temp.path().join(hex("research"));
        let names = segments(&dir);
        assert_eq!(names.len(), 16);
        assert_eq!(names[0], format!("segment-{:020}.jsonl", 5));
        assert_eq!(names[15], format!("segment-{:020}.jsonl", 20));
        assert_eq!(
            at(&log.read("research", 200)),
            (5..=20).rev().collect::<Vec<_>>()
        );
    }

    #[test]
    fn torn_last_line_is_skipped_and_does_not_swallow_the_next_entry() {
        let temp = tempfile::tempdir().unwrap();
        let mut log = AgentDecisionLog::open(temp.path()).unwrap();
        log.append("research", &entry(1, "one")).unwrap();
        log.append("research", &entry(2, "two")).unwrap();
        let dir = temp.path().join(hex("research"));
        let active = dir.join(&segments(&dir)[0]);
        let mut bytes = fs::read(&active).unwrap();
        bytes.extend_from_slice(br#"{"atMs":3,"sessionKey":"#);
        fs::write(&active, bytes).unwrap();
        assert_eq!(at(&log.read("research", 10)), vec![2, 1]);
        log.append("research", &entry(4, "four")).unwrap();
        assert_eq!(at(&log.read("research", 10)), vec![4, 2, 1]);
    }

    #[test]
    fn appends_reuse_one_segment_handle_until_a_rotation_or_an_outside_edit() {
        let temp = tempfile::tempdir().unwrap();
        let mut log = AgentDecisionLog::open(temp.path()).unwrap();
        for n in 1..=3 {
            log.append("research", &entry(n, "Run tests")).unwrap();
        }
        assert_eq!(log.opens, 1);
        log.append("writer", &entry(1, "Other agent")).unwrap();
        assert_eq!(log.opens, 2);
        // Two entries over half a segment each cannot share one: rotate.
        let big = "b".repeat(SEGMENT_LIMIT / 2 + 1);
        log.append("research", &entry(4, &big)).unwrap();
        assert_eq!(log.opens, 2);
        log.append("research", &entry(5, &big)).unwrap();
        assert_eq!(log.opens, 3);
        // A segment changed outside the log is reopened and its tail checked.
        let dir = temp.path().join(hex("research"));
        let active = dir.join(segments(&dir).last().unwrap());
        let mut bytes = fs::read(&active).unwrap();
        bytes.extend_from_slice(br#"{"atMs":6,"#);
        fs::write(&active, bytes).unwrap();
        log.append("research", &entry(7, "seven")).unwrap();
        assert_eq!(log.opens, 4);
        assert_eq!(at(&log.read("research", 3)), vec![7, 5, 4]);
    }

    #[test]
    fn agent_ids_become_hex_directory_names() {
        let temp = tempfile::tempdir().unwrap();
        let mut log = AgentDecisionLog::open(temp.path()).unwrap();
        log.append("../evil/one", &entry(1, "one")).unwrap();
        assert_eq!(hex("../evil/one"), "2e2e2f6576696c2f6f6e65");
        assert_eq!(
            segments(temp.path()),
            vec!["2e2e2f6576696c2f6f6e65".to_owned()]
        );
        assert!(!temp.path().parent().unwrap().join("evil").exists());
        assert_eq!(at(&log.read("../evil/one", 10)), vec![1]);
    }
}
