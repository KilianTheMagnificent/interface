//! Read-only transcript cache verification shared by listing and history.
//! Identity/version checks never substitute for checking the complete old prefix
//! on growth. The hash accelerates display reads and grants no effect authority.
use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    fs::{File, Metadata, OpenOptions},
    hash::Hasher,
    io::{self, BufRead, Read, Seek},
    path::Path,
};

/// One transcript record may be this large; a longer line fails the read.
const MAX_RECORD_BYTES: u64 = 192 * 1024 * 1024;

/// A prefix hashes in blocks this large at fixed file offsets, so its hash
/// never depends on how the bytes were fed and a long prefix verifies on
/// several threads.
const PREFIX_BLOCK: u64 = 1024 * 1024;

/// The most threads one prefix verification reads on.
const PREFIX_READERS: usize = 4;

/// The hash of a transcript prefix: every complete [`PREFIX_BLOCK`] hashed on
/// its own, then those digests and the trailing partial block in order. It is
/// as strong a check of every byte as one hash over the whole prefix.
#[derive(Clone)]
pub(super) struct PrefixHash {
    blocks: DefaultHasher,
    block: DefaultHasher,
    len: u64,
}

impl PrefixHash {
    pub(super) fn new() -> Self {
        Self {
            blocks: DefaultHasher::new(),
            block: DefaultHasher::new(),
            len: 0,
        }
    }

    pub(super) fn write(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let room = PREFIX_BLOCK - self.len % PREFIX_BLOCK;
            let (head, rest) = bytes
                .split_at(usize::try_from(room).map_or(bytes.len(), |room| room.min(bytes.len())));
            self.block.write(head);
            self.len += head.len() as u64;
            if self.len % PREFIX_BLOCK == 0 {
                self.blocks
                    .write_u64(std::mem::take(&mut self.block).finish());
            }
            bytes = rest;
        }
    }

    pub(super) fn finish(&self) -> u64 {
        let mut total = self.blocks.clone();
        total.write_u64(self.block.finish());
        total.write_u64(self.len);
        total.finish()
    }
}

/// A transcript that moved on while it was read. Claude appends to a live
/// transcript every few seconds, so a fresh attempt usually succeeds.
#[derive(Debug)]
struct Changed(&'static str);

impl fmt::Display for Changed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for Changed {}

/// A read that loses that race is tried again at most this many times in all,
/// resuming from the verified prefix it kept.
pub(super) const READ_ATTEMPTS: usize = 3;

/// Whether `error` reports a transcript that changed while it was read, as
/// opposed to one that is unsafe or unreadable.
pub(super) fn changed_during_read(error: &io::Error) -> bool {
    matches!(error.get_ref(), Some(inner) if inner.is::<Changed>())
}

#[cfg(test)]
thread_local! {
    /// Runs before each unchanged check, so a test can append mid-read.
    pub(super) static BEFORE_VERIFY: std::cell::RefCell<Option<Box<dyn FnMut()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Open the transcript without following a final link or blocking on a FIFO,
/// and require it to still be the regular file `named` described.
pub(super) fn open_checked(path: &Path, named: &Metadata) -> io::Result<(File, Metadata)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("transcript identity changed"));
    }
    if !same_version(named, &metadata) {
        return Err(io::Error::other(Changed("transcript identity changed")));
    }
    Ok((file, metadata))
}

/// The hash of the file's first `offset` bytes when it still matches
/// `fingerprint`, so parsing can resume there; `None` when the prefix changed.
pub(super) fn verify_prefix(
    file: &mut File,
    offset: u64,
    fingerprint: u64,
) -> io::Result<Option<PrefixHash>> {
    let checked = prefix(file, offset)?;
    Ok((checked.finish() == fingerprint).then_some(checked))
}

/// Read the next record into `line` (cleared first), returning its length;
/// 0 at the end. A record over the cap fails the read.
pub(super) fn next_record(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<usize> {
    line.clear();
    let count = reader
        .by_ref()
        .take(MAX_RECORD_BYTES + 1)
        .read_until(b'\n', line)?;
    if count as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::other("transcript record too large"));
    }
    Ok(count)
}

/// Fail with `message` unless both the open file and the path still show the
/// version the read started from.
pub(super) fn verify_unchanged(
    path: &Path,
    file: &File,
    metadata: &Metadata,
    message: &'static str,
) -> io::Result<()> {
    #[cfg(test)]
    BEFORE_VERIFY.with_borrow_mut(|hook| {
        if let Some(hook) = hook {
            hook();
        }
    });
    if !same_version(metadata, &file.metadata()?)
        || !same_version(metadata, &super::safe_file_metadata(path)?)
    {
        return Err(io::Error::other(Changed(message)));
    }
    Ok(())
}

pub(super) fn same_version(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left.ctime() != right.ctime()
            || left.ctime_nsec() != right.ctime_nsec()
            || left.mode() != right.mode()
            || left.nlink() != right.nlink()
        {
            return false;
        }
    }
    super::same_file_snapshot(left, right)
}

pub(super) fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        false
    }
}

/// Hashes the file's first `bytes` bytes, its complete blocks on up to
/// [`PREFIX_READERS`] threads.
fn prefix(file: &mut File, bytes: u64) -> io::Result<PrefixHash> {
    let blocks = bytes / PREFIX_BLOCK;
    let mut hash = PrefixHash::new();
    for digest in block_digests(file, blocks)? {
        hash.blocks.write_u64(digest);
    }
    hash.len = blocks * PREFIX_BLOCK;
    file.seek(io::SeekFrom::Start(hash.len))?;
    let mut remaining = bytes - hash.len;
    let mut chunk = [0; 16 * 1024];
    while remaining > 0 {
        let count = usize::try_from(remaining.min(chunk.len() as u64)).unwrap();
        file.read_exact(&mut chunk[..count])?;
        hash.write(&chunk[..count]);
        remaining -= count as u64;
    }
    Ok(hash)
}

/// The digests of the file's first `blocks` complete blocks, in order.
fn block_digests(file: &File, blocks: u64) -> io::Result<Vec<u64>> {
    let readers = if cfg!(unix) {
        std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(PREFIX_READERS)
            .min(usize::try_from(blocks).unwrap_or(usize::MAX))
    } else {
        1
    };
    if readers < 2 {
        return (0..blocks).map(|index| block_digest(file, index)).collect();
    }
    let per_reader = blocks.div_ceil(readers as u64);
    std::thread::scope(|scope| {
        let workers = (0..readers as u64)
            .map(|reader| {
                let first = reader * per_reader;
                let last = (first + per_reader).min(blocks);
                scope.spawn(move || {
                    (first..last)
                        .map(|index| block_digest(file, index))
                        .collect::<io::Result<Vec<_>>>()
                })
            })
            .collect::<Vec<_>>();
        let mut digests = Vec::with_capacity(usize::try_from(blocks).unwrap_or(0));
        for worker in workers {
            digests.extend(
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))?,
            );
        }
        Ok(digests)
    })
}

fn block_digest(file: &File, index: u64) -> io::Result<u64> {
    let mut hash = DefaultHasher::new();
    let mut chunk = vec![0; 64 * 1024];
    let mut offset = index * PREFIX_BLOCK;
    let end = offset + PREFIX_BLOCK;
    while offset < end {
        let count = usize::try_from((end - offset).min(chunk.len() as u64)).unwrap();
        read_exact_at(file, &mut chunk[..count], offset)?;
        hash.write(&chunk[..count]);
        offset += count as u64;
    }
    Ok(hash.finish())
}

/// A positional read: readers on several threads share one descriptor
/// without sharing its file offset.
#[cfg(unix)]
pub(super) fn read_exact_at(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buffer, offset)
}

/// Without positional reads only one reader runs, so it may move the offset.
#[cfg(not(unix))]
pub(super) fn read_exact_at(mut file: &File, buffer: &mut [u8], offset: u64) -> io::Result<()> {
    file.seek(io::SeekFrom::Start(offset))?;
    file.read_exact(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn complete_records_extend_the_verified_prefix_across_read_chunks() {
        let mut file = tempfile::tempfile().unwrap();
        let records = [b"header\n".to_vec(), vec![b'x'; 33_001], b"tail\n".to_vec()];
        let mut hash = PrefixHash::new();
        let mut length = 0;
        for record in records {
            file.seek(io::SeekFrom::End(0)).unwrap();
            file.write_all(&record).unwrap();
            length += record.len() as u64;
            hash.write(&record);
            let verified = prefix(&mut file, length).unwrap();
            assert_eq!(hash.finish(), verified.finish());
            hash = verified;
        }
        assert!(prefix(&mut file, length + 1).is_err());
    }

    #[test]
    fn block_hashes_check_every_byte_however_the_prefix_was_fed_or_split() {
        let block = usize::try_from(PREFIX_BLOCK).unwrap();
        // Records of uneven sizes over several blocks, so records straddle
        // block boundaries and verification runs on several readers.
        let mut bytes = Vec::new();
        let mut size = 1;
        while bytes.len() < 5 * block + 123 {
            bytes.extend(std::iter::repeat_n(
                b'a' + u8::try_from(size % 26).unwrap(),
                size,
            ));
            bytes.push(b'\n');
            size = size * 7 % 99_991 + 1;
        }
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&bytes).unwrap();
        let fed = |end: usize, step: usize| {
            let mut hash = PrefixHash::new();
            for piece in bytes[..end].chunks(step) {
                hash.write(piece);
            }
            hash
        };
        let mut records = PrefixHash::new();
        for record in bytes.split_inclusive(|byte| *byte == b'\n') {
            records.write(record);
        }
        assert_eq!(records.finish(), fed(bytes.len(), bytes.len()).finish());
        for end in [
            0,
            1,
            block - 1,
            block,
            block + 1,
            3 * block,
            3 * block + 77,
            bytes.len(),
        ] {
            let verified = prefix(&mut file, end as u64).unwrap();
            assert_eq!(
                verified.finish(),
                fed(end, 4093).finish(),
                "prefix of {end}"
            );
            assert_eq!(
                verified.finish(),
                fed(end, block).finish(),
                "prefix of {end}"
            );
            // A verified prefix resumes exactly where a fresh hash would be.
            let mut resumed = verified.clone();
            resumed.write(&bytes[end..]);
            assert_eq!(resumed.finish(), records.finish(), "resumed from {end}");
        }

        // One changed byte in a middle block fails the complete prefix.
        let mut changed = bytes.clone();
        changed[2 * block + 5] ^= 1;
        file.seek(io::SeekFrom::Start(0)).unwrap();
        file.write_all(&changed).unwrap();
        let length = bytes.len() as u64;
        assert!(
            verify_prefix(&mut file, length, records.finish())
                .unwrap()
                .is_none()
        );
        file.seek(io::SeekFrom::Start(0)).unwrap();
        file.write_all(&bytes).unwrap();
        assert!(
            verify_prefix(&mut file, length, records.finish())
                .unwrap()
                .is_some()
        );
    }
}
