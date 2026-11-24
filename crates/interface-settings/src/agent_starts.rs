//! Durable first-message intents. A reserved effect survives a crash as unresolved;
//! only a definitely-unsent outcome permits another attempt with the same input.
use crate::{SettingsError, leased::LeasedDocument};
use interface_storage::{JsonLimits, StorageError, sha256_hex};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

const LIMIT: usize = 4096;
const BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentStartAdmission {
    Admitted,
    Bound(String),
    Unresolved,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    agent_id: String,
    fingerprint: String,
    session_key: Option<String>,
    definitely_unsent: bool,
    /// When this process reserved it, counting up; 0 for an intent read from
    /// disk. Not stored: the journal keeps no age, so every intent from an
    /// earlier run counts as older than any reserved in this one.
    #[serde(skip)]
    reserved: u64,
}

impl Intent {
    /// Bound to its thread or definitely unsent. An unresolved intent is the
    /// no-replay guard and is never evicted.
    const fn settled(&self) -> bool {
        self.session_key.is_some() || self.definitely_unsent
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    intents: BTreeMap<String, Intent>,
}

impl State {
    /// Drops the oldest settled intent other than `keep` (among equally old
    /// ones a definitely-unsent one first, whose eviction changes nothing)
    /// and answers about how many bytes that freed; `None` when every other
    /// intent is unresolved.
    fn evict_oldest_settled(&mut self, keep: &str) -> Option<usize> {
        let oldest = self
            .intents
            .iter()
            .filter(|(id, intent)| *id != keep && intent.settled())
            .min_by_key(|(id, intent)| (intent.reserved, !intent.definitely_unsent, *id))
            .map(|(id, _)| id.clone())?;
        let dropped = self.intents.remove(&oldest)?;
        Some(serde_json::to_vec(&(oldest, dropped)).map_or(0, |bytes| bytes.len()))
    }
    /// Evicts the oldest settled intents until about `over` bytes are freed.
    /// Answers whether anything was evicted.
    fn make_room(&mut self, mut over: usize, keep: &str) -> bool {
        let mut changed = false;
        while over > 0 {
            let Some(freed) = self.evict_oldest_settled(keep) else {
                break;
            };
            over = over.saturating_sub(freed);
            changed = true;
        }
        changed
    }
}

pub(crate) struct AgentStarts {
    document: LeasedDocument,
    state: State,
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}
fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 12, 60_000)?)
}

impl AgentStarts {
    pub(crate) fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state) = LeasedDocument::open::<State>(
            path,
            ".agent-starts.lock",
            limits()?,
            "First-message storage is uncertain; restart before sending.",
        )?;
        let state = state.unwrap_or_else(|| State {
            version: 1,
            intents: BTreeMap::new(),
        });
        if state.version != 1
            || state.intents.len() > LIMIT
            || state.intents.iter().any(|(id, intent)| {
                id.is_empty()
                    || id.len() > 512
                    || intent.agent_id.is_empty()
                    || intent.fingerprint.len() != 64
                    || (intent.session_key.is_some() && intent.definitely_unsent)
            })
        {
            return Err(invalid("Invalid first-message intent journal."));
        }
        Ok(Self { document, state })
    }

    /// Commits `state`; at the byte cap the oldest settled intents other
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

    pub(crate) fn reserve(
        &mut self,
        request: &str,
        agent: &str,
        text: &str,
    ) -> Result<AgentStartAdmission, SettingsError> {
        if self.document.is_uncertain()
            || request.trim().is_empty()
            || request.chars().count() > 128
            || agent.is_empty()
            || text.trim().is_empty()
            || text.len() > 1024 * 1024
        {
            return Err(invalid("Invalid or unavailable first-message intent."));
        }
        let input = serde_json::to_vec(&(agent, text))
            .map_err(|_| invalid("Could not fingerprint the first message."))?;
        let fingerprint = sha256_hex(&input);
        if let Some(intent) = self.state.intents.get(request) {
            if intent.agent_id != agent || intent.fingerprint != fingerprint {
                return Err(invalid(
                    "This request id already belongs to different input.",
                ));
            }
            if let Some(key) = &intent.session_key {
                return Ok(AgentStartAdmission::Bound(key.clone()));
            }
            if !intent.definitely_unsent {
                return Ok(AgentStartAdmission::Unresolved);
            }
        }
        let mut state = self.state.clone();
        // A new request past the limit takes the place of the oldest intent
        // that is bound or definitely unsent.
        if !state.intents.contains_key(request)
            && state.intents.len() >= LIMIT
            && state.evict_oldest_settled(request).is_none()
        {
            return Err(invalid(
                "The first-message journal is full. Existing intents were retained; no message was sent.",
            ));
        }
        let reserved = state
            .intents
            .values()
            .map(|intent| intent.reserved)
            .max()
            .unwrap_or(0)
            + 1;
        state.intents.insert(
            request.into(),
            Intent {
                agent_id: agent.into(),
                fingerprint,
                session_key: None,
                definitely_unsent: false,
                reserved,
            },
        );
        self.commit(state, request)?;
        Ok(AgentStartAdmission::Admitted)
    }

    pub(crate) fn settle(
        &mut self,
        request: &str,
        session_key: Option<String>,
        definitely_unsent: bool,
    ) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        let intent = state
            .intents
            .get_mut(request)
            .ok_or_else(|| invalid("Missing first-message intent."))?;
        if intent.session_key.is_some()
            || intent.definitely_unsent
            || (session_key.is_some() && definitely_unsent)
        {
            return Err(invalid("First-message intent already settled."));
        }
        intent.session_key = session_key;
        intent.definitely_unsent = definitely_unsent;
        self.commit(state, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_starts_survive_restart_and_bind_input() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("starts.json");
        let mut journal = AgentStarts::open(&path).unwrap();
        assert_eq!(
            journal.reserve("r", "a", "work").unwrap(),
            AgentStartAdmission::Admitted
        );
        drop(journal);
        let mut journal = AgentStarts::open(&path).unwrap();
        assert_eq!(
            journal.reserve("r", "a", "work").unwrap(),
            AgentStartAdmission::Unresolved
        );
        assert!(journal.reserve("r", "a", "other").is_err());
        assert!(journal.reserve("r", "b", "work").is_err());
        journal.settle("r", None, true).unwrap();
        assert_eq!(
            journal.reserve("r", "a", "work").unwrap(),
            AgentStartAdmission::Admitted
        );
        journal.settle("r", Some("codex#1".into()), false).unwrap();
        drop(journal);
        let mut journal = AgentStarts::open(&path).unwrap();
        assert_eq!(
            journal.reserve("r", "a", "work").unwrap(),
            AgentStartAdmission::Bound("codex#1".into())
        );
    }
    fn intent(agent: &str, session_key: Option<&str>, definitely_unsent: bool) -> Intent {
        Intent {
            agent_id: agent.into(),
            fingerprint: sha256_hex(&serde_json::to_vec(&(agent, "work")).unwrap()),
            session_key: session_key.map(Into::into),
            definitely_unsent,
            reserved: 0,
        }
    }

    /// A journal an earlier run left holding `intents`.
    fn journal_holding(path: &Path, intents: Vec<(String, Intent)>) -> AgentStarts {
        let mut journal = AgentStarts::open(path).unwrap();
        let mut state = journal.state.clone();
        state.intents = intents.into_iter().collect();
        journal.commit(state, "").unwrap();
        drop(journal);
        AgentStarts::open(path).unwrap()
    }

    #[test]
    fn a_new_request_past_the_limit_replaces_the_oldest_settled_intent() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("starts.json");
        let intents = (0..LIMIT)
            .map(|i| {
                let intent = match i {
                    0 | 1 => intent("a", None, false),
                    _ if i % 2 == 0 => intent("a", Some("codex#bound"), false),
                    _ => intent("a", None, true),
                };
                (format!("req-{i:04}"), intent)
            })
            .collect();
        let mut journal = journal_holding(&path, intents);
        assert_eq!(
            journal.reserve("new", "a", "work").unwrap(),
            AgentStartAdmission::Admitted
        );
        // The journal keeps no age, so an earlier run's intents go first,
        // the definitely-unsent ones before the bound ones.
        assert!(!journal.state.intents.contains_key("req-0003"));
        for kept in ["req-0000", "req-0001", "req-0002", "req-0005"] {
            assert!(journal.state.intents.contains_key(kept), "{kept}");
        }
        journal.settle("new", None, true).unwrap();
        assert_eq!(
            journal.reserve("newer", "a", "work").unwrap(),
            AgentStartAdmission::Admitted
        );
        assert!(!journal.state.intents.contains_key("req-0005"));
        assert!(journal.state.intents.contains_key("new"));
        drop(journal);
        let mut journal = AgentStarts::open(&path).unwrap();
        assert_eq!(journal.state.intents.len(), LIMIT);
        assert_eq!(
            journal.reserve("req-0000", "a", "work").unwrap(),
            AgentStartAdmission::Unresolved
        );
        assert_eq!(
            journal.reserve("newer", "a", "work").unwrap(),
            AgentStartAdmission::Unresolved
        );
    }

    #[test]
    fn unresolved_intents_are_never_evicted() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("starts.json");
        let intents = (0..LIMIT)
            .map(|i| (format!("req-{i:04}"), intent("a", None, false)))
            .collect();
        let mut journal = journal_holding(&path, intents);
        let before = std::fs::read(&path).unwrap();
        assert!(journal.reserve("new", "a", "work").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(journal.state.intents.len(), LIMIT);
        assert_eq!(
            journal.reserve("req-0000", "a", "work").unwrap(),
            AgentStartAdmission::Unresolved
        );
    }

    #[test]
    fn an_intent_over_the_byte_cap_evicts_settled_intents_and_retries_once() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("starts.json");
        // Three intents of 1 MiB fit the 4 MiB journal; a fourth does not.
        let agent = "a".repeat(1024 * 1024);
        let mut journal = journal_holding(
            &path,
            vec![
                ("unresolved".into(), intent(&agent, None, false)),
                ("bound".into(), intent(&agent, Some("codex#1"), false)),
                ("unsent".into(), intent(&agent, None, true)),
            ],
        );
        assert_eq!(
            journal.reserve("new", &agent, "work").unwrap(),
            AgentStartAdmission::Admitted
        );
        assert!(journal.state.intents.contains_key("unresolved"));
        assert!(journal.state.intents.contains_key("bound"));
        assert!(!journal.state.intents.contains_key("unsent"));
        drop(journal);

        let mut journal = journal_holding(
            &path,
            vec![
                ("one".into(), intent(&agent, None, false)),
                ("two".into(), intent(&agent, None, false)),
                ("three".into(), intent(&agent, None, false)),
            ],
        );
        let error = journal.reserve("new", &agent, "work").unwrap_err();
        assert!(
            matches!(
                error,
                SettingsError::Storage(StorageError::BoundsExceeded { .. })
            ),
            "{error:?}"
        );
        assert_eq!(journal.state.intents.len(), 3);
    }

    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-starts.v1.json");
        let mut journal = AgentStarts::open(&path).unwrap();
        journal.reserve("r", "a", "work").unwrap();
        journal.reserve("s", "a", "other work").unwrap();
        crate::golden::assert_golden("agent-starts.reserved.json", temp.path(), &path);
        drop(journal);
        let mut journal = AgentStarts::open(&path).unwrap();
        journal.settle("r", Some("codex#1".into()), false).unwrap();
        journal.settle("s", None, true).unwrap();
        crate::golden::assert_golden("agent-starts.settled.json", temp.path(), &path);
        assert!(temp.path().join(".agent-starts.lock").exists());
    }
    #[test]
    fn agent_start_actual_process_exit_never_replays_a_reserved_effect() {
        const ROOT: &str = "IFACE_START_CRASH_FIXTURE";
        if let Some(root) = std::env::var_os(ROOT) {
            let root = std::path::PathBuf::from(root);
            let mut journal = AgentStarts::open(&root.join("starts.json")).unwrap();
            assert_eq!(
                journal.reserve("same", "agent", "synthetic work").unwrap(),
                AgentStartAdmission::Admitted
            );
            let boundary = std::env::var("IFACE_START_CRASH_BOUNDARY").unwrap();
            if boundary != "reserved" {
                std::fs::write(root.join("dispatched"), "one synthetic effect").unwrap();
            }
            if boundary == "bound" {
                journal
                    .settle("same", Some("codex#bound".into()), false)
                    .unwrap();
            }
            // Exit skips Rust destructors and proves OS lease release + durable boundary recovery.
            std::process::exit(0);
        }
        for boundary in ["reserved", "dispatched", "bound"] {
            let root = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", "agent_starts::tests::agent_start_actual_process_exit_never_replays_a_reserved_effect", "--nocapture"]).env(ROOT, root.path()).env("IFACE_START_CRASH_BOUNDARY", boundary).status().unwrap();
            assert!(status.success());
            let mut recovered = AgentStarts::open(&root.path().join("starts.json")).unwrap();
            let admission = recovered
                .reserve("same", "agent", "synthetic work")
                .unwrap();
            assert_eq!(
                admission,
                if boundary == "bound" {
                    AgentStartAdmission::Bound("codex#bound".into())
                } else {
                    AgentStartAdmission::Unresolved
                }
            );
            assert_eq!(
                root.path().join("dispatched").exists(),
                boundary != "reserved"
            );
        }
    }
}
