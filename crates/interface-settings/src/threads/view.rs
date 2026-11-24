//! Read-only thread projection and the ordering shared by reads and mutations.
//! Persisted records and transitions stay in the parent module.

use super::{ThreadRecord, ThreadsDocument, TurnRecord, keyed_first};
use interface_contracts::threads::{
    THREADS_VIEW_VERSION, ThreadSection, ThreadTurn, ThreadView, ThreadsView, WorkspaceMode,
};
use std::{cmp::Ordering, collections::BTreeMap};

impl ThreadRecord {
    fn view(
        &self,
        document: &ThreadsDocument,
        now: u64,
        worktree_missing: &mut impl FnMut(&str) -> bool,
    ) -> ThreadView {
        let workspace = self.workspace.clone().map(|mut workspace| {
            workspace.missing = (workspace.mode == WorkspaceMode::Worktree).then(|| {
                workspace
                    .worktree_path
                    .as_deref()
                    .is_none_or(worktree_missing)
            });
            // Only while the thread still owns it: a worktree handed on to
            // a new thread (Edit from here) is that thread's now.
            workspace.kept = self
                .release
                .filter(|_| workspace.owned)
                .and_then(|release| release.outcome.kept());
            workspace
        });
        ThreadView {
            key: self.key.clone(),
            section: self.section(now),
            // A switched thread is as old as its first segment.
            created_at: document.chain_created_at(self),
            pinned_at: self.pinned_at,
            pin_order: self.pin_order.clone(),
            active_order: self.active_order.clone(),
            settled_at: self.settled_at,
            settled_reason: self.settled_reason,
            snoozed_until: self.snoozed_until,
            woke: self.woke(now),
            unread: self.unread(),
            last_visited_at: self.last_visited_at,
            last_run: self.last_run.clone(),
            auto_settle: !self.auto_settle_disabled,
            workspace,
            lineage: self.lineage.clone(),
            pull_request: self.pull_request.clone(),
            title_source: self.title_source,
            turns: self.turns.iter().map(TurnRecord::view).collect(),
            // The thread host joins it from the Away store.
            away: None,
            // The thread host joins it from the session accounts store.
            account: None,
            chain: document.chain_view(self),
            schedules: None,
            // The thread host joins it from the team tools ledger.
            started_from_outside: None,
            // The thread host joins it from its run tap (protocol 66).
            limit: None,
            secret_request: None,
            side: None,
            folder_trust: None,
        }
    }
}

impl TurnRecord {
    fn view(&self) -> ThreadTurn {
        ThreadTurn {
            ordinal: self.ordinal,
            sent_at: self.sent_at,
            prompt_digest: self.prompt_digest.clone(),
            has_start: self.start_ref.is_some(),
            has_end: self.end_ref.is_some(),
            state: self.state,
            ended_at: self.ended_at,
            files_changed: self.files_changed,
            added: self.added,
            removed: self.removed,
        }
    }
}

impl ThreadsDocument {
    /// The listed records of `section` in sidebar order. Pinned and Active: keyed
    /// first by key, then the rest newest first; Snoozed by wake time; Settled
    /// newest settle first.
    #[must_use]
    pub fn ordered(&self, section: ThreadSection, now: u64) -> Vec<usize> {
        let records = &self.records;
        // An earlier segment of a switched thread is never listed or placed.
        let mut indices = (0..records.len())
            .filter(|&index| records[index].listed() && records[index].section(now) == section)
            .collect::<Vec<_>>();
        indices.sort_by(|&left, &right| self.compare_records(left, right, section));
        indices
    }

    /// The whole view, in sidebar order: Pinned, Active, Snoozed, Settled,
    /// From terminals.
    /// `worktree_missing` answers whether a worktree folder is absent.
    #[must_use]
    pub fn view(&self, now: u64, worktree_missing: impl Fn(&str) -> bool) -> ThreadsView {
        // One filesystem observation per distinct path, only for this view.
        // Forks can share a worktree; a later read must check it again.
        let mut missing = BTreeMap::new();
        let mut check_missing = |path: &str| {
            *missing
                .entry(path.to_owned())
                .or_insert_with(|| worktree_missing(path))
        };
        let mut threads = Vec::with_capacity(self.records.len());
        for section in [
            ThreadSection::Pinned,
            ThreadSection::Active,
            ThreadSection::Snoozed,
            ThreadSection::Settled,
            ThreadSection::FromTerminals,
        ] {
            threads.extend(
                self.ordered(section, now)
                    .into_iter()
                    .map(|index| self.records[index].view(self, now, &mut check_missing)),
            );
        }
        ThreadsView {
            version: THREADS_VIEW_VERSION,
            revision: self.revision,
            now,
            settings: self.settings.clone(),
            threads,
            hidden_keys: Vec::new(),
        }
    }

    pub(super) fn compare_records(
        &self,
        left: usize,
        right: usize,
        section: ThreadSection,
    ) -> Ordering {
        let (a, b) = (&self.records[left], &self.records[right]);
        let order = match section {
            ThreadSection::Pinned => keyed_first(a.pin_order.as_deref(), b.pin_order.as_deref())
                .then_with(|| b.pinned_at.cmp(&a.pinned_at)),
            ThreadSection::Active => {
                keyed_first(a.active_order.as_deref(), b.active_order.as_deref())
                    .then_with(|| b.created_at.cmp(&a.created_at))
            }
            ThreadSection::Snoozed => a.snoozed_until.cmp(&b.snoozed_until),
            ThreadSection::Settled | ThreadSection::FromTerminals => {
                b.settled_at.cmp(&a.settled_at)
            }
        };
        order.then_with(|| a.key.cmp(&b.key))
    }
}
