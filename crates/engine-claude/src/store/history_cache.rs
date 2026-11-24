//! Bounded read-only transcript projections. File identity is checked on every
//! read. Growing files additionally verify the complete old prefix before using
//! its parsed projection: an in-place rewrite must not masquerade as an append.
//! A tail too large to keep parsed is kept as a tail index instead: where each
//! of its records sits, so the next read parses only those again.

use super::cache_file::{
    PrefixHash, READ_ATTEMPTS, changed_during_read, next_record, open_checked, read_exact_at,
    same_identity, same_version, verify_prefix, verify_unchanged,
};
use interface_common::sync::lock;
use serde_json::Value;
use std::{
    collections::VecDeque,
    fs::{File, Metadata},
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

const CACHE_FILES: usize = 8;
const CACHE_BYTES_PER_FILE: usize = 8 * 1024 * 1024;
type Slot = Arc<Mutex<Option<Projection>>>;
type Lru = VecDeque<(PathBuf, usize, Slot)>;
static CACHE: LazyLock<Mutex<Lru>> = LazyLock::new(Mutex::default);

struct Projection {
    metadata: Metadata,
    offset: u64,
    fingerprint: u64,
    matching: usize,
    /// The offset and length of each kept history record, oldest first.
    kept: VecDeque<(u64, usize)>,
    /// Those records parsed, while they fit [`CACHE_BYTES_PER_FILE`] at four
    /// times their bytes. A tail index keeps none and parses them again.
    values: Vec<Value>,
}

impl Projection {
    /// This tail as an index, for a first read of another page size.
    fn index(&self) -> Self {
        Self {
            metadata: self.metadata.clone(),
            offset: self.offset,
            fingerprint: self.fingerprint,
            matching: self.matching,
            kept: self.kept.clone(),
            values: Vec::new(),
        }
    }

    /// Moves the kept records out for a read of `limit` records. A tail kept
    /// for a larger page starts a smaller one with its newest records.
    fn take_window(&mut self, limit: usize) -> Window {
        let mut values = std::mem::take(&mut self.values).into_iter();
        let mut window: Window = self
            .kept
            .drain(..)
            .map(|(at, len)| (at, len, values.next()))
            .collect();
        while window.len() > limit {
            window.pop_front();
        }
        window
    }
}

/// The kept records while a read works on them: offset, length and, once
/// parsed, the record.
type Window = VecDeque<(u64, usize, Option<Value>)>;

pub(super) fn read(path: &Path, limit: usize) -> io::Result<(Vec<Value>, usize)> {
    // The explicit all-history API remains uncached, retaining its old semantics.
    if limit == 0 {
        return super::fold_history_jsonl(path, limit);
    }
    let (slot, sibling) = {
        let mut cache = lock(&CACHE);
        let hit = cache
            .iter()
            .position(|(name, size, _)| name == path && *size == limit);
        let entry = hit
            .and_then(|index| cache.remove(index))
            .unwrap_or_else(|| (path.to_path_buf(), limit, Arc::new(Mutex::new(None))));
        // The same transcript read at another page size, most recent first.
        let sibling = cache
            .iter()
            .rev()
            .find(|(name, _, _)| name == path)
            .map(|(_, _, other)| Arc::clone(other));
        let slot = Arc::clone(&entry.2);
        cache.push_back(entry);
        (slot, sibling)
    };
    let (outcome, cacheable) = {
        let mut cached = lock(&slot);
        if cached.is_none() {
            // A "load older" page starts from the tail a smaller page kept.
            // Never waits on that read, so two page sizes cannot deadlock.
            *cached = sibling
                .as_ref()
                .and_then(|other| other.try_lock().ok()?.as_ref().map(Projection::index));
        }
        let mut attempt = 1;
        let outcome = loop {
            match read_projection(path, limit, &mut cached) {
                Err(error) if changed_during_read(&error) && attempt < READ_ATTEMPTS => {
                    attempt += 1;
                }
                outcome => break outcome,
            }
        };
        (outcome, cached.is_some())
    };
    settle(&mut lock(&CACHE), &slot, cacheable);
    outcome
}

/// Trims the LRU only once the read knows whether it left something worth
/// keeping: a read that failed for any reason but the transcript moving on
/// drops its own empty slot and never evicts a warm one.
fn settle(cache: &mut Lru, slot: &Slot, cacheable: bool) {
    if !cacheable {
        // A slot another reader holds, or refilled meanwhile, stays.
        cache.retain(|(_, _, other)| {
            !Arc::ptr_eq(other, slot) || other.try_lock().map_or(true, |held| held.is_some())
        });
    }
    while cache.len() > CACHE_FILES {
        cache.pop_front();
    }
}

type Page = (Vec<Value>, usize);

/// Reads a page through the projection in `cached`, leaving there what the
/// next read may resume from: the new projection, or only its tail index when
/// its records are too large to keep parsed; a sound tail index when the
/// transcript moved on during the read; and nothing after any other failure.
fn read_projection(path: &Path, limit: usize, cached: &mut Option<Projection>) -> io::Result<Page> {
    let previous = cached.take();
    let named = super::safe_file_metadata(path)?;
    let (mut file, metadata) = match open_checked(path, &named) {
        Ok(opened) => opened,
        Err(error) => {
            if changed_during_read(&error) {
                *cached = previous;
            }
            return Err(error);
        }
    };
    let mut projection = previous
        .filter(|old| same_identity(&old.metadata, &metadata) && old.offset <= metadata.len());
    let mut hash = PrefixHash::new();
    let mut unchanged = false;
    if let Some(old) = &projection {
        if same_version(&old.metadata, &metadata) && old.offset == metadata.len() {
            if old.values.len() == old.kept.len() && old.kept.len() == old.matching.min(limit) {
                let result = page_of(old, None, limit);
                let verified = verify_unchanged(
                    path,
                    &file,
                    &metadata,
                    "transcript changed during cached read",
                );
                if verified.as_ref().err().is_none_or(changed_during_read) {
                    *cached = projection;
                }
                return verified.map(|()| result);
            }
            // A tail index, or one kept for another page size: parse its
            // records from the unchanged file.
            unchanged = true;
        } else {
            match verify_prefix(&mut file, old.offset, old.fingerprint)? {
                Some(checked) => hash = checked,
                None => projection = None,
            }
        }
    }
    let mut projection = projection.unwrap_or_else(|| Projection {
        metadata: metadata.clone(),
        offset: 0,
        fingerprint: 0,
        matching: 0,
        kept: VecDeque::new(),
        values: Vec::new(),
    });
    let mut window = projection.take_window(limit);
    let partial = if unchanged {
        None
    } else {
        parse_new_lines(
            &mut file,
            metadata.len(),
            &mut projection,
            hash,
            &mut window,
            limit,
        )?
    };
    if !matches!(
        fill(&file, &mut window, projection.matching.min(limit)),
        Ok(true)
    ) {
        verify_unchanged(path, &file, &metadata, "transcript changed during read")?;
        // The file shows the version the index was taken from, yet no longer
        // holds history records where it says: read the whole file instead.
        return read_projection(path, limit, cached);
    }
    let verified = verify_unchanged(
        path,
        &file,
        &metadata,
        if unchanged {
            "transcript changed during cached read"
        } else {
            "transcript changed during read"
        },
    );
    projection.metadata = metadata;
    let mut bytes = 0_usize;
    let mut values = Vec::with_capacity(window.len());
    for (at, len, value) in window {
        bytes = bytes.saturating_add(len.saturating_mul(4));
        projection.kept.push_back((at, len));
        values.extend(value);
    }
    if let Err(error) = verified {
        // The fingerprint covers every byte of the prefix, and the metadata
        // is the version the read opened, which the file has since left: the
        // next read verifies that whole prefix again before resuming from it,
        // and parses the kept records from what it verified.
        if changed_during_read(&error) {
            *cached = Some(projection);
        }
        return Err(error);
    }
    if bytes > CACHE_BYTES_PER_FILE {
        // Too large to keep parsed: keep only where the records sit, and
        // hand the parsed records over instead of copying.
        let result = page(values.into_iter(), projection.matching, partial, limit);
        *cached = Some(projection);
        return Ok(result);
    }
    projection.values = values;
    let result = page_of(&projection, partial, limit);
    *cached = Some(projection);
    Ok(result)
}

/// Parses the complete lines from the projection's offset to `end` into the
/// window, extending the offset and the prefix hash over them. Returns a valid
/// partial last line, which is never committed.
fn parse_new_lines(
    file: &mut File,
    end: u64,
    projection: &mut Projection,
    mut hash: PrefixHash,
    window: &mut Window,
    limit: usize,
) -> io::Result<Option<Value>> {
    let mut partial = None;
    file.seek(SeekFrom::Start(projection.offset))?;
    let mut reader = BufReader::new(Read::take(&*file, end - projection.offset));
    let mut line = Vec::new();
    loop {
        let count = next_record(&mut reader, &mut line)?;
        if count == 0 {
            break;
        }
        let record = serde_json::from_slice::<Value>(&line)
            .ok()
            .filter(super::is_history_message);
        if line.last() != Some(&b'\n') {
            // A valid legacy last line is visible, but never committed to the
            // offset cache until its newline arrives. Partial JSON is retried.
            partial = record;
            break;
        }
        let at = projection.offset;
        projection.offset += count as u64;
        hash.write(&line);
        if let Some(record) = record {
            projection.matching += 1;
            window.push_back((at, count, Some(record)));
            if window.len() > limit {
                window.pop_front();
            }
        }
    }
    projection.fingerprint = hash.finish();
    Ok(partial)
}

/// Parses the kept records a tail index left in the file, then reads back
/// from the oldest of them, one line at a time, until `wanted` records are
/// kept: a larger page started from a smaller one's tail. Every byte read lies
/// in the prefix this read verified. `false` when the file no longer holds a
/// history record where the index says one is, or holds too few before it.
fn fill(file: &File, window: &mut Window, wanted: usize) -> io::Result<bool> {
    let mut line = Vec::new();
    for (at, len, value) in window.iter_mut() {
        if value.is_none() {
            *value = parse_at(file, *at, *len, &mut line)?;
            if value.is_none() {
                return Ok(false);
            }
        }
    }
    let mut end = window.front().map_or(0, |&(at, _, _)| at);
    let mut chunk = Vec::new();
    while window.len() < wanted {
        if end == 0 {
            return Ok(false);
        }
        let start = line_start(file, end - 1, &mut chunk)?;
        let len = usize::try_from(end - start).map_err(io::Error::other)?;
        if let Some(record) = parse_at(file, start, len, &mut line)? {
            window.push_front((start, len, Some(record)));
        }
        end = start;
    }
    Ok(true)
}

/// The record at `at`, parsed exactly as the forward read parses its line.
fn parse_at(file: &File, at: u64, len: usize, line: &mut Vec<u8>) -> io::Result<Option<Value>> {
    line.clear();
    line.resize(len, 0);
    read_exact_at(file, line, at)?;
    Ok(serde_json::from_slice::<Value>(line)
        .ok()
        .filter(super::is_history_message))
}

/// Where the line ending at the newline at `newline` starts: just past the
/// newline before it, or the start of the file.
fn line_start(file: &File, newline: u64, chunk: &mut Vec<u8>) -> io::Result<u64> {
    chunk.resize(64 * 1024, 0);
    let mut end = newline;
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let part = &mut chunk[..usize::try_from(end - start).unwrap()];
        read_exact_at(file, part, start)?;
        if let Some(found) = part.iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + found as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

fn page_of(projection: &Projection, partial: Option<Value>, limit: usize) -> Page {
    page(
        projection.values.iter().cloned(),
        projection.matching,
        partial,
        limit,
    )
}

fn page(
    kept: impl Iterator<Item = Value>,
    matching: usize,
    partial: Option<Value>,
    limit: usize,
) -> Page {
    let total = matching + usize::from(partial.is_some());
    let mut records: Vec<_> = kept.collect();
    if let Some(partial) = partial {
        records.push(partial);
    }
    if records.len() > limit {
        records.remove(0);
    }
    let older = total.saturating_sub(records.len());
    (records, older)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::OpenOptions, io::Write};

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_unchanged_and_growing_history() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("benchmark.jsonl");
        let line = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}\n",
            "x".repeat(1024)
        );
        std::fs::write(&path, line.repeat(10_000)).unwrap();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            std::hint::black_box(super::super::fold_history_jsonl(&path, 500).unwrap());
        }
        let fresh = start.elapsed();
        read(&path, 500).unwrap();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            std::hint::black_box(read(&path, 500).unwrap());
        }
        let warm = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..10 {
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(line.as_bytes())
                .unwrap();
            std::hint::black_box(read(&path, 500).unwrap());
        }
        let growing = start.elapsed();
        assert_eq!(
            read(&path, 500).unwrap(),
            super::super::fold_history_jsonl(&path, 500).unwrap()
        );
        eprintln!(
            "10 reads of 10,000 records: fresh {fresh:?}, unchanged {warm:?}, growing {growing:?}; input {} bytes",
            line.len() * 10_000
        );
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_unchanged_and_growing_history_over_the_cache_cap() {
        const RECORDS: usize = 20_000;
        const ROUNDS: u32 = 5;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("benchmark.jsonl");
        // About 8 KiB per record, shaped like a transcript line: envelope
        // fields, a content array, escaped tool output and usage numbers.
        let text = "fn main() {\\n    println!(\\\"line \\u00e9 {}\\\", 42);\\n}\\t// output\\n"
            .repeat(125);
        let line = |index: usize| {
            let kind = if index % 2 == 0 { "user" } else { "assistant" };
            format!(
                "{{\"parentUuid\":\"{prev:08}-0000-4000-8000-000000000000\",\"isSidechain\":false,\"userType\":\"external\",\"cwd\":\"/tmp/project\",\"sessionId\":\"benchmark\",\"version\":\"2.0.0\",\"type\":\"{kind}\",\"message\":{{\"role\":\"{kind}\",\"model\":\"claude\",\"content\":[{{\"type\":\"text\",\"text\":\"Step {index}\"}},{{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_{index}\",\"content\":\"{text}\",\"is_error\":false}}],\"usage\":{{\"input_tokens\":{index},\"output_tokens\":512,\"cache_read_input_tokens\":120000}}}},\"uuid\":\"{index:08}-0000-4000-8000-000000000000\",\"timestamp\":\"2026-11-14T12:00:00.000Z\"}}\n",
                prev = index.saturating_sub(1)
            )
        };
        let mut bytes = String::with_capacity(RECORDS * 8_300);
        for index in 0..RECORDS {
            bytes.push_str(&line(index));
        }
        std::fs::write(&path, &bytes).unwrap();
        drop(bytes);
        let input = std::fs::metadata(&path).unwrap().len();
        let start = std::time::Instant::now();
        std::hint::black_box(super::super::fold_history_jsonl(&path, 500).unwrap());
        let fresh = start.elapsed();
        let start = std::time::Instant::now();
        read(&path, 500).unwrap();
        let first = start.elapsed();
        let start = std::time::Instant::now();
        for _ in 0..ROUNDS {
            std::hint::black_box(read(&path, 500).unwrap());
        }
        let unchanged = start.elapsed() / ROUNDS;
        let start = std::time::Instant::now();
        for index in 0..ROUNDS as usize {
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(line(RECORDS + index).as_bytes())
                .unwrap();
            std::hint::black_box(read(&path, 500).unwrap());
        }
        let appended = start.elapsed() / ROUNDS;
        let start = std::time::Instant::now();
        std::hint::black_box(read(&path, 1_000).unwrap());
        let older = start.elapsed();
        assert_eq!(
            read(&path, 500).unwrap(),
            super::super::fold_history_jsonl(&path, 500).unwrap()
        );
        assert_eq!(
            read(&path, 1_000).unwrap(),
            super::super::fold_history_jsonl(&path, 1_000).unwrap()
        );
        eprintln!(
            "{RECORDS} records, {input} bytes: full fold {fresh:?}, first read {first:?}, unchanged re-read {unchanged:?}, appended re-read {appended:?} (each the mean of {ROUNDS}), first larger page {older:?}"
        );
    }

    #[test]
    fn failed_reads_leave_no_slot_and_uncacheable_reads_keep_only_a_tail_index() {
        let root = tempfile::tempdir().unwrap();
        let large = root.path().join("large.jsonl");
        let line = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}\n",
            "x".repeat(4096)
        );
        // About 2.5 MiB of records: past the 8 MiB projection estimate.
        std::fs::write(&large, line.repeat(600)).unwrap();
        assert_eq!(
            read(&large, 1_000).unwrap(),
            super::super::fold_history_jsonl(&large, 1_000).unwrap()
        );
        let mut cached = None;
        read_projection(&large, 1_000, &mut cached).unwrap();
        assert!(cached.is_some_and(|index| index.values.is_empty() && index.kept.len() == 600));
        let missing = root.path().join("missing.jsonl");
        assert!(read(&missing, 1_000).is_err());
        let cache = lock(&CACHE);
        assert!(!cache.iter().any(|(name, _, _)| name == &missing));
        drop(cache);

        let warm = |index: usize| {
            (
                PathBuf::from(format!("/warm/{index}")),
                1,
                Arc::new(Mutex::new(None::<Projection>)),
            )
        };
        let mut lru: Lru = (0..CACHE_FILES).map(warm).collect();
        let cold = warm(CACHE_FILES);
        let slot = Arc::clone(&cold.2);
        lru.push_back(cold);
        settle(&mut lru, &slot, false);
        assert_eq!(lru.len(), CACHE_FILES);
        assert_eq!(lru[0].0, PathBuf::from("/warm/0"));
        assert!(!lru.iter().any(|(_, _, other)| Arc::ptr_eq(other, &slot)));
    }

    #[test]
    fn cached_reads_match_fresh_reads_across_append_partial_rewrite_and_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"text\":\"one\"}\n").unwrap();
        let check = || {
            assert_eq!(
                read(&path, 2).unwrap(),
                super::super::fold_history_jsonl(&path, 2).unwrap()
            );
        };
        check();
        check();
        for tail in [
            "{\"type\":\"assistant\",",
            "\"text\":\"two\"}",
            "\n",
            "{\"type\":\"user\",\"text\":\"three\"}\n",
        ] {
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(tail.as_bytes())
                .unwrap();
            check();
        }
        std::fs::write(&path, "{\"type\":\"user\",\"text\":\"rewritten and longer than the cached prefix, which cannot be trusted as append\"}\n").unwrap();
        check();
        std::fs::write(root.path().join("replacement"), "{\"type\":\"user\"}\n").unwrap();
        std::fs::rename(root.path().join("replacement"), &path).unwrap();
        check();
        std::fs::write(&path, "").unwrap();
        check();
    }

    fn append_record(path: &Path) {
        OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"{\"type\":\"assistant\",\"text\":\"more\"}\n")
            .unwrap();
    }

    #[test]
    fn an_append_during_the_read_keeps_a_projection_the_next_read_resumes_from() {
        use super::super::cache_file::BEFORE_VERIFY;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.jsonl");
        std::fs::write(&path, "{\"type\":\"user\",\"text\":\"one\"}\n").unwrap();
        let mut cached = None;
        read_projection(&path, 2, &mut cached).unwrap();
        append_record(&path);
        // Claude appends again before every check: the read fails as changed,
        // and the prefix it verified stays for the next read.
        let moving = path.clone();
        BEFORE_VERIFY.set(Some(Box::new(move || append_record(&moving))));
        let error = read_projection(&path, 2, &mut cached).unwrap_err();
        BEFORE_VERIFY.set(None);
        assert!(changed_during_read(&error));
        assert!(cached.as_ref().is_some_and(|kept| kept.offset > 0));
        assert_eq!(
            read_projection(&path, 2, &mut cached).unwrap(),
            super::super::fold_history_jsonl(&path, 2).unwrap()
        );

        // A single collision is read again rather than reported.
        let mut left = 1;
        let moving = path.clone();
        append_record(&path);
        BEFORE_VERIFY.set(Some(Box::new(move || {
            if left > 0 {
                left -= 1;
                append_record(&moving);
            }
        })));
        let page = read(&path, 2);
        BEFORE_VERIFY.set(None);
        assert_eq!(
            page.unwrap(),
            super::super::fold_history_jsonl(&path, 2).unwrap()
        );

        // Any other failure still drops the projection.
        std::fs::remove_file(&path).unwrap();
        assert!(read_projection(&path, 2, &mut cached).is_err());
        assert!(cached.is_none());
    }

    /// A history record of its own over the cache cap, at four times its bytes.
    fn large_record(kind: &str, text: &str) -> String {
        format!(
            "{{\"type\":\"{kind}\",\"text\":\"{text}\",\"pad\":\"{}\"}}\n",
            "x".repeat(CACHE_BYTES_PER_FILE / 4)
        )
    }

    /// Large history records between lines that are not history.
    fn large_transcript(records: usize) -> String {
        let mut transcript = String::new();
        for index in 0..records {
            let kind = if index % 2 == 0 { "user" } else { "assistant" };
            transcript.push_str(&large_record(kind, &index.to_string()));
            transcript.push_str("{\"type\":\"summary\",\"summary\":\"between\"}\n");
            transcript.push_str("{\"type\":\"user\",\"isSidechain\":true}\n");
            transcript.push_str("not json\n");
        }
        transcript
    }

    #[test]
    fn tail_index_reads_match_fresh_reads_over_the_cache_cap() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.jsonl");
        std::fs::write(&path, large_transcript(5)).unwrap();
        let mut cached = None;
        let mut check = || {
            let fresh = super::super::fold_history_jsonl(&path, 2).unwrap();
            assert_eq!(read_projection(&path, 2, &mut cached).unwrap(), fresh);
            let index = cached.as_ref().unwrap();
            assert!(index.values.is_empty(), "only a tail index is kept");
            for limit in [1, 2, 3, 7] {
                assert_eq!(
                    read(&path, limit).unwrap(),
                    super::super::fold_history_jsonl(&path, limit).unwrap(),
                    "limit {limit}"
                );
            }
        };
        // Unchanged.
        check();
        check();
        // Appended: a record that is not history, then one that is.
        let append = |tail: &str| {
            OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all(tail.as_bytes())
                .unwrap();
        };
        append("{\"type\":\"system\"}\n");
        check();
        append(&large_record("assistant", "appended"));
        check();
        // A partial last line, visible once valid, then completed.
        let record = large_record("user", "partial");
        let (head, tail) = record.split_at(record.len() / 2);
        append(head);
        check();
        append(tail.trim_end());
        check();
        append("\n");
        check();
        append("{\"type\":\"user\",\"text\":\"small\"}\n");
        check();
        // Rewritten in place with the same length: the prefix check fails.
        let mut rewritten = std::fs::read(&path).unwrap();
        let text = rewritten
            .windows(10)
            .position(|bytes| bytes == b"\"text\":\"0\"")
            .unwrap();
        rewritten[text + 8] = b'9';
        std::fs::write(&path, &rewritten).unwrap();
        check();
        // Truncated.
        std::fs::write(&path, large_transcript(3)).unwrap();
        check();
        std::fs::write(&path, "").unwrap();
        check();
        // Replaced by another file.
        std::fs::write(root.path().join("replacement"), large_transcript(4)).unwrap();
        std::fs::rename(root.path().join("replacement"), &path).unwrap();
        check();
    }

    #[test]
    fn a_larger_page_extends_a_smaller_pages_tail_index_backwards() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("history.jsonl");
        std::fs::write(&path, large_transcript(6)).unwrap();
        let mut small = None;
        read_projection(&path, 2, &mut small).unwrap();
        let small = small.unwrap();
        let mut full = None;
        read_projection(&path, 4, &mut full).unwrap();
        let full = full.unwrap();

        // The two earlier records are found by reading back from the tail.
        let file = File::open(&path).unwrap();
        let mut window: Window = small
            .kept
            .iter()
            .map(|&(at, len)| (at, len, None))
            .collect();
        assert!(fill(&file, &mut window, 4).unwrap());
        let found: Vec<_> = window.iter().map(|&(at, len, _)| (at, len)).collect();
        assert_eq!(found, Vec::from(full.kept.clone()));

        // Started from the smaller tail, a larger page reads as a fresh one,
        // unchanged and after an append; so does a smaller page.
        for limit in [4, 5, 6, 9, 1] {
            let mut seeded = Some(small.index());
            assert_eq!(
                read_projection(&path, limit, &mut seeded).unwrap(),
                super::super::fold_history_jsonl(&path, limit).unwrap(),
                "limit {limit}"
            );
        }
        let mut seeded = Some(small.index());
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(large_record("assistant", "appended").as_bytes())
            .unwrap();
        assert_eq!(
            read_projection(&path, 5, &mut seeded).unwrap(),
            super::super::fold_history_jsonl(&path, 5).unwrap()
        );

        // An index that no longer points at history records falls back to
        // reading the whole file.
        let mut stale = None;
        read_projection(&path, 4, &mut stale).unwrap();
        let stale = stale.unwrap();
        let first = std::fs::read(&path)
            .unwrap()
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap()
            + 1;
        let mut summary = stale.index();
        summary.kept[0] = (
            first as u64,
            "{\"type\":\"summary\",\"summary\":\"between\"}\n".len(),
        );
        let mut longer = stale.index();
        longer.kept[0].1 += 1;
        for index in [summary, longer] {
            let mut index = Some(index);
            assert_eq!(
                read_projection(&path, 4, &mut index).unwrap(),
                super::super::fold_history_jsonl(&path, 4).unwrap()
            );
        }
    }
}
