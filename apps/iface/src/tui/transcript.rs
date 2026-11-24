//! A session's transcript cells plus the change marks the layout cache reads.
//!
//! Every mutation goes through a method here, so no edit can bypass the
//! marks: the renderer asks which absolute cell index changed first since the
//! version it last laid out, and restyles only from there.
use std::collections::{HashMap, VecDeque, hash_map::Entry};
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{TRANSCRIPT_LIMIT, TranscriptCell};

/// Distinct changes remembered between two layouts; older ones force a full relayout.
const CHANGE_LOG_LIMIT: usize = 64;

/// Bytes of diff text (before plus after) one transcript keeps. Past it the
/// oldest diffs give theirs up; their cells stay and draw the same.
const DIFF_TEXT_BUDGET_BYTES: usize = 4 * 1024 * 1024;

static NEXT_TRANSCRIPT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(super) struct Transcript {
    cells: Arc<Vec<TranscriptCell>>,
    id: u64,
    /// Absolute index of `cells[0]`: every cell ever dropped from the front
    /// (the `TRANSCRIPT_LIMIT` drain and `clear`) keeps its number.
    front: u64,
    version: u64,
    /// `(version, absolute index)` per distinct change, oldest first. A run of
    /// edits to one cell (streamed deltas) coalesces into one entry.
    changes: VecDeque<(u64, u64)>,
    /// Every change after this version is still in `changes`.
    covered_after: u64,
    /// Absolute index of the first tool cell per call id, so a tool event
    /// finds its cell without scanning every cell before it. Tool cells are
    /// only added by `push`, and no edit changes a cell's call id or kind.
    tools: Arc<HashMap<String, u64>>,
    /// A tool cell was pushed while an earlier one had the same call id. Only
    /// then can dropping the earlier one leave another to index.
    duplicate_tools: bool,
    /// Absolute indices of the diff cells still holding text, oldest first,
    /// and the bytes of that text.
    diff_cells: VecDeque<u64>,
    diff_bytes: usize,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl Deref for Transcript {
    type Target = [TranscriptCell];

    fn deref(&self) -> &[TranscriptCell] {
        &self.cells
    }
}

impl Transcript {
    pub(super) fn new(cells: Vec<TranscriptCell>) -> Self {
        let mut tools = HashMap::new();
        let mut duplicate_tools = false;
        let mut diff_cells = VecDeque::new();
        let mut diff_bytes = 0;
        for (index, cell) in cells.iter().enumerate() {
            if let Some(call_id) = tool_call_id(cell) {
                duplicate_tools |= tools.contains_key(call_id);
                tools.entry(call_id.to_owned()).or_insert(index as u64);
            }
            if let Some(bytes) = diff_text_bytes(cell) {
                diff_cells.push_back(index as u64);
                diff_bytes += bytes;
            }
        }
        let mut transcript = Self {
            cells: Arc::new(cells),
            id: NEXT_TRANSCRIPT_ID.fetch_add(1, Ordering::Relaxed),
            front: 0,
            version: 0,
            changes: VecDeque::new(),
            covered_after: 0,
            tools: Arc::new(tools),
            duplicate_tools,
            diff_cells,
            diff_bytes,
        };
        transcript.release_diff_text();
        transcript
    }

    /// A shared handle for readers that outlive the state lock. Holding it
    /// while the transcript is edited makes that edit clone every cell.
    pub(super) fn snapshot(&self) -> Arc<Vec<TranscriptCell>> {
        Arc::clone(&self.cells)
    }

    pub(super) const fn id(&self) -> u64 {
        self.id
    }

    pub(super) const fn front(&self) -> u64 {
        self.front
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    /// The lowest absolute cell index changed after `version`: `None` when
    /// nothing changed, `Some(front)`-or-lower when the log no longer reaches
    /// back that far (callers then relayout everything).
    pub(super) fn first_change_since(&self, version: u64) -> Option<u64> {
        if version == self.version {
            return None;
        }
        if version < self.covered_after || version > self.version {
            return Some(0);
        }
        self.changes
            .iter()
            .filter(|(changed, _)| *changed > version)
            .map(|(_, index)| *index)
            .min()
    }

    pub(super) fn push(&mut self, cell: TranscriptCell) {
        let index = self.front + self.cells.len() as u64;
        if let Some(call_id) = tool_call_id(&cell) {
            match Arc::make_mut(&mut self.tools).entry(call_id.to_owned()) {
                Entry::Vacant(entry) => {
                    entry.insert(index);
                }
                Entry::Occupied(_) => self.duplicate_tools = true,
            }
        }
        if let Some(bytes) = diff_text_bytes(&cell) {
            self.diff_cells.push_back(index);
            self.diff_bytes += bytes;
        }
        let cells = Arc::make_mut(&mut self.cells);
        cells.push(cell);
        if cells.len() > TRANSCRIPT_LIMIT {
            let excess = cells.len() - TRANSCRIPT_LIMIT;
            let first = self.front;
            let tools = Arc::make_mut(&mut self.tools);
            let mut uncovered = Vec::new();
            for (offset, dropped) in cells.drain(..excess).enumerate() {
                self.diff_bytes -= diff_text_bytes(&dropped).unwrap_or(0);
                if let Some(call_id) = tool_call_id(&dropped)
                    && tools.get(call_id) == Some(&(first + offset as u64))
                {
                    tools.remove(call_id);
                    if self.duplicate_tools {
                        uncovered.push(call_id.to_owned());
                    }
                }
            }
            self.front += excess as u64;
            while self
                .diff_cells
                .front()
                .is_some_and(|index| *index < self.front)
            {
                self.diff_cells.pop_front();
            }
            for call_id in uncovered {
                if let Some(next) = cells
                    .iter()
                    .position(|cell| tool_call_id(cell) == Some(call_id.as_str()))
                {
                    tools.insert(call_id, self.front + next as u64);
                }
            }
        }
        self.release_diff_text();
        self.mark(index);
    }

    /// Releases the oldest diffs' text until what is left fits the budget.
    /// The released cells are not marked changed: a diff draws as one line
    /// from its path, so no layout reads the text; only the details overlay.
    fn release_diff_text(&mut self) {
        while self.diff_bytes > DIFF_TEXT_BUDGET_BYTES {
            let Some(absolute) = self.diff_cells.pop_front() else {
                break;
            };
            let Some(TranscriptCell::Diff {
                before,
                after,
                truncated,
                ..
            }) = absolute
                .checked_sub(self.front)
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| Arc::make_mut(&mut self.cells).get_mut(index))
            else {
                continue;
            };
            let released = before.take().map_or(0, |text| text.len())
                + after.take().map_or(0, |text| text.len());
            self.diff_bytes -= released;
            *truncated = true;
        }
    }

    /// Drops every cell. New cells get fresh absolute indices, so no cached
    /// layout of the old ones can be mistaken for them.
    pub(super) fn clear(&mut self) {
        let cells = Arc::make_mut(&mut self.cells);
        self.front += cells.len() as u64;
        cells.clear();
        self.tools = Arc::default();
        self.duplicate_tools = false;
        self.diff_cells.clear();
        self.diff_bytes = 0;
        self.mark(self.front);
    }

    /// The last cell for editing. It is marked changed even if the caller
    /// only inspects it; an extra restyle of one cell is harmless.
    pub(super) fn last_mut(&mut self) -> Option<&mut TranscriptCell> {
        let last = self.cells.len().checked_sub(1)?;
        self.mark(self.front + last as u64);
        Arc::make_mut(&mut self.cells).last_mut()
    }

    /// The first tool cell with `call_id`, for editing.
    pub(super) fn tool_mut(&mut self, call_id: &str) -> Option<&mut TranscriptCell> {
        let absolute = *self.tools.get(call_id)?;
        let index = usize::try_from(absolute.checked_sub(self.front)?).ok()?;
        debug_assert_eq!(
            self.cells.get(index).and_then(tool_call_id),
            Some(call_id),
            "the tool index points at its cell"
        );
        self.mark(absolute);
        Arc::make_mut(&mut self.cells).get_mut(index)
    }

    /// The last cell matching `predicate`, for editing.
    pub(super) fn rfind_mut(
        &mut self,
        predicate: impl Fn(&TranscriptCell) -> bool,
    ) -> Option<&mut TranscriptCell> {
        let index = self.cells.iter().rposition(predicate)?;
        self.mark(self.front + index as u64);
        Arc::make_mut(&mut self.cells).get_mut(index)
    }

    fn mark(&mut self, index: u64) {
        self.version += 1;
        match self.changes.back_mut() {
            Some((changed, last)) if *last == index => *changed = self.version,
            _ => {
                self.changes.push_back((self.version, index));
                if self.changes.len() > CHANGE_LOG_LIMIT
                    && let Some((dropped, _)) = self.changes.pop_front()
                {
                    self.covered_after = dropped;
                }
            }
        }
    }
}

/// The bytes of diff text `cell` holds, if it is a diff holding any.
fn diff_text_bytes(cell: &TranscriptCell) -> Option<usize> {
    match cell {
        TranscriptCell::Diff { before, after, .. } if before.is_some() || after.is_some() => {
            Some(before.as_ref().map_or(0, String::len) + after.as_ref().map_or(0, String::len))
        }
        _ => None,
    }
}

fn tool_call_id(cell: &TranscriptCell) -> Option<&str> {
    match cell {
        TranscriptCell::Tool { call_id, .. } => Some(call_id),
        _ => None,
    }
}
