//! Store v5 (protocol 59, T3 section 5): a thread whose provider was
//! switched is a chain of provider sessions (segments). Exactly one segment
//! is the head: the session the thread runs on now, and the only one listed.
//! Every member carries `chain: {id, order}` (`id` the first segment's key,
//! `order` its activation); every member but the head carries `segmentOf`
//! naming the head. A switch moves the thread's owner-facing state to the
//! new head and points every member at it in the same commit.

use std::collections::BTreeMap;

use interface_contracts::{
    engine::EngineId,
    ipc::parse_key,
    threads::{ThreadChain, ThreadChainSegment, ThreadRun, TitleSource, WorkspaceMode},
};
use serde::{Deserialize, Serialize};

use super::{THREADS_INVALID_CODE, ThreadRecord, ThreadRefusal, ThreadsDocument, TurnRecord};

/// A thread's switch was refused: not a project thread.
pub const THREADS_SWITCH_PLAIN_CODE: &str = "threads-switch-plain";
/// The target is the provider the thread runs on.
pub const THREADS_SWITCH_SAME_CODE: &str = "threads-same-provider";

/// One record's place in its chain.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainLink {
    /// The chain's first segment's key.
    pub id: String,
    /// Activations: 0 for the first segment, one more each time a segment
    /// became the head.
    pub order: u32,
}

/// One segment as the host reads a chain: oldest activation first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainPart {
    pub key: String,
    pub order: u32,
    /// The head: the session the thread runs on now.
    pub head: bool,
    /// When it was created or first sent to.
    pub first_at: u64,
    /// When it last ran (its last turn's end or send, or its last run).
    pub last_at: u64,
}

/// The first words of the prompts a switch sends
/// (`interface_collaboration::thread_handoff::switch_prompt`); a turn whose
/// preview starts with one is a handoff, not the owner's prompt.
pub const HANDOFF_PREFIXES: [&str; 2] = [
    "You are taking over a thread from ",
    "The owner switched this thread back to you",
];

/// Whether a turn's prompt preview is a switch's handoff.
#[must_use]
pub fn is_handoff_preview(preview: &str) -> bool {
    HANDOFF_PREFIXES
        .iter()
        .any(|prefix| preview.starts_with(prefix))
}

impl ThreadRecord {
    /// Listed in the inbox view: not an earlier segment of a switched
    /// thread.
    #[must_use]
    pub const fn listed(&self) -> bool {
        self.segment_of.is_none()
    }

    fn first_at(&self) -> u64 {
        self.turns
            .first()
            .map_or(self.created_at, |turn| turn.sent_at.min(self.created_at))
    }

    fn last_at(&self) -> u64 {
        [
            Some(self.first_at()),
            self.turns.last().map(|turn| turn.sent_at),
            self.turns.last().and_then(|turn| turn.ended_at),
            self.last_run.as_ref().map(|run| run.started_at),
            self.last_run.as_ref().and_then(|run| run.ended_at),
        ]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(self.created_at)
    }
}

impl ThreadsDocument {
    /// The head of `key`'s thread: `key` itself for a head or an ordinary
    /// thread, the head it belongs to for a segment; `None` for a session
    /// with no record.
    #[must_use]
    pub fn head_of(&self, key: &str) -> Option<&str> {
        let record = self.record(key)?;
        Some(record.segment_of.as_deref().unwrap_or(&record.key))
    }

    /// Every record of `key`'s thread, oldest activation first (the head
    /// last); just its own record for an ordinary thread, none for an
    /// unknown key.
    #[must_use]
    pub fn chain_members(&self, key: &str) -> Vec<&ThreadRecord> {
        let Some(record) = self.record(key) else {
            return Vec::new();
        };
        let Some(chain) = &record.chain else {
            return vec![record];
        };
        let mut members = self
            .records
            .iter()
            .filter(|other| other.chain.as_ref().is_some_and(|link| link.id == chain.id))
            .collect::<Vec<_>>();
        members.sort_by_key(|member| {
            (
                member.segment_of.is_none(),
                member.chain.as_ref().map(|link| link.order),
            )
        });
        members
    }

    /// [`Self::chain_members`] as the host reads them.
    #[must_use]
    pub fn chain_parts(&self, key: &str) -> Vec<ChainPart> {
        self.chain_members(key)
            .into_iter()
            .map(|member| ChainPart {
                key: member.key.clone(),
                order: member.chain.as_ref().map_or(0, |link| link.order),
                head: member.listed(),
                first_at: member.first_at(),
                last_at: member.last_at(),
            })
            .collect()
    }

    /// The segment of `key`'s thread a switch to `engine` goes back to: an
    /// earlier segment on that provider whose session `exists`, the most
    /// recently active one; `None` means a new session.
    #[must_use]
    pub fn switch_target(
        &self,
        key: &str,
        engine: &str,
        exists: impl Fn(&str) -> bool,
    ) -> Option<String> {
        self.chain_members(key)
            .into_iter()
            .rev()
            .filter(|member| !member.listed())
            .find(|member| {
                parse_key(&member.key).is_ok_and(|(owner, _)| owner.as_str() == engine)
                    && exists(&member.key)
            })
            .map(|member| member.key.clone())
    }

    /// Makes `target` the head of `head`'s thread, in one change: a new
    /// session (no record yet, or one reconcile seeded a moment ago) joins
    /// the chain, or an earlier segment comes back. The thread's
    /// owner-facing state (its place and pin, snooze, auto-settle, visit
    /// and unread marks, workspace and worktree ownership, lineage, pull
    /// request, title source, release) moves from `head` to `target`; every
    /// member points at `target`. A new head on a provider whose session
    /// starts with its prompt (`launch_prompt`, ACP) records it as turn 0,
    /// running.
    ///
    /// # Errors
    /// Refuses an unknown thread, a segment, a plain chat, a target that is
    /// the thread's own session, and a target from another thread.
    pub fn switch_provider(
        &mut self,
        head: &str,
        target: &str,
        launch_prompt: Option<&str>,
        now: u64,
    ) -> Result<(), ThreadRefusal> {
        let head_index = self.index(head)?;
        let record = &self.records[head_index];
        if !record.listed() {
            return Err(ThreadRefusal::invalid(
                "That is an earlier part of a thread. Switch from the thread itself.",
            ));
        }
        if record
            .workspace
            .as_ref()
            .is_none_or(|workspace| workspace.mode == WorkspaceMode::None)
        {
            return Err(ThreadRefusal::new(
                THREADS_SWITCH_PLAIN_CODE,
                "Switching provider is for project threads. Start a project thread to work with another provider.",
            ));
        }
        if target == head {
            return Err(ThreadRefusal::new(
                THREADS_SWITCH_SAME_CODE,
                "The thread already runs on that session.",
            ));
        }
        let chain_id = record
            .chain
            .as_ref()
            .map_or_else(|| record.key.clone(), |link| link.id.clone());
        let next_order = self
            .chain_members(head)
            .iter()
            .filter_map(|member| member.chain.as_ref().map(|link| link.order))
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let back = self
            .record(target)
            .is_some_and(|other| other.chain.as_ref().is_some_and(|link| link.id == chain_id));
        if !back
            && self
                .record(target)
                .is_some_and(|other| other.chain.is_some())
        {
            return Err(ThreadRefusal::new(
                THREADS_INVALID_CODE,
                "That session belongs to another thread.",
            ));
        }
        if self.records[head_index].chain.is_none() {
            self.records[head_index].chain = Some(ChainLink {
                id: chain_id.clone(),
                order: 0,
            });
        }
        let target_index = if back {
            self.index(target)?
        } else {
            let index = self.ensure_record(target, now)?;
            let mut fresh = ThreadRecord::new(target, now);
            if let Some(prompt) = launch_prompt {
                fresh.turns.push(TurnRecord::new(0, prompt, now, None));
                fresh.last_run = Some(ThreadRun {
                    started_at: now,
                    ended_at: None,
                    outcome: None,
                });
            }
            self.records[index] = fresh;
            index
        };
        // Making room may have evicted a record, moving the head.
        let head_index = self.index(head)?;
        let moved = owner_state(&mut self.records[head_index]);
        let new_head = &mut self.records[target_index];
        moved.apply(new_head);
        new_head.segment_of = None;
        new_head.missing_since = None;
        new_head.chain = Some(ChainLink {
            id: chain_id.clone(),
            order: next_order,
        });
        let target = new_head.key.clone();
        for record in &mut self.records {
            if record.key != target
                && record
                    .chain
                    .as_ref()
                    .is_some_and(|link| link.id == chain_id)
            {
                record.segment_of = Some(target.clone());
            }
        }
        Ok(())
    }

    /// Puts every chain right before a commit: a chain whose head went (its
    /// session missing for a day, or evicted) gets its most recent segment
    /// as the head, a lone `segmentOf` without a chain is cleared, and every
    /// member points at the one head.
    pub(super) fn repair_chains(&mut self) {
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (index, record) in self.records.iter_mut().enumerate() {
            match &record.chain {
                Some(link) => groups.entry(link.id.clone()).or_default().push(index),
                None => record.segment_of = None,
            }
        }
        for members in groups.into_values() {
            let order = |index: usize| {
                self.records[index]
                    .chain
                    .as_ref()
                    .map_or(0, |link| link.order)
            };
            let heads = members
                .iter()
                .copied()
                .filter(|&index| self.records[index].listed())
                .collect::<Vec<_>>();
            let head = heads
                .iter()
                .copied()
                .max_by_key(|&index| (order(index), index))
                .or_else(|| {
                    members
                        .iter()
                        .copied()
                        .max_by_key(|&index| (order(index), index))
                });
            let Some(head) = head else {
                continue;
            };
            let head_key = self.records[head].key.clone();
            for &index in &members {
                let record = &mut self.records[index];
                record.segment_of = (index != head).then(|| head_key.clone());
            }
        }
    }

    /// Every chain has exactly one head, and every other member names it.
    pub(super) fn chains_valid(&self) -> bool {
        let mut heads: BTreeMap<&str, &str> = BTreeMap::new();
        for record in &self.records {
            if let Some(link) = &record.chain
                && record.listed()
                && heads
                    .insert(link.id.as_str(), record.key.as_str())
                    .is_some()
            {
                return false;
            }
        }
        self.records
            .iter()
            .all(|record| match (&record.chain, &record.segment_of) {
                (Some(link), Some(head)) => heads.get(link.id.as_str()) == Some(&head.as_str()),
                (None, Some(_)) => false,
                _ => true,
            })
    }

    /// The head's chain for its view: only when it has more than one
    /// segment.
    pub(super) fn chain_view(&self, record: &ThreadRecord) -> Option<ThreadChain> {
        record.chain.as_ref()?;
        let parts = self.chain_parts(&record.key);
        if parts.len() < 2 {
            return None;
        }
        Some(ThreadChain {
            segments: parts
                .into_iter()
                .filter_map(|part| {
                    let (engine, _) = parse_key(&part.key).ok()?;
                    let (added, removed) = self
                        .record(&part.key)
                        .and_then(|member| line_totals(&member.turns))
                        .unzip();
                    Some(ThreadChainSegment {
                        engine: EngineId::new(engine.as_str()).ok()?,
                        key: part.key,
                        order: part.order,
                        first_at: part.first_at,
                        last_at: part.last_at,
                        added,
                        removed,
                    })
                })
                .collect(),
        })
    }

    /// Files the checkpoint an ACP launch took before its agent started on
    /// the handoff as that turn's start (protocol 69), so the turn is
    /// measured like any other. Only a launch's own turn 0 that has none.
    pub fn file_launch_start(&mut self, key: &str, start_ref: String) {
        if let Some(turn) = self
            .records
            .iter_mut()
            .find(|record| record.key == key)
            .and_then(|record| record.turns.first_mut())
            .filter(|turn| turn.ordinal == 0 && turn.start_ref.is_none())
        {
            turn.start_ref = Some(start_ref);
        }
    }

    /// The earliest creation of `record`'s chain: the thread's age.
    pub(super) fn chain_created_at(&self, record: &ThreadRecord) -> u64 {
        if record.chain.is_none() {
            return record.created_at;
        }
        self.chain_members(&record.key)
            .iter()
            .map(|member| member.created_at)
            .min()
            .unwrap_or(record.created_at)
    }
}

/// A segment's line totals (protocol 69): its turns that measured both
/// counts, summed; `None` when none did. The renderer totals a thread's own
/// turns by the same rule (`threadDiffTotals`), so a head's segment agrees
/// with its turns.
fn line_totals(turns: &[TurnRecord]) -> Option<(u32, u32)> {
    turns
        .iter()
        .filter_map(|turn| turn.added.zip(turn.removed))
        .reduce(|(added, removed), (more, fewer)| {
            (added.saturating_add(more), removed.saturating_add(fewer))
        })
}

/// The owner-facing state a switch moves to the new head.
struct OwnerState {
    pinned_at: Option<u64>,
    pin_order: Option<String>,
    active_order: Option<String>,
    settled_at: Option<u64>,
    settled_reason: Option<interface_contracts::threads::SettledReason>,
    unsettled_at: Option<u64>,
    snoozed_until: Option<u64>,
    snoozed_at: Option<u64>,
    auto_settle_disabled: bool,
    last_visited_at: Option<u64>,
    marked_unread: bool,
    workspace: Option<interface_contracts::threads::ThreadWorkspace>,
    git_root: Option<String>,
    lineage: Option<interface_contracts::threads::ThreadLineage>,
    pull_request: Option<interface_contracts::threads::ThreadPullRequest>,
    pull_request_checked_at: Option<u64>,
    merged_seen_at: Option<u64>,
    pull_request_tracked: bool,
    title_source: Option<TitleSource>,
    release: Option<super::WorkspaceRelease>,
}

/// Takes the owner-facing state off `record`, which keeps only what a
/// segment needs: its key, creation, chain, turns (with their refs), run and
/// repository root (for its refs).
fn owner_state(record: &mut ThreadRecord) -> OwnerState {
    OwnerState {
        pinned_at: record.pinned_at.take(),
        pin_order: record.pin_order.take(),
        active_order: record.active_order.take(),
        settled_at: record.settled_at.take(),
        settled_reason: record.settled_reason.take(),
        unsettled_at: record.unsettled_at.take(),
        snoozed_until: record.snoozed_until.take(),
        snoozed_at: record.snoozed_at.take(),
        auto_settle_disabled: std::mem::take(&mut record.auto_settle_disabled),
        last_visited_at: record.last_visited_at.take(),
        marked_unread: std::mem::take(&mut record.marked_unread),
        workspace: record.workspace.take(),
        git_root: record.git_root.clone(),
        lineage: record.lineage.take(),
        pull_request: record.pull_request.take(),
        pull_request_checked_at: record.pull_request_checked_at.take(),
        merged_seen_at: record.merged_seen_at.take(),
        pull_request_tracked: std::mem::take(&mut record.pull_request_tracked),
        title_source: record.title_source.take(),
        release: record.release.take(),
    }
}

impl OwnerState {
    fn apply(self, record: &mut ThreadRecord) {
        record.pinned_at = self.pinned_at;
        record.pin_order = self.pin_order;
        record.active_order = self.active_order;
        record.settled_at = self.settled_at;
        record.settled_reason = self.settled_reason;
        record.unsettled_at = self.unsettled_at;
        record.snoozed_until = self.snoozed_until;
        record.snoozed_at = self.snoozed_at;
        record.auto_settle_disabled = self.auto_settle_disabled;
        record.last_visited_at = self.last_visited_at;
        record.marked_unread = self.marked_unread;
        record.workspace = self.workspace;
        record.git_root = self.git_root;
        record.lineage = self.lineage;
        record.pull_request = self.pull_request;
        record.pull_request_checked_at = self.pull_request_checked_at;
        record.merged_seen_at = self.merged_seen_at;
        record.pull_request_tracked = self.pull_request_tracked;
        record.title_source = self.title_source;
        record.release = self.release;
    }
}

#[cfg(test)]
#[path = "chain_tests.rs"]
mod tests;
