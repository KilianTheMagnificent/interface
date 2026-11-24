//! One durable latest-prompt receipt per session, reserved before provider I/O.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::recovery::{
    PromptDelivery, PromptRecoveryInput, PromptRecoveryReceipt, RecoveryMode, RecoverySource,
};
use interface_storage::{JsonLimits, StorageError, sha256_hex};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    sequence: u64,
    receipts: BTreeMap<String, PromptRecoveryReceipt>,
    #[serde(default)]
    draft_paths: Vec<String>,
    #[serde(default)]
    launch_bindings: BTreeMap<String, BoundLaunch>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BoundLaunch {
    session_key: String,
    receipt: PromptRecoveryReceipt,
}
/// Receipts kept; past it a new session's receipt replaces the oldest settled one.
const RECEIPTS: usize = 256;
impl State {
    /// The oldest receipt that only offers Retry or Continue: settled
    /// (accepted or definitely unsent), not a pending launch, not the thread a
    /// saved launch points at, and not `keep`. An unknown outcome and every
    /// launch guard stay until the owner forgets them: they are the no-replay
    /// and duplicate-launch guards. Dropping a settled one only removes that
    /// old session's recovery offer; `prepare` refuses a missing receipt.
    fn oldest_settled(&self, keep: &str) -> Option<String> {
        self.receipts
            .values()
            .filter(|r| {
                r.delivery != PromptDelivery::Unknown
                    && r.key != keep
                    && !r.key.starts_with("pending-launch#")
                    && !self
                        .launch_bindings
                        .values()
                        .any(|binding| binding.session_key == r.key)
            })
            .min_by(|a, b| (a.at, &a.key).cmp(&(b.at, &b.key)))
            .map(|r| r.key.clone())
    }
    /// Evicts the oldest settled receipts until about `over` bytes are freed.
    /// Answers whether anything was evicted.
    fn make_room(&mut self, mut over: usize, keep: &str) -> bool {
        let mut changed = false;
        while over > 0 {
            let Some(receipt) = self
                .oldest_settled(keep)
                .and_then(|key| self.receipts.remove(&key))
            else {
                break;
            };
            over = over.saturating_sub(serde_json::to_vec(&receipt).map_or(0, |b| b.len()));
            changed = true;
        }
        changed
    }
}
pub struct PromptRecoveryStore {
    document: LeasedDocument,
    state: State,
}
fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}
fn prepared(
    receipt: &PromptRecoveryReceipt,
    mode: RecoveryMode,
) -> Result<PromptRecoveryInput, SettingsError> {
    if mode == RecoveryMode::Retry && receipt.delivery != PromptDelivery::Unsent {
        return Err(invalid(
            "Only a definitely-unsent prompt can be retried. Inspect the thread and use Continue for work that may have started.",
        ));
    }
    if mode == RecoveryMode::Continue && receipt.delivery == PromptDelivery::Unsent {
        return Err(invalid(
            "This prompt never reached the provider. Use Retry with its original context.",
        ));
    }
    let text = if mode == RecoveryMode::Retry {
        receipt.input.text.clone()
    } else {
        format!(
            "Continue after an interruption. The previous request may already have changed files or performed external actions. First inspect the conversation, current files and available receipts to determine what completed. Do not replay completed or ambiguous actions. If an external action's outcome cannot be established, ask me before repeating it. Continue only the remaining work toward this original goal:\n\n{}",
            receipt.input.text
        )
    };
    Ok(PromptRecoveryInput {
        text,
        context: receipt.input.context.clone(),
        source: Some(RecoverySource {
            id: receipt.id.clone(),
            mode,
        }),
    })
}
impl PromptRecoveryStore {
    /// # Errors
    /// Refuses another writer, unsafe storage or malformed receipts.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state) = LeasedDocument::open::<State>(
            path,
            ".prompt-recovery.lock",
            JsonLimits::new(16 * 1024 * 1024, 24, 100_000)?,
            "Prompt recovery storage is uncertain; restart before another send.",
        )?;
        let state = state.unwrap_or(State {
            version: 1,
            sequence: 0,
            receipts: BTreeMap::new(),
            draft_paths: Vec::new(),
            launch_bindings: BTreeMap::new(),
        });
        if state.version != 1
            || !valid_draft_paths(&state.draft_paths)
            || state.receipts.len() > RECEIPTS
            || state.launch_bindings.len()
                + state
                    .receipts
                    .keys()
                    .filter(|key| key.starts_with("pending-launch#"))
                    .count()
                > 256
            || state.launch_bindings.iter().any(|(key, binding)| {
                !key.starts_with("pending-launch#")
                    || key != &binding.receipt.key
                    || binding.session_key.is_empty()
                    || binding.session_key.len() > 512
                    || binding.receipt.delivery != PromptDelivery::Accepted
                    || binding.receipt.input.validate().is_err()
                    || binding.receipt.serialized.len() > 2 * 1024 * 1024
                    || binding.receipt.id.len() != 64
            })
            || state.receipts.iter().any(|(key, r)| {
                key != &r.key
                    || r.input.validate().is_err()
                    || r.serialized.len() > 2 * 1024 * 1024
                    || r.id.len() != 64
            })
        {
            return Err(invalid(
                "Invalid prompt recovery store; existing receipts were retained.",
            ));
        }
        Ok(Self { document, state })
    }
    /// Commits `state`; at the byte cap the oldest settled receipts other
    /// than `keep` make room for one retry.
    fn commit(&mut self, mut state: State, keep: &str) -> Result<(), SettingsError> {
        match self.document.commit(&state) {
            // Nothing was written; making room allows one retry.
            Err(SettingsError::Storage(StorageError::BoundsExceeded {
                max_bytes,
                actual_bytes,
                ..
            })) if state.make_room(
                usize::try_from(actual_bytes)
                    .unwrap_or(usize::MAX)
                    .saturating_sub(max_bytes),
                keep,
            ) =>
            {
                self.document.commit(&state)?;
            }
            result => result?,
        }
        self.state = state;
        Ok(())
    }
    #[must_use]
    pub fn read(&self, key: &str) -> Option<PromptRecoveryReceipt> {
        self.state
            .receipts
            .get(key)
            .or_else(|| self.state.launch_bindings.get(key).map(|b| &b.receipt))
            .cloned()
    }
    /// # Errors
    /// Refuses stale receipts and retry of work that may already have run.
    pub fn prepare(
        &self,
        key: &str,
        id: &str,
        mode: RecoveryMode,
    ) -> Result<PromptRecoveryInput, SettingsError> {
        let receipt = self
            .state
            .receipts
            .get(key)
            .filter(|r| r.id == id)
            .ok_or_else(|| {
                invalid("That recovery receipt is no longer current. Refresh the thread.")
            })?;
        prepared(receipt, mode)
    }
    /// # Errors
    /// Reserves an unknown outcome before I/O; later sends cannot replay a consumed receipt.
    pub fn reserve(
        &mut self,
        key: &str,
        input: PromptRecoveryInput,
        serialized: &str,
        at: u64,
    ) -> Result<String, SettingsError> {
        input.validate().map_err(|e| invalid(&e.to_string()))?;
        if key.is_empty() || key.len() > 512 || serialized.len() > 2 * 1024 * 1024 {
            return Err(invalid("Invalid prompt recovery request."));
        }
        if let Some(source) = &input.source {
            let expected = self.prepare(key, &source.id, source.mode)?;
            // Context must be reauthorized before sending. Its filtered diff bytes
            // may refresh, but its identity must not silently change.
            if input.text != expected.text
                || input.context.len() != expected.context.len()
                || input
                    .context
                    .iter()
                    .zip(&expected.context)
                    .any(|(a, b)| a.kind != b.kind || a.value != b.value)
            {
                return Err(invalid(
                    "Recovery input changed. Review the preserved prompt and attachments before sending.",
                ));
            }
        }
        let mut state = self.state.clone();
        if !state.receipts.contains_key(key) && state.receipts.len() >= RECEIPTS {
            let Some(oldest) = state.oldest_settled(key) else {
                return Err(invalid(
                    "Prompt recovery is full. Forget an old receipt before opening another recoverable thread.",
                ));
            };
            state.receipts.remove(&oldest);
        }
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or_else(|| invalid("Prompt recovery sequence exhausted."))?;
        let id = sha256_hex(format!("{key}\0{}\0{at}", state.sequence).as_bytes());
        state.receipts.insert(
            key.into(),
            PromptRecoveryReceipt {
                id: id.clone(),
                key: key.into(),
                input,
                serialized: serialized.into(),
                delivery: PromptDelivery::Unknown,
                at,
            },
        );
        self.commit(state, key)?;
        Ok(id)
    }
    /// # Errors
    /// Settles only the reserved prompt; a failed durable write stays unknown.
    pub fn settle(
        &mut self,
        key: &str,
        id: &str,
        delivery: PromptDelivery,
    ) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        let row = state
            .receipts
            .get_mut(key)
            .filter(|r| r.id == id)
            .ok_or_else(|| invalid("Prompt recovery changed during delivery."))?;
        if row.delivery != PromptDelivery::Unknown {
            return Err(invalid("Prompt receipt was already settled."));
        }
        row.delivery = delivery;
        self.commit(state, key)
    }
    /// # Errors
    /// A launch with an unresolved outcome must never be issued again blindly.
    pub fn reserve_launch(
        &mut self,
        identity: &str,
        input: PromptRecoveryInput,
        serialized: &str,
        at: u64,
    ) -> Result<(String, String), SettingsError> {
        if input.source.is_some() {
            return Err(invalid(
                "Recover an existing prompt in its original thread.",
            ));
        }
        let context: Vec<_> = input.context.iter().map(|c| (&c.kind, &c.value)).collect();
        let fingerprint = serde_json::to_vec(&(identity, &input.text, context))
            .map_err(|_| invalid("Launch recovery could not be encoded."))?;
        let key = format!("pending-launch#{}", sha256_hex(&fingerprint));
        if let Some(binding) = self.state.launch_bindings.get(&key) {
            return Err(invalid(&format!(
                "This launch already started thread {}. Open it from Saved thread launches and inspect its outcome. Forget the saved launch explicitly before intentionally creating the same work again.",
                binding.session_key
            )));
        }
        if self
            .read(&key)
            .is_some_and(|r| r.delivery != PromptDelivery::Unsent)
        {
            return Err(invalid(
                "This launch may already have started. Inspect the existing threads before continuing; Interface has retained its prompt and attachments and will not launch it again automatically.",
            ));
        }
        if self.state.launch_bindings.len()
            + self
                .state
                .receipts
                .keys()
                .filter(|key| key.starts_with("pending-launch#"))
                .count()
            >= 256
            && !self.state.receipts.contains_key(&key)
        {
            return Err(invalid(
                "Saved thread launches are full. Inspect and forget an old saved launch before creating another.",
            ));
        }
        let id = self.reserve(&key, input, serialized, at)?;
        Ok((key, id))
    }
    /// # Errors
    /// Atomically binds a receipt while retaining its duplicate-launch guard.
    pub fn bind_launch(&mut self, pending: &str, id: &str, key: &str) -> Result<(), SettingsError> {
        if key.is_empty() || key.len() > 512 || self.state.receipts.contains_key(key) {
            return Err(invalid(
                "The new thread's recovery receipt could not be bound.",
            ));
        }
        let mut state = self.state.clone();
        let mut receipt = state
            .receipts
            .remove(pending)
            .filter(|r| r.id == id && r.delivery == PromptDelivery::Unknown)
            .ok_or_else(|| invalid("The pending launch receipt changed."))?;
        receipt.delivery = PromptDelivery::Accepted;
        state.launch_bindings.insert(
            pending.into(),
            BoundLaunch {
                session_key: key.into(),
                receipt: receipt.clone(),
            },
        );
        receipt.key = key.into();
        state.receipts.insert(key.into(), receipt);
        self.commit(state, key)
    }
    #[must_use]
    pub fn retained_paths(&self) -> Vec<PathBuf> {
        self.state
            .receipts
            .values()
            .chain(
                self.state
                    .launch_bindings
                    .values()
                    .map(|binding| &binding.receipt),
            )
            .flat_map(|r| &r.input.context)
            .filter(|c| c.kind == interface_contracts::ipc::PromptContextDescriptorKind::File)
            .map(|c| PathBuf::from(&c.value))
            .chain(self.state.draft_paths.iter().map(PathBuf::from))
            .collect()
    }
    /// # Errors
    /// Bounded retention hints never grant read or provider authority.
    pub fn retain_drafts(&mut self, paths: Vec<String>) -> Result<(), SettingsError> {
        if !valid_draft_paths(&paths) {
            return Err(invalid(
                "Too many attachment paths to retain safely. Remove unused attachments before pasting another image.",
            ));
        }
        let mut state = self.state.clone();
        state.draft_paths = paths
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        self.commit(state, "")
    }
    #[must_use]
    pub fn pending(&self) -> Vec<interface_contracts::recovery::PromptRecoverySummary> {
        self.state
            .receipts
            .values()
            .filter(|r| r.key.starts_with("pending-launch#"))
            .map(|r| interface_contracts::recovery::PromptRecoverySummary {
                key: r.key.clone(),
                id: r.id.clone(),
                preview: r.input.text.chars().take(160).collect(),
                delivery: r.delivery,
                session_key: None,
            })
            .chain(self.state.launch_bindings.values().map(|binding| {
                interface_contracts::recovery::PromptRecoverySummary {
                    key: binding.receipt.key.clone(),
                    id: binding.receipt.id.clone(),
                    preview: binding.receipt.input.text.chars().take(160).collect(),
                    delivery: PromptDelivery::Accepted,
                    session_key: Some(binding.session_key.clone()),
                }
            }))
            .collect()
    }
    /// # Errors
    /// Refuses to forget a receipt that changed while it was being inspected.
    pub fn forget(&mut self, key: &str, id: &str) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        if let Some(binding) = state.launch_bindings.get(key) {
            if binding.receipt.id != id {
                return Err(invalid("The saved launch changed."));
            }
            state.launch_bindings.remove(key);
            return self.commit(state, "");
        }
        if state.receipts.get(key).is_none_or(|r| r.id != id) {
            return Err(invalid("The recovery receipt changed."));
        }
        state.receipts.remove(key);
        state
            .launch_bindings
            .retain(|_, binding| binding.session_key != key);
        self.commit(state, "")
    }
    /// # Errors
    /// Only the exact accepted launch may be acknowledged after its key reaches the owner.
    pub fn acknowledge_launch(&mut self, key: &str, id: &str) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        let Some(fingerprint) = state
            .launch_bindings
            .iter()
            .find(|(_, binding)| binding.session_key == key && binding.receipt.id == id)
            .map(|(fingerprint, _)| fingerprint.clone())
        else {
            return Err(invalid(
                "That accepted launch is not awaiting acknowledgement.",
            ));
        };
        state.launch_bindings.remove(&fingerprint);
        self.commit(state, "")
    }
}
fn valid_draft_paths(paths: &[String]) -> bool {
    paths.len() <= 2048
        && paths.iter().map(String::len).sum::<usize>() <= 1024 * 1024
        && paths
            .iter()
            .all(|path| path.len() <= 4096 && !path.contains('\0') && Path::new(path).is_absolute())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_never_replays_unknown_and_retains_original_context_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recovery.json");
        let input = PromptRecoveryInput {
            text: "Make the report".into(),
            context: vec![interface_contracts::ipc::PromptContextDescriptor {
                id: "image".into(),
                kind: interface_contracts::ipc::PromptContextDescriptorKind::File,
                label: "Sketch".into(),
                value: "/private/images/sketch.png".into(),
                detail: None,
                estimated_tokens: None,
                safe_content: None,
            }],
            source: None,
        };
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        let id = store
            .reserve("claude#a", input.clone(), "serialized prompt", 1)
            .unwrap();
        drop(store);
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        assert!(store.prepare("claude#a", &id, RecoveryMode::Retry).is_err());
        let resumed = store
            .prepare("claude#a", &id, RecoveryMode::Continue)
            .unwrap();
        assert!(
            resumed
                .text
                .contains("Do not replay completed or ambiguous actions")
        );
        assert_eq!(resumed.context, input.context);
        assert_eq!(
            store.retained_paths(),
            vec![PathBuf::from("/private/images/sketch.png")]
        );
        store
            .settle("claude#a", &id, PromptDelivery::Unsent)
            .unwrap();
        let retry = store.prepare("claude#a", &id, RecoveryMode::Retry).unwrap();
        assert_eq!(retry.text, input.text);
        let next = store
            .reserve("claude#a", retry.clone(), "retry serialized", 2)
            .unwrap();
        assert_ne!(next, id);
        assert!(store.reserve("claude#a", retry, "duplicate", 3).is_err());
    }
    #[test]
    fn initial_launch_is_durable_binds_to_thread_and_refuses_ambiguous_recreation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("launch.json");
        let input = PromptRecoveryInput {
            text: "Create the report".into(),
            context: Vec::new(),
            source: None,
        };
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        let (pending, id) = store
            .reserve_launch("claude:/project", input.clone(), "full prompt", 1)
            .unwrap();
        drop(store);
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        assert!(
            store
                .reserve_launch("claude:/project", input.clone(), "full prompt", 2)
                .is_err()
        );
        store.settle(&pending, &id, PromptDelivery::Unsent).unwrap();
        let (pending, id) = store
            .reserve_launch("claude:/project", input.clone(), "full prompt", 3)
            .unwrap();
        store.bind_launch(&pending, &id, "claude#actual").unwrap();
        assert_eq!(
            store.read(&pending).unwrap().delivery,
            PromptDelivery::Accepted
        );
        drop(store);
        let store = PromptRecoveryStore::open(&path).unwrap();
        let receipt = store.read("claude#actual").unwrap();
        assert_eq!(receipt.input, input);
        assert_eq!(receipt.serialized, "full prompt");
        assert_eq!(receipt.delivery, PromptDelivery::Accepted);
        assert!(
            store
                .prepare("claude#actual", &id, RecoveryMode::Retry)
                .is_err()
        );
        assert!(
            store
                .prepare("claude#actual", &id, RecoveryMode::Continue)
                .is_ok()
        );
    }
    #[test]
    fn accepted_launch_guard_survives_restart_later_prompts_and_requires_exact_acknowledgement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("guard.json");
        let input = PromptRecoveryInput {
            text: "Create the report".into(),
            context: Vec::new(),
            source: None,
        };
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        let (pending, id) = store
            .reserve_launch("claude:project", input.clone(), "report", 1)
            .unwrap();
        store.bind_launch(&pending, &id, "claude#actual").unwrap();
        drop(store);
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        assert!(
            store
                .reserve_launch("claude:project", input.clone(), "report", 2)
                .is_err()
        );
        assert_eq!(
            store.pending()[0].session_key.as_deref(),
            Some("claude#actual")
        );
        let next = store
            .reserve("claude#actual", input.clone(), "later prompt", 3)
            .unwrap();
        assert!(
            store
                .reserve_launch("claude:project", input.clone(), "report", 4)
                .is_err()
        );
        assert!(store.acknowledge_launch("claude#actual", &next).is_err());
        assert!(store.acknowledge_launch("claude#other", &id).is_err());
        store.acknowledge_launch("claude#actual", &id).unwrap();
        assert!(store.pending().is_empty());
        assert_eq!(store.read("claude#actual").unwrap().id, next);
        let (pending, id) = store
            .reserve_launch("claude:project", input.clone(), "report", 5)
            .unwrap();
        store.bind_launch(&pending, &id, "claude#second").unwrap();
        store.forget("claude#second", &id).unwrap();
        assert!(store.pending().is_empty());
        let (pending, id) = store
            .reserve_launch("claude:project", input.clone(), "report", 6)
            .unwrap();
        store.bind_launch(&pending, &id, "claude#third").unwrap();
        store.forget(&pending, &id).unwrap();
        assert!(
            store.read("claude#third").is_some(),
            "explicit launch cleanup preserves thread recovery"
        );
        assert!(
            store
                .reserve_launch("claude:project", input, "report", 7)
                .is_ok()
        );
    }

    #[test]
    fn older_recovery_store_defaults_launch_bindings_without_losing_receipts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.json");
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        let id = store
            .reserve(
                "claude#existing",
                PromptRecoveryInput {
                    text: "Keep me".into(),
                    context: Vec::new(),
                    source: None,
                },
                "Keep me",
                1,
            )
            .unwrap();
        drop(store);
        let mut saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        saved.as_object_mut().unwrap().remove("launchBindings");
        std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
        let store = PromptRecoveryStore::open(&path).unwrap();
        assert_eq!(store.read("claude#existing").unwrap().id, id);
        assert!(store.pending().is_empty());
    }

    fn text(text: &str) -> PromptRecoveryInput {
        PromptRecoveryInput {
            text: text.into(),
            context: Vec::new(),
            source: None,
        }
    }

    fn receipt(
        key: &str,
        at: u64,
        delivery: PromptDelivery,
        serialized: &str,
    ) -> PromptRecoveryReceipt {
        PromptRecoveryReceipt {
            id: sha256_hex(key.as_bytes()),
            key: key.into(),
            input: text("Earlier prompt"),
            serialized: serialized.into(),
            delivery,
            at,
        }
    }

    /// A store already holding `receipts`, written the way earlier sends left it.
    fn store_holding(path: &Path, receipts: Vec<PromptRecoveryReceipt>) -> PromptRecoveryStore {
        let mut store = PromptRecoveryStore::open(path).unwrap();
        let mut state = store.state.clone();
        state.receipts = receipts.into_iter().map(|r| (r.key.clone(), r)).collect();
        store.commit(state, "").unwrap();
        drop(store);
        PromptRecoveryStore::open(path).unwrap()
    }

    #[test]
    fn a_new_session_past_the_cap_replaces_the_oldest_settled_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("full.json");
        // Key order and age run opposite ways: `claude#255` is the oldest.
        let mut store = store_holding(
            &path,
            (0..256u64)
                .map(|i| {
                    let delivery = if i % 2 == 0 {
                        PromptDelivery::Accepted
                    } else {
                        PromptDelivery::Unsent
                    };
                    receipt(&format!("claude#{i:03}"), 1_000 - i, delivery, "earlier")
                })
                .collect(),
        );
        let id = store
            .reserve("claude#new", text("Next chat"), "Next chat", 2_000)
            .unwrap();
        assert!(store.read("claude#255").is_none());
        assert!(store.read("claude#254").is_some());
        assert_eq!(store.read("claude#new").unwrap().id, id);
        store
            .settle("claude#new", &id, PromptDelivery::Accepted)
            .unwrap();
        store
            .reserve("claude#next", text("Another"), "Another", 2_001)
            .unwrap();
        assert!(store.read("claude#254").is_none());
        drop(store);
        let store = PromptRecoveryStore::open(&path).unwrap();
        assert_eq!(store.state.receipts.len(), 256);
        assert!(store.read("claude#new").is_some());
        assert!(store.read("claude#next").is_some());
    }

    #[test]
    fn unknown_outcomes_and_launch_guards_are_never_evicted() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("guards.json");
        let mut store = store_holding(
            &path,
            (0..256u64)
                .map(|i| receipt(&format!("claude#{i:03}"), i, PromptDelivery::Unknown, "x"))
                .collect(),
        );
        let before = std::fs::read(&path).unwrap();
        assert!(
            store
                .reserve("claude#new", text("Next chat"), "Next chat", 1_000)
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(store.state.receipts.len(), 256);

        // The two oldest settled receipts are launch guards: a pending launch
        // that never left, and the thread a saved launch points at.
        drop(store);
        let mut store = store_holding(&path, Vec::new());
        let (pending, id) = store
            .reserve_launch("claude:/project", text("Launch"), "Launch", 1)
            .unwrap();
        store.bind_launch(&pending, &id, "claude#bound").unwrap();
        let (unsent_launch, id) = store
            .reserve_launch("claude:/other", text("Other launch"), "Other launch", 2)
            .unwrap();
        store
            .settle(&unsent_launch, &id, PromptDelivery::Unsent)
            .unwrap();
        let mut state = store.state.clone();
        for i in 0..254u64 {
            let r = receipt(
                &format!("claude#{i:03}"),
                10 + i,
                PromptDelivery::Unknown,
                "x",
            );
            state.receipts.insert(r.key.clone(), r);
        }
        store.commit(state, "").unwrap();
        assert_eq!(store.state.receipts.len(), 256);
        assert!(
            store
                .reserve("claude#new", text("Next chat"), "Next chat", 1_000)
                .is_err()
        );
        let unknown = store.read("claude#100").unwrap();
        store
            .settle("claude#100", &unknown.id, PromptDelivery::Accepted)
            .unwrap();
        store
            .reserve("claude#new", text("Next chat"), "Next chat", 1_000)
            .unwrap();
        assert!(store.read("claude#100").is_none());
        assert_eq!(
            store.read("claude#bound").unwrap().delivery,
            PromptDelivery::Accepted
        );
        assert_eq!(
            store.read(&unsent_launch).unwrap().delivery,
            PromptDelivery::Unsent
        );
        assert!(
            store
                .pending()
                .iter()
                .any(|launch| launch.session_key.as_deref() == Some("claude#bound"))
        );
        assert!(
            store
                .reserve_launch("claude:/project", text("Launch"), "Launch", 1_001)
                .is_err()
        );
    }

    #[test]
    fn a_receipt_over_the_byte_cap_evicts_settled_receipts_and_retries_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bytes.json");
        // Seven receipts of 2 MiB fit the 16 MiB file; an eighth does not.
        let large = "x".repeat(2 * 1024 * 1024);
        let mut receipts: Vec<_> = (0..6u64)
            .map(|i| {
                receipt(
                    &format!("claude#{i}"),
                    10 + i,
                    PromptDelivery::Accepted,
                    &large,
                )
            })
            .collect();
        receipts.push(receipt(
            "claude#unknown",
            1,
            PromptDelivery::Unknown,
            &large,
        ));
        let mut store = store_holding(&path, receipts);
        store
            .reserve("claude#new", text("Big"), &large, 100)
            .unwrap();
        assert!(store.read("claude#unknown").is_some());
        assert!(store.read("claude#0").is_none());
        assert!(store.read("claude#1").is_some());
        drop(store);

        let all_unknown = (0..7u64)
            .map(|i| receipt(&format!("claude#{i}"), i, PromptDelivery::Unknown, &large))
            .collect();
        let mut store = store_holding(&path, all_unknown);
        let error = store
            .reserve("claude#new", text("Big"), &large, 100)
            .unwrap_err();
        assert!(
            matches!(
                error,
                SettingsError::Storage(StorageError::BoundsExceeded { .. })
            ),
            "{error:?}"
        );
        assert_eq!(store.state.receipts.len(), 7);
    }

    #[test]
    fn queued_image_pins_and_orphaned_launch_inventory_survive_restart_and_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("retention.json");
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        store
            .retain_drafts(vec!["/images/queued.png".into()])
            .unwrap();
        let input = PromptRecoveryInput {
            text: "Original goal".into(),
            context: Vec::new(),
            source: None,
        };
        let (key, id) = store
            .reserve_launch("claude:/project", input, "Original goal", 1)
            .unwrap();
        drop(store);
        let mut store = PromptRecoveryStore::open(&path).unwrap();
        assert_eq!(
            store.retained_paths(),
            vec![PathBuf::from("/images/queued.png")]
        );
        assert_eq!(store.pending()[0].key, key);
        assert_eq!(store.pending()[0].preview, "Original goal");
        assert_eq!(store.pending()[0].delivery, PromptDelivery::Unknown);
        assert!(store.retain_drafts(vec!["/too-many".into(); 2049]).is_err());
        assert_eq!(store.retained_paths().len(), 1);
        store.forget(&key, &id).unwrap();
        assert!(store.pending().is_empty());
        store.retain_drafts(Vec::new()).unwrap();
        assert!(store.retained_paths().is_empty());
    }
}
