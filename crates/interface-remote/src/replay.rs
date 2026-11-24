//! A bounded observer of output. It never holds up the authoritative PTY reader.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub type StreamKey = (Uuid, u64);
const STREAM_BYTES: usize = 256 * 1024;
const MAX_STREAMS: usize = 128;
const MAX_FRAMES: usize = 512;

/// The pane's own bounded output tail: its absolute output byte count and the
/// newest bytes before that count. It seeds a stream whose output was counted
/// but not kept while phone access was off.
pub type ReplaySeed = Arc<dyn Fn() -> Option<(u64, Vec<u8>)> + Send + Sync>;

#[derive(Default)]
struct Stream {
    next: u64,
    consumed_cursor: Option<u64>,
    bytes: usize,
    frames: VecDeque<(u64, Vec<u8>)>,
    /// Output was counted without being kept, so the next phone read first
    /// fills in what the pane's tail still holds.
    seed_due: bool,
}

impl Stream {
    /// Fills in kept output from a tail covering `[end - tail.len(), end)`.
    /// The pump records the tail before this stream, so the tail may hold a
    /// batch not yet counted here; those bytes arrive by [`Replay::append`].
    fn seed(&mut self, end: u64, tail: &[u8]) {
        let Some(ahead) = end
            .checked_sub(self.next)
            .and_then(|ahead| usize::try_from(ahead).ok())
        else {
            return;
        };
        let keep = tail.len().saturating_sub(ahead);
        let start = self.next - keep as u64;
        let oldest = self.frames.front().map_or(self.next, |(offset, _)| *offset);
        if start < oldest {
            self.frames.clear();
            self.frames.push_back((start, tail[..keep].to_vec()));
            self.bytes = keep;
        }
    }
}

/// Output streams by terminal generation. A generation's stream lives while
/// its output is still being read ([`Replay::register`] until
/// [`Replay::finish`]) or while it is its terminal's visible generation
/// ([`Replay::show`] until [`Replay::hide`] or a newer `show`), so exited
/// panes stay replayable and dismissed or superseded ones release their slot.
///
/// Every stream counts its output, because approvals compare a screen's byte
/// cursor with that count. Only while output is
/// retained (phone access is on) are the bytes themselves kept.
pub struct Replay {
    table: Mutex<Table>,
}

impl Default for Replay {
    /// A replay that retains output.
    fn default() -> Self {
        Self {
            table: Mutex::new(Table {
                retaining: true,
                ..Table::default()
            }),
        }
    }
}

#[derive(Default)]
struct Table {
    streams: BTreeMap<StreamKey, Stream>,
    reading: BTreeSet<StreamKey>,
    visible: BTreeMap<Uuid, u64>,
    seeds: BTreeMap<StreamKey, ReplaySeed>,
    retaining: bool,
}

impl Table {
    fn retire_unless_live(&mut self, key: StreamKey) {
        if !self.reading.contains(&key) && self.visible.get(&key.0) != Some(&key.1) {
            self.streams.remove(&key);
            self.seeds.remove(&key);
        }
    }

    fn retire_terminal_except(&mut self, id: Uuid, kept: Option<u64>) {
        let Self {
            streams,
            reading,
            seeds,
            ..
        } = self;
        streams.retain(|key, _| key.0 != id || Some(key.1) == kept || reading.contains(key));
        seeds.retain(|key, _| streams.contains_key(key));
    }
}

pub struct ReplayPage {
    pub start: u64,
    pub end: u64,
    pub gap: bool,
    pub bytes: Vec<u8>,
}

impl ReplayPage {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = b"IRP1".to_vec();
        bytes.extend_from_slice(&self.start.to_le_bytes());
        bytes.extend_from_slice(&self.end.to_le_bytes());
        bytes.push(u8::from(self.gap));
        bytes.extend_from_slice(&self.bytes);
        bytes
    }
}

impl Replay {
    /// A replay that counts output but keeps none until
    /// [`Self::set_retaining`]: phone access starts off.
    #[must_use]
    pub fn without_retention() -> Self {
        Self {
            table: Mutex::default(),
        }
    }

    /// Phone access turned on or off. Turning it off releases every kept
    /// byte; each stream then keeps counting, and its first phone read after
    /// access returns is seeded from the pane's tail.
    pub fn set_retaining(&self, retaining: bool) {
        if let Ok(mut table) = self.table.lock()
            && std::mem::replace(&mut table.retaining, retaining)
            && !retaining
        {
            for stream in table.streams.values_mut() {
                stream.frames.clear();
                stream.bytes = 0;
                stream.seed_due |= stream.next > 0;
            }
        }
    }

    /// Any human input invalidates approval observations until new output arrives.
    pub fn invalidate_approval(&self, key: StreamKey) {
        if let Ok(mut table) = self.table.lock() {
            if let Some(stream) = table.streams.get_mut(&key) {
                stream.consumed_cursor = Some(stream.next);
            }
        }
    }

    pub fn approval_current(&self, key: StreamKey, cursor: u64) -> bool {
        self.table.lock().ok().is_some_and(|table| {
            table
                .streams
                .get(&key)
                .is_some_and(|s| s.next == cursor && s.consumed_cursor != Some(cursor))
        })
    }

    /// Serializes one response with output observation. A consumed screen cannot
    /// approve a second request while waiting for the CLI to paint its next frame.
    /// # Errors
    /// Refuses stale or consumed cursors and propagates input admission errors.
    pub fn respond_current(
        &self,
        key: StreamKey,
        cursor: u64,
        enqueue: impl FnOnce() -> Result<(), crate::RemoteError>,
    ) -> Result<(), crate::RemoteError> {
        let mut table = self.table.lock().map_err(|_| stale_approval())?;
        let stream = table.streams.get_mut(&key).ok_or_else(stale_approval)?;
        if stream.next != cursor || stream.consumed_cursor == Some(cursor) {
            return Err(stale_approval());
        }
        stream.consumed_cursor = Some(cursor);
        enqueue()
    }

    /// Starts retaining a generation whose output is about to be read. It is
    /// retained at least until [`Self::finish`] for the same key.
    pub fn register(&self, key: StreamKey) {
        self.register_with(key, None);
    }

    /// [`Self::register`], naming the tail that seeds the stream when output
    /// was counted without being kept.
    pub fn register_seeded(&self, key: StreamKey, seed: ReplaySeed) {
        self.register_with(key, Some(seed));
    }

    fn register_with(&self, key: StreamKey, seed: Option<ReplaySeed>) {
        if let Ok(mut table) = self.table.lock() {
            table.reading.insert(key);
            if table.streams.len() < MAX_STREAMS {
                table.streams.entry(key).or_default();
            }
            if let Some(seed) = seed
                && table.streams.contains_key(&key)
            {
                table.seeds.insert(key, seed);
            }
        }
    }

    /// The generation's output reader has stopped. Its stream is released
    /// unless the generation is still visible.
    pub fn finish(&self, key: StreamKey) {
        if let Ok(mut table) = self.table.lock() {
            table.reading.remove(&key);
            table.retire_unless_live(key);
        }
    }

    /// `key` is now its terminal's visible generation. Older generations of
    /// that terminal are released once their output has been read.
    pub fn show(&self, key: StreamKey) {
        if let Ok(mut table) = self.table.lock()
            && table.visible.insert(key.0, key.1) != Some(key.1)
        {
            table.retire_terminal_except(key.0, Some(key.1));
        }
    }

    /// The terminal is no longer visible. Its streams are released once their
    /// output has been read.
    pub fn hide(&self, id: Uuid) {
        if let Ok(mut table) = self.table.lock() {
            table.visible.remove(&id);
            table.retire_terminal_except(id, None);
        }
    }

    pub fn append(&self, key: StreamKey, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let Ok(mut table) = self.table.lock() else {
            return;
        };
        let retaining = table.retaining;
        let streams = &mut table.streams;
        // Admission is bounded; adding a pane never evicts another live pane.
        if !streams.contains_key(&key) && streams.len() >= MAX_STREAMS {
            return;
        }
        let stream = streams.entry(key).or_default();
        let Some(next) = stream.next.checked_add(bytes.len() as u64) else {
            return;
        };
        if !retaining {
            stream.next = next;
            stream.seed_due = true;
            return;
        }
        let retained = &bytes[bytes.len().saturating_sub(STREAM_BYTES)..];
        stream
            .frames
            .push_back((next - retained.len() as u64, retained.to_vec()));
        stream.bytes += retained.len();
        stream.next = next;
        while stream.bytes > STREAM_BYTES || stream.frames.len() > MAX_FRAMES {
            if let Some((_, removed)) = stream.frames.pop_front() {
                stream.bytes -= removed.len();
            }
        }
    }

    pub fn read(&self, key: StreamKey, cursor: u64) -> Option<ReplayPage> {
        let mut table = self.table.lock().ok()?;
        let Table {
            streams,
            seeds,
            retaining,
            ..
        } = &mut *table;
        let stream = streams.get_mut(&key)?;
        if *retaining && stream.seed_due {
            stream.seed_due = false;
            if let Some((end, tail)) = seeds.get(&key).and_then(|seed| seed()) {
                stream.seed(end, &tail);
            }
        }
        let oldest = stream
            .frames
            .front()
            .map_or(stream.next, |(offset, _)| *offset);
        let gap = cursor < oldest || cursor > stream.next;
        let start = if gap { oldest } else { cursor };
        let count = usize::try_from(stream.next - start).ok()?;
        let mut bytes = Vec::with_capacity(count);
        // Frames are contiguous and ordered. Caught-up readers do no frame
        // work; other readers binary-search the first relevant frame.
        let first = if count == 0 {
            stream.frames.len()
        } else {
            stream
                .frames
                .partition_point(|(offset, frame)| *offset + frame.len() as u64 <= start)
        };
        for (offset, frame) in stream.frames.range(first..) {
            let skipped = usize::try_from(start.saturating_sub(*offset)).unwrap_or(usize::MAX);
            if skipped < frame.len() {
                bytes.extend_from_slice(&frame[skipped..]);
            }
        }
        Some(ReplayPage {
            start,
            end: stream.next,
            gap,
            bytes,
        })
    }
}

fn stale_approval() -> crate::RemoteError {
    crate::RemoteError::new(
        "approval-changed",
        "This request has changed. Review the latest request before responding.",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_pages_match_the_retained_bytes_at_every_boundary() {
        let replay = Replay::default();
        let key = (Uuid::new_v4(), 1);
        replay.register(key);
        assert!(replay.read(key, 0).unwrap().bytes.is_empty());
        let bytes: Vec<_> = (0_u8..100).collect();
        for chunk in bytes.chunks(7) {
            replay.append(key, chunk);
        }
        for cursor in 0..=bytes.len() {
            let page = replay.read(key, cursor as u64).unwrap();
            assert_eq!(page.bytes, bytes[cursor..]);
            assert_eq!(page.start, cursor as u64);
            assert_eq!(page.end, bytes.len() as u64);
            assert!(!page.gap);
        }
        let future = replay.read(key, 101).unwrap();
        assert!(future.gap);
        assert_eq!(future.bytes, bytes);
        for _ in 0..MAX_FRAMES {
            replay.append(key, b"z");
        }
        let retained = replay.read(key, 0).unwrap();
        assert!(retained.gap);
        assert_eq!(retained.bytes, vec![b'z'; MAX_FRAMES]);
        assert_eq!(
            replay.read(key, retained.start + 3).unwrap().bytes,
            vec![b'z'; MAX_FRAMES - 3]
        );
    }
    #[test]
    fn approvals_are_invalidated_by_output_input_and_an_already_consumed_screen() {
        let replay = Replay::default();
        let key = (Uuid::new_v4(), 1);
        replay.append(key, b"request");
        assert!(replay.approval_current(key, 7));
        replay.invalidate_approval(key);
        assert!(!replay.approval_current(key, 7));
        replay.append(key, b"new");
        assert!(replay.respond_current(key, 7, || panic!("stale")).is_err());
        assert!(replay.respond_current(key, 10, || Ok(())).is_ok());
        assert!(
            replay
                .respond_current(key, 10, || panic!("repeat"))
                .is_err()
        );
        assert!(!replay.approval_current(key, 10));
    }

    #[test]
    fn output_counted_while_access_was_off_is_seeded_from_the_pane_tail() {
        let replay = Replay::without_retention();
        let key = (Uuid::new_v4(), 1);
        // The pane's whole output; its tail is the last 8 bytes.
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let tail = Arc::clone(&output);
        replay.register_seeded(
            key,
            Arc::new(move || {
                let all = tail.lock().unwrap();
                let kept = all[all.len().saturating_sub(8)..].to_vec();
                Some((all.len() as u64, kept))
            }),
        );
        let produce = |bytes: &[u8]| {
            output.lock().unwrap().extend_from_slice(bytes);
            replay.append(key, bytes);
        };

        // Counted for approvals, kept for no one.
        produce(b"0123456789");
        assert!(replay.approval_current(key, 10));
        let page = replay.read(key, 0).unwrap();
        assert!(page.gap && page.bytes.is_empty());
        assert_eq!(page.end, 10);

        // The first read once access is on fills in what the tail holds, at
        // the same absolute cursors.
        replay.set_retaining(true);
        produce(b"abc");
        let page = replay.read(key, 0).unwrap();
        assert_eq!((page.start, page.end, page.gap), (5, 13, true));
        assert_eq!(page.bytes, b"56789abc");
        let page = replay.read(key, 7).unwrap();
        assert_eq!((page.bytes.as_slice(), page.gap), (&b"789abc"[..], false));

        // Turning access off releases what was kept. A tail a batch ahead of
        // the stream seeds only the bytes the stream has counted.
        replay.set_retaining(false);
        assert!(replay.read(key, 7).unwrap().bytes.is_empty());
        output.lock().unwrap().extend_from_slice(b"XY");
        replay.set_retaining(true);
        let page = replay.read(key, 0).unwrap();
        assert_eq!((page.start, page.end), (7, 13));
        assert_eq!(page.bytes, b"789abc");
        replay.append(key, b"XY");
        assert_eq!(replay.read(key, 7).unwrap().bytes, b"789abcXY");
    }

    #[test]
    fn output_matches_the_typescript_binary_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-access-v1.json"
        ))
        .unwrap();
        let page = ReplayPage {
            start: 5,
            end: 8,
            gap: false,
            bytes: b"hi\n".to_vec(),
        };
        let hex = fixture["output"]["hex"].as_str().unwrap();
        let expected: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
            .collect();
        assert_eq!(page.encode(), expected);
    }

    #[test]
    fn a_stream_lives_while_its_output_is_read_or_its_generation_is_visible() {
        let replay = Replay::default();
        let id = Uuid::new_v4();
        let (first, second) = ((id, 1), (id, 2));
        replay.register(first);
        replay.append(first, b"first");
        replay.show(first);
        replay.finish(first);
        assert!(
            replay.read(first, 0).is_some(),
            "an exited pane stays replayable while it is visible"
        );

        // A restart reads its new generation before that generation is shown.
        replay.register(second);
        replay.append(second, b"second");
        assert!(replay.read(first, 0).is_some());
        replay.show(second);
        assert!(replay.read(first, 0).is_none(), "the superseded generation");
        assert!(replay.read(second, 0).is_some());

        // Dismissal keeps a generation whose output is still being read.
        replay.hide(id);
        assert!(replay.read(second, 0).is_some());
        replay.finish(second);
        assert!(replay.read(second, 0).is_none());

        // A superseded generation still being read outlives the newer show.
        let other = Uuid::new_v4();
        replay.register((other, 1));
        replay.show((other, 1));
        replay.register((other, 2));
        replay.show((other, 2));
        assert!(replay.read((other, 1), 0).is_some());
        replay.finish((other, 1));
        assert!(replay.read((other, 1), 0).is_none());
        assert!(replay.read((other, 2), 0).is_some());

        // A generation that never became visible goes when its reader stops.
        let hidden = (Uuid::new_v4(), 1);
        replay.register(hidden);
        replay.append(hidden, b"never shown");
        replay.finish(hidden);
        assert!(replay.read(hidden, 0).is_none());
    }

    #[test]
    fn reconnect_and_overflow_are_explicit_and_generation_scoped() {
        let replay = Replay::default();
        let key = (Uuid::new_v4(), 1);
        replay.append(key, b"first");
        replay.append(key, b"second");
        let page = replay.read(key, 5).unwrap();
        assert_eq!(page.bytes, b"second");
        assert_eq!(page.end, 11);
        assert!(!page.gap);
        assert!(replay.read((key.0, 2), 0).is_none());
        replay.append(key, &vec![b'x'; STREAM_BYTES + 9]);
        let page = replay.read(key, 0).unwrap();
        assert!(page.gap);
        assert_eq!(page.bytes.len(), STREAM_BYTES);
        assert_eq!(page.end - page.start, STREAM_BYTES as u64);
        assert!(replay.read(key, page.end).unwrap().bytes.is_empty());
    }
}
