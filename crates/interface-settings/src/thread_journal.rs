//! The thread journal: an append-only, per-thread record of what happened
//! (prompts, answers, tool calls, approvals, how each turn ended), written by
//! the host as events happen. Contract:
//! `docs/superpowers/specs/2026-11-14-t3-systems-design.md` section 1.
//!
//! `<data root>/thread-journal/<h>/segment-<20-digit first seq>.jsonl`, `<h>`
//! the first 16 hex of sha256(session key). The first line of a segment is a
//! [`JournalHeader`]; every other line one [`JournalLine`]. One
//! [`ThreadJournal`] (the process holding `host.lock`) writes; the readers in
//! [`read`] take no lock and skip what they cannot parse.
//!
//! Children: `fold.rs` (engine events into entries, joining message deltas),
//! `bounds.rs` (the 64 KiB entry bound), `store.rs` (segments, rotation,
//! retention, quarantine, the restart marker), `read.rs` (pages, replay to
//! engine events, search, last answer, watch), `import.rs` (a provider's
//! history into entries).

mod bounds;
mod fold;
mod import;
pub mod read;
mod store;

pub use bounds::{ENTRY_LIMIT, bounded_line};
pub use fold::{JournalFold, entry_for_event, is_journalled};
pub use import::{import_entries, import_entries_from};
pub use read::{
    ChainSegment, EndedTurn, JournalWatch, SearchOutcome, last_answer, last_ended_turn,
    merge_visuals, read, replay, replay_chain, replay_chain_history, replay_history, search,
    search_with, tail, visual_events, watch,
};
pub use store::{JournalLimits, ThreadJournal};

/// The journal's directory under the data root.
pub const JOURNAL_DIR: &str = "thread-journal";
/// Where unreadable segments go, under [`JOURNAL_DIR`].
pub const QUARANTINE_DIR: &str = "quarantine";
/// A segment rotates before it would pass this many bytes.
pub const SEGMENT_LIMIT: usize = 1024 * 1024;
/// A thread keeps its newest this many segments.
pub const RETAINED_SEGMENTS: usize = 32;
/// The directory holds at most this many threads...
pub const MAX_THREADS: usize = 4096;
/// ...and at most this many bytes.
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;
/// A `read` or `tail` page answers at most this many serialized bytes.
pub const PAGE_BYTE_LIMIT: usize = 1024 * 1024;
/// `search` answers at most this many hits...
pub const SEARCH_HIT_LIMIT: usize = 100;
/// ...and at most this many from one thread.
pub const SEARCH_HITS_PER_THREAD: usize = 5;

/// The directory name for `key`: the first 16 hex of sha256(key).
#[must_use]
pub fn thread_hash(key: &str) -> String {
    let mut hash = interface_storage::sha256_hex(key.as_bytes());
    hash.truncate(16);
    hash
}

#[cfg(test)]
mod tests;
