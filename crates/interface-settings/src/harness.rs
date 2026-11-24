//! Durable attention and immutable evidence; independent of legacy store schemas.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::harness::{AttentionItem, AttentionTarget, HarnessState, TaskEvidence};
use interface_storage::{JsonLimits, StorageError};
use std::path::Path;

const BYTES: usize = 8 * 1024 * 1024;
const ATTENTION_LIMIT: usize = 4096;
const TASK_LIMIT: usize = 512;
// Retention (approved by the owner on 27 Sep 2026). A write that grows history
// past a soft cap first drops the oldest resolved attention and the oldest
// settled evidence, so normal use never reaches the hard limits above. Nothing
// unresolved is dropped: when only unresolved items remain at a hard limit the
// write is refused as before. The file format is unchanged; it holds fewer
// resolved items.
const ATTENTION_SOFT_CAP: usize = ATTENTION_LIMIT / 2;
const TASK_SOFT_CAP: usize = TASK_LIMIT / 2;
/// A write refused at the byte cap is retried once after resolved history is
/// trimmed to this size.
const SOFT_BYTES: usize = BYTES / 4 * 3;
fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn target_valid(target: &AttentionTarget) -> bool {
    match target {
        AttentionTarget::Session { key } => text(key, 512),
        AttentionTarget::Terminal {
            terminal_id,
            generation,
        } => text(terminal_id, 128) && *generation > 0 && *generation <= 9_007_199_254_740_991,
    }
}
fn validate(state: &HarnessState) -> Result<(), SettingsError> {
    if state.version != 1
        || state.revision >= 9_007_199_254_740_991
        || state.attention.len() > ATTENTION_LIMIT
        || state.tasks.len() > TASK_LIMIT
    {
        return Err(invalid("Unsupported or full harness store."));
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in &state.attention {
        if !ids.insert(&item.id)
            || !text(&item.id, 512)
            || !text(&item.title, 512)
            || item.place.len() > 4096
            || item.place.chars().any(char::is_control)
            || !target_valid(&item.target)
            || !matches!(
                item.kind.as_str(),
                "finished" | "failed" | "needs-input" | "interrupted"
            )
            || !matches!(
                item.provenance.as_str(),
                "provider" | "terminal-hook" | "renderer-observation" | "check" | "recovery"
            )
            || item.at > 8_640_000_000_000_000
        {
            return Err(invalid("Invalid attention identity or fields."));
        }
    }
    if state.branches.len() > 512 {
        return Err(invalid("Branch history is full."));
    }
    let mut children = std::collections::BTreeSet::new();
    for branch in &state.branches {
        if !text(&branch.parent, 512)
            || !text(&branch.child, 512)
            || branch.parent == branch.child
            || !children.insert(&branch.child)
            || !text(&branch.root, 4096)
            || !Path::new(&branch.root).is_absolute()
            || branch.boundary != "provider-current"
        {
            return Err(invalid("Invalid conversation branch."));
        }
    }
    if state.drafts.len() > 512 {
        return Err(invalid("Draft store is full."));
    }
    let mut targets = std::collections::BTreeSet::new();
    for draft in &state.drafts {
        if !targets.insert(
            serde_json::to_string(&draft.target).map_err(|_| invalid("Invalid draft target"))?,
        ) || !target_valid(&draft.target)
            || draft.text.len() > 48_000
            || draft.text.contains('\0')
            || draft.revision >= 9_007_199_254_740_990
        {
            return Err(invalid("Invalid draft."));
        }
    }
    ids.clear();
    for task in &state.tasks {
        if !ids.insert(&task.id)
            || !text(&task.id, 512)
            || !text(&task.objective, 8000)
            || !text(&task.root, 4096)
            || !Path::new(&task.root).is_absolute()
            || !text(&task.baseline, 128)
            || !text(&task.candidate, 128)
            || task
                .check_candidate
                .as_ref()
                .is_some_and(|value| !text(value, 128))
            || !text(&task.state, 512)
            || task.at > 8_640_000_000_000_000
            || task.changed_paths.iter().any(|path| {
                !text(path, 4096)
                    || !Path::new(path)
                        .components()
                        .all(|part| matches!(part, std::path::Component::Normal(_)))
            })
            || task.findings.iter().any(|finding| !text(finding, 8000))
            || task
                .checks
                .iter()
                .any(|check| check.stdout.len() > 1_048_576 || check.stderr.len() > 1_048_576)
            || !target_valid(&task.target)
            || task.changed_paths.len() > 1000
            || task.checks.len() > 100
            || task.findings.len() > 100
        {
            return Err(invalid("Invalid task evidence."));
        }
    }
    Ok(())
}
/// Drops the oldest items `evictable` accepts, never the newest item, while
/// `take` asks for more room. Order is otherwise preserved.
fn drop_oldest<T>(
    items: &mut Vec<T>,
    evictable: impl Fn(&T) -> bool,
    mut take: impl FnMut(&T) -> bool,
) {
    let newest = items.len().saturating_sub(1);
    let mut index = 0;
    items.retain(|item| {
        let keep = index == newest || !evictable(item) || !take(item);
        index += 1;
        keep
    });
}
/// Evidence is settled once no unresolved attention names its target. The
/// newest evidence (the result being recorded) is never dropped.
fn drop_settled_tasks(state: &mut HarnessState, take: impl FnMut(&TaskEvidence) -> bool) {
    let HarnessState {
        attention, tasks, ..
    } = state;
    let open: Vec<&AttentionTarget> = attention
        .iter()
        .filter(|item| !item.resolved)
        .map(|item| &item.target)
        .collect();
    drop_oldest(tasks, |task| !open.contains(&&task.target), take);
}
/// Takes one more item while `excess` is positive, counting it down.
fn take_one(excess: &mut usize) -> bool {
    if *excess == 0 {
        return false;
    }
    *excess -= 1;
    true
}
/// Takes one more item while `over` bytes remain to be freed, counting down
/// the item's compact encoding. That never exceeds what the item adds to the
/// pretty-printed file, so dropping the taken items frees at least `over`.
fn take_bytes(over: &mut usize, item: &impl serde::Serialize) -> bool {
    if *over == 0 {
        return false;
    }
    *over = over.saturating_sub(serde_json::to_vec(item).map_or(0, |bytes| bytes.len()));
    true
}
/// Past the soft caps, drops the oldest resolved attention and the oldest
/// settled evidence, down to the caps or until nothing resolved is left.
fn retain(state: &mut HarnessState) {
    let mut excess = state.attention.len().saturating_sub(ATTENTION_SOFT_CAP);
    if excess > 0 {
        drop_oldest(
            &mut state.attention,
            |item| item.resolved,
            |_| take_one(&mut excess),
        );
    }
    let mut excess = state.tasks.len().saturating_sub(TASK_SOFT_CAP);
    if excess > 0 {
        drop_settled_tasks(state, |_| take_one(&mut excess));
    }
}
/// After a write `actual` bytes long was refused at the byte cap: drops the
/// oldest settled evidence, then the oldest resolved attention, until the file
/// would be at most [`SOFT_BYTES`]. Returns whether anything was dropped.
fn trim_to_soft_bytes(state: &mut HarnessState, actual: u64) -> bool {
    let before = (state.attention.len(), state.tasks.len());
    let mut over = usize::try_from(actual)
        .unwrap_or(usize::MAX)
        .saturating_sub(SOFT_BYTES);
    drop_settled_tasks(state, |task| take_bytes(&mut over, task));
    drop_oldest(
        &mut state.attention,
        |item| item.resolved,
        |item| take_bytes(&mut over, item),
    );
    before != (state.attention.len(), state.tasks.len())
}
pub struct HarnessStore {
    document: LeasedDocument,
    state: HarnessState,
}
impl HarnessStore {
    /// # Errors
    /// Refuses unsafe, corrupt, unknown-version, or concurrently owned storage.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        let (document, state) = LeasedDocument::open::<HarnessState>(
            file,
            ".harness.lock",
            JsonLimits::new(BYTES, 24, 500_000)?,
            "Harness persistence needs recovery after an uncertain write.",
        )?;
        let state = state.unwrap_or_default();
        validate(&state)?;
        Ok(Self { document, state })
    }
    #[must_use]
    pub fn snapshot(&self) -> HarnessState {
        self.state.clone()
    }
    /// The saved draft for one target, cloning only that draft.
    #[must_use]
    pub fn draft(
        &self,
        target: &AttentionTarget,
    ) -> Option<interface_contracts::harness::SharedDraft> {
        self.state
            .drafts
            .iter()
            .find(|draft| &draft.target == target)
            .cloned()
    }
    /// Up to `limit` of the newest tasks `keep` accepts, newest first,
    /// cloning only those tasks.
    #[must_use]
    pub fn recent_tasks(
        &self,
        keep: impl Fn(&TaskEvidence) -> bool,
        limit: usize,
    ) -> Vec<TaskEvidence> {
        self.state
            .tasks
            .iter()
            .rev()
            .filter(|task| keep(task))
            .take(limit)
            .cloned()
            .collect()
    }
    fn commit(&mut self, mut state: HarnessState) -> Result<HarnessState, SettingsError> {
        self.document.ensure_writable()?;
        if state == self.state {
            return Ok(state);
        }
        state.revision += 1;
        validate(&state)?;
        match self.document.commit(&state) {
            // Nothing was written; resolved history makes room for one retry.
            Err(SettingsError::Storage(StorageError::BoundsExceeded { actual_bytes, .. }))
                if trim_to_soft_bytes(&mut state, actual_bytes) =>
            {
                self.document.commit(&state)?;
            }
            result => result?,
        }
        self.state = state;
        Ok(self.snapshot())
    }
    /// # Errors
    /// Refuses conflicting event identity and never evicts unresolved work.
    /// Past the soft cap the oldest resolved attention makes room.
    pub fn record(&mut self, mut item: AttentionItem) -> Result<HarnessState, SettingsError> {
        let mut next = self.state.clone();
        if let Some(old) = next.attention.iter().find(|old| old.id == item.id) {
            if old.target != item.target || old.kind != item.kind {
                return Err(invalid(
                    "Attention ID reused for a different target or event.",
                ));
            }
            return Ok(self.snapshot()); // Reconnect cannot resurrect resolved work or reset seen state.
        }
        item.seen = false;
        item.resolved = false;
        next.attention.push(item);
        retain(&mut next);
        self.commit(next)
    }
    /// # Errors
    /// Fails if durable storage cannot acknowledge viewing. Viewing never resolves a need.
    pub fn seen(&mut self, ids: &[String]) -> Result<HarnessState, SettingsError> {
        if ids.len() > 4096 {
            return Err(invalid("Too many attention IDs."));
        }
        let mut next = self.state.clone();
        for item in &mut next.attention {
            if ids.contains(&item.id) {
                item.seen = true;
            }
        }
        self.commit(next)
    }
    /// # Errors
    /// Requires the original exact target. Resolution is separate from navigation.
    pub fn resolve(
        &mut self,
        id: &str,
        target: &AttentionTarget,
    ) -> Result<HarnessState, SettingsError> {
        let mut next = self.state.clone();
        let item = next
            .attention
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| invalid("Attention item is no longer available."))?;
        if &item.target != target {
            return Err(invalid("Attention target changed."));
        }
        item.resolved = true;
        self.commit(next)
    }
    /// # Errors
    /// Rejects stale edits rather than overwriting another screen's draft.
    pub fn save_draft(
        &mut self,
        target: AttentionTarget,
        revision: u64,
        text: String,
    ) -> Result<HarnessState, SettingsError> {
        let mut next = self.state.clone();
        if let Some(draft) = next.drafts.iter_mut().find(|draft| draft.target == target) {
            if draft.revision != revision {
                return Err(invalid(
                    "Draft changed on another screen. Reload it before saving.",
                ));
            }
            if draft.text == text {
                return Ok(next);
            }
            draft.text = text;
            draft.revision += 1;
        } else {
            if revision != 0 {
                return Err(invalid("Draft revision is stale."));
            }
            next.drafts.push(interface_contracts::harness::SharedDraft {
                target,
                revision: 1,
                text,
            });
        }
        self.commit(next)
    }
    /// # Errors
    /// Preserves the original parent of a confirmed provider-created conversation.
    pub fn record_branch(
        &mut self,
        branch: interface_contracts::harness::ConversationBranch,
    ) -> Result<HarnessState, SettingsError> {
        let mut next = self.state.clone();
        if let Some(old) = next.branches.iter().find(|old| old.child == branch.child) {
            if old != &branch {
                return Err(invalid(
                    "Conversation already has a different recorded parent.",
                ));
            }
            return Ok(next);
        }
        next.branches.push(branch);
        self.commit(next)
    }
    /// # Errors
    /// Evidence is immutable. A repeated identity must contain identical evidence.
    /// Past the soft cap the oldest settled evidence makes room.
    pub fn evidence(&mut self, task: TaskEvidence) -> Result<HarnessState, SettingsError> {
        let mut next = self.state.clone();
        if let Some(old) = next.tasks.iter().find(|old| old.id == task.id) {
            if old != &task {
                return Err(invalid("Result ID already names a different candidate."));
            }
            return Ok(next);
        }
        next.tasks.push(task);
        retain(&mut next);
        self.commit(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item() -> AttentionItem {
        AttentionItem {
            id: "done:t:1:1".into(),
            kind: "finished".into(),
            target: AttentionTarget::Terminal {
                terminal_id: "t".into(),
                generation: 1,
            },
            title: "Review result".into(),
            place: "project".into(),
            at: 1,
            provenance: "terminal-hook".into(),
            seen: false,
            resolved: false,
        }
    }
    #[test]
    fn reopen_dedup_seen_and_generation_fences() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        let mut store = HarnessStore::open(&file).unwrap();
        assert!(HarnessStore::open(&file).is_err());
        store.record(item()).unwrap();
        store.seen(&[item().id]).unwrap();
        drop(store);
        let mut store = HarnessStore::open(&file).unwrap();
        assert!(store.snapshot().attention[0].seen);
        assert!(!store.snapshot().attention[0].resolved);
        let wrong = AttentionTarget::Terminal {
            terminal_id: "t".into(),
            generation: 2,
        };
        assert!(store.resolve(&item().id, &wrong).is_err());
        store.resolve(&item().id, &item().target).unwrap();
        store.record(item()).unwrap();
        assert_eq!(store.snapshot().attention.len(), 1);
        assert!(store.snapshot().attention[0].resolved);
        let mut other = item();
        other.target = wrong;
        assert!(store.record(other).is_err());
    }
    #[test]
    fn simultaneous_screen_edits_cannot_overwrite_a_draft() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = HarnessStore::open(&temp.path().join("harness.v1.json")).unwrap();
        let target = item().target;
        store
            .save_draft(target.clone(), 0, "desktop edit".into())
            .unwrap();
        assert!(
            store
                .save_draft(target.clone(), 0, "phone edit".into())
                .is_err()
        );
        assert_eq!(store.snapshot().drafts[0].text, "desktop edit");
        store
            .save_draft(target.clone(), 1, "merged deliberately".into())
            .unwrap();
        assert_eq!(store.snapshot().drafts[0].revision, 2);
        let replacement = AttentionTarget::Terminal {
            terminal_id: "t".into(),
            generation: 2,
        };
        store
            .save_draft(replacement.clone(), 0, "separate process".into())
            .unwrap();
        assert_eq!(store.snapshot().drafts.len(), 2);
        assert_eq!(store.draft(&target).unwrap().text, "merged deliberately");
        assert_eq!(store.draft(&replacement).unwrap().revision, 1);
        let absent = AttentionTarget::Terminal {
            terminal_id: "t".into(),
            generation: 3,
        };
        assert!(store.draft(&absent).is_none());
    }
    #[test]
    fn recent_tasks_are_the_newest_matches_first() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = HarnessStore::open(&temp.path().join("harness.v1.json")).unwrap();
        for index in 0..5_u64 {
            store
                .evidence(TaskEvidence {
                    id: format!("task-{index}"),
                    objective: "Check the result".into(),
                    target: AttentionTarget::Terminal {
                        terminal_id: "t".into(),
                        generation: 1 + index % 2,
                    },
                    root: "/project".into(),
                    baseline: "base".into(),
                    candidate: "candidate".into(),
                    changed_paths: Vec::new(),
                    checks: Vec::new(),
                    check_candidate: None,
                    findings: Vec::new(),
                    state: "confirmed".into(),
                    at: index,
                })
                .unwrap();
        }
        let first_generation = |task: &TaskEvidence| {
            matches!(task.target, AttentionTarget::Terminal { generation: 1, .. })
        };
        let ids =
            |tasks: Vec<TaskEvidence>| tasks.into_iter().map(|task| task.id).collect::<Vec<_>>();
        assert_eq!(
            ids(store.recent_tasks(first_generation, 20)),
            ["task-4", "task-2", "task-0"]
        );
        assert_eq!(
            ids(store.recent_tasks(first_generation, 2)),
            ["task-4", "task-2"]
        );
    }
    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        let mut store = HarnessStore::open(&file).unwrap();
        store.record(item()).unwrap();
        crate::golden::assert_golden("harness.recorded.json", temp.path(), &file);
        drop(store);
        let mut store = HarnessStore::open(&file).unwrap();
        store.seen(&[item().id]).unwrap();
        store
            .save_draft(item().target, 0, "draft text".into())
            .unwrap();
        crate::golden::assert_golden("harness.seen.json", temp.path(), &file);
        assert!(temp.path().join(".harness.lock").exists());
    }
    #[test]
    fn an_oversized_result_is_refused_and_later_records_still_save() {
        use interface_contracts::ipc::{
            ReviewCheckName, ReviewCheckRunResult, ReviewCheckRunStatus,
        };
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        let mut store = HarnessStore::open(&file).unwrap();
        store.record(item()).unwrap();
        let before = std::fs::read(&file).unwrap();
        // Nine checks of 1 MiB each pass validation but exceed the 8 MiB file.
        let check = ReviewCheckRunResult {
            name: ReviewCheckName::Test,
            status: ReviewCheckRunStatus::Failed,
            exit_code: Some(1),
            stdout: "x".repeat(1_048_576),
            stderr: String::new(),
            duration_ms: 1,
            output_truncated: true,
        };
        let evidence = TaskEvidence {
            id: "large".into(),
            objective: "Check the result".into(),
            target: item().target,
            root: "/project".into(),
            baseline: "base".into(),
            candidate: "candidate".into(),
            changed_paths: Vec::new(),
            checks: vec![check; 9],
            check_candidate: None,
            findings: Vec::new(),
            state: "confirmed".into(),
            at: 1,
        };
        let error = store.evidence(evidence).unwrap_err();
        assert!(
            matches!(
                error,
                SettingsError::Storage(interface_storage::StorageError::BoundsExceeded { .. })
            ),
            "{error:?}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert!(store.snapshot().tasks.is_empty());
        store.seen(&[item().id]).unwrap();
        drop(store);
        let reopened = HarnessStore::open(&file).unwrap();
        assert!(reopened.snapshot().attention[0].seen);
        assert!(reopened.snapshot().tasks.is_empty());
    }
    #[test]
    fn a_drifted_file_still_closes_the_store() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        let mut store = HarnessStore::open(&file).unwrap();
        store.record(item()).unwrap();
        std::fs::write(&file, "{}").unwrap();
        assert!(store.seen(&[item().id]).is_err());
        std::fs::remove_file(&file).unwrap();
        let error = store.seen(&[item().id]).unwrap_err().to_string();
        assert!(
            error.contains("needs recovery after an uncertain write"),
            "{error}"
        );
    }
    #[test]
    fn corrupt_and_drifted_files_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        let mut store = HarnessStore::open(&file).unwrap();
        store.record(item()).unwrap();
        std::fs::write(&file, "foreign").unwrap();
        assert!(store.seen(&[item().id]).is_err());
        drop(store);
        assert!(HarnessStore::open(&file).is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "foreign");
    }
    fn numbered(index: usize, resolved: bool) -> AttentionItem {
        AttentionItem {
            id: format!("done:t:{index}"),
            at: index as u64,
            seen: resolved,
            resolved,
            ..item()
        }
    }
    fn result(id: &str, terminal: &str, stdout: usize) -> TaskEvidence {
        use interface_contracts::ipc::{
            ReviewCheckName, ReviewCheckRunResult, ReviewCheckRunStatus,
        };
        TaskEvidence {
            id: id.into(),
            objective: "Check the result".into(),
            target: AttentionTarget::Terminal {
                terminal_id: terminal.into(),
                generation: 1,
            },
            root: "/project".into(),
            baseline: "base".into(),
            candidate: "candidate".into(),
            changed_paths: Vec::new(),
            checks: (stdout > 0)
                .then(|| ReviewCheckRunResult {
                    name: ReviewCheckName::Test,
                    status: ReviewCheckRunStatus::Passed,
                    exit_code: Some(0),
                    stdout: "x".repeat(stdout),
                    stderr: String::new(),
                    duration_ms: 1,
                    output_truncated: false,
                })
                .into_iter()
                .collect(),
            check_candidate: None,
            findings: Vec::new(),
            state: "confirmed".into(),
            at: 1,
        }
    }
    /// A store already holding `state`, written the way an earlier build that
    /// kept every item would have left it.
    fn filled(state: &HarnessState) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        HarnessStore::open(&file).unwrap().record(item()).unwrap();
        let mut bytes = serde_json::to_vec_pretty(state).unwrap();
        bytes.push(b'\n');
        std::fs::write(&file, bytes).unwrap();
        (temp, file)
    }
    fn ids(attention: &[AttentionItem]) -> Vec<String> {
        attention.iter().map(|item| item.id.clone()).collect()
    }
    #[test]
    fn resolved_attention_past_the_soft_cap_is_dropped_oldest_first() {
        // Every fourth item is still unresolved; the rest were resolved.
        let full = HarnessState {
            attention: (0..ATTENTION_LIMIT)
                .map(|index| numbered(index, index % 4 != 0))
                .collect(),
            ..HarnessState::default()
        };
        let (_temp, file) = filled(&full);
        let mut store = HarnessStore::open(&file).unwrap();
        assert_eq!(
            store.snapshot().attention,
            full.attention,
            "loads unchanged"
        );
        let state = store.record(numbered(ATTENTION_LIMIT, false)).unwrap();
        assert_eq!(state.attention.len(), ATTENTION_SOFT_CAP);
        let unresolved: Vec<_> = full
            .attention
            .iter()
            .filter(|item| !item.resolved)
            .collect();
        let resolved: Vec<_> = full.attention.iter().filter(|item| item.resolved).collect();
        // All 1024 unresolved stay; of the resolved, only the newest 1023 do.
        let kept = ATTENTION_SOFT_CAP - unresolved.len() - 1;
        let mut expected: Vec<_> = full
            .attention
            .iter()
            .filter(|item| !item.resolved || resolved[resolved.len() - kept..].contains(item))
            .cloned()
            .collect();
        expected.push(numbered(ATTENTION_LIMIT, false));
        assert_eq!(ids(&state.attention), ids(&expected));
        assert_eq!(state.attention, expected);
        drop(store);
        assert_eq!(HarnessStore::open(&file).unwrap().snapshot(), state);
    }
    #[test]
    fn unresolved_attention_is_never_dropped_and_a_full_store_still_refuses() {
        let mut full = HarnessState {
            attention: (0..ATTENTION_LIMIT)
                .map(|index| numbered(index, false))
                .collect(),
            ..HarnessState::default()
        };
        full.attention[7].resolved = true;
        let (_temp, file) = filled(&full);
        let mut store = HarnessStore::open(&file).unwrap();
        // The one resolved item makes room; every unresolved one stays.
        let state = store.record(numbered(ATTENTION_LIMIT, false)).unwrap();
        assert_eq!(state.attention.len(), ATTENTION_LIMIT);
        assert!(state.attention.iter().all(|item| !item.resolved));
        assert!(!ids(&state.attention).contains(&full.attention[7].id));
        let before = std::fs::read(&file).unwrap();
        // Only unresolved work remains at the hard limit: refused as before.
        let error = store
            .record(numbered(ATTENTION_LIMIT + 1, false))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Unsupported or full harness store."),
            "{error}"
        );
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(store.snapshot(), state);
    }
    #[test]
    fn settled_evidence_past_the_soft_cap_is_dropped_oldest_first() {
        // Terminal "open" still needs the user; terminal "done" does not.
        let open = AttentionItem {
            target: AttentionTarget::Terminal {
                terminal_id: "open".into(),
                generation: 1,
            },
            ..numbered(0, false)
        };
        let full = HarnessState {
            attention: vec![open, numbered(1, true)],
            tasks: (0..TASK_LIMIT)
                .map(|index| {
                    let terminal = if index % 8 == 0 { "open" } else { "done" };
                    result(&format!("task-{index}"), terminal, 0)
                })
                .collect(),
            ..HarnessState::default()
        };
        let (_temp, file) = filled(&full);
        let mut store = HarnessStore::open(&file).unwrap();
        let state = store.evidence(result("newest", "done", 0)).unwrap();
        assert_eq!(state.tasks.len(), TASK_SOFT_CAP);
        let task_ids: Vec<_> = state.tasks.iter().map(|task| task.id.as_str()).collect();
        // The 64 unsettled results stay; the settled ones go oldest first.
        let settled_kept = TASK_SOFT_CAP - 64 - 1;
        let mut expected: Vec<String> = Vec::new();
        let mut settled_seen = 0;
        let settled_total = TASK_LIMIT - 64;
        for index in 0..TASK_LIMIT {
            if index % 8 == 0 {
                expected.push(format!("task-{index}"));
            } else {
                settled_seen += 1;
                if settled_seen > settled_total - settled_kept {
                    expected.push(format!("task-{index}"));
                }
            }
        }
        expected.push("newest".into());
        assert_eq!(task_ids, expected);
        assert_eq!(state.attention, full.attention, "attention is untouched");
        drop(store);
        assert_eq!(HarnessStore::open(&file).unwrap().snapshot(), state);
    }
    #[test]
    fn a_write_over_the_byte_cap_drops_the_oldest_settled_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("harness.v1.json");
        let mut store = HarnessStore::open(&file).unwrap();
        let open = AttentionItem {
            target: AttentionTarget::Terminal {
                terminal_id: "open".into(),
                generation: 1,
            },
            ..numbered(0, false)
        };
        store.record(open).unwrap();
        // One unsettled result and seven settled ones of 950 KiB each.
        store
            .evidence(result("pinned", "open", 950 * 1024))
            .unwrap();
        for index in 0..7 {
            store
                .evidence(result(&format!("old-{index}"), "done", 950 * 1024))
                .unwrap();
        }
        assert!(std::fs::metadata(&file).unwrap().len() > 7 * 1024 * 1024);
        // The next one would pass 8 MiB: the oldest settled results go until
        // the file is back under the soft size, and the write saves.
        let state = store
            .evidence(result("newest", "done", 950 * 1024))
            .unwrap();
        let task_ids: Vec<_> = state.tasks.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(
            task_ids,
            ["pinned", "old-3", "old-4", "old-5", "old-6", "newest"]
        );
        let written = std::fs::metadata(&file).unwrap().len();
        assert!(written <= SOFT_BYTES as u64, "{written}");
        drop(store);
        assert_eq!(HarnessStore::open(&file).unwrap().snapshot(), state);
    }
}
