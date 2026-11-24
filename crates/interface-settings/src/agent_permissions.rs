//! Per-agent permission modes, stored beside the roster in
//! `agent-permissions.v1.json`. Only `auto` and `bypass` are stored; an absent
//! agent is gated. Older builds never read this file, so a rollback runs every
//! agent behind the approval gate, and `agents.v2.json` never changes shape.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::agents::AgentPermissionMode;
use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// Twice the roster bound, so entries an older build left behind never block opening.
const LIMIT: usize = 100;
const BYTES: usize = 64 * 1024;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    modes: BTreeMap<String, AgentPermissionMode>,
}

pub(crate) struct AgentPermissions {
    document: LeasedDocument,
    state: State,
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.into())
}
fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 6, 2_000)?)
}

impl AgentPermissions {
    /// # Errors
    /// Refuses unsafe, corrupt, foreign-version or already leased storage. Never resets it.
    pub(crate) fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state) = LeasedDocument::open::<State>(
            path,
            ".agent-permissions.lock",
            limits()?,
            "Agent permission storage is uncertain; restart Interface before changing it.",
        )?;
        let state = state.unwrap_or_else(|| State {
            version: 1,
            modes: BTreeMap::new(),
        });
        if state.version != 1
            || state.modes.len() > LIMIT
            || state
                .modes
                .keys()
                .any(|id| id.is_empty() || id.chars().count() > 256 || id.contains('\0'))
        {
            return Err(invalid("Invalid agent permission modes."));
        }
        Ok(Self { document, state })
    }

    /// The agent's mode; an agent with no entry is gated.
    pub(crate) fn mode(&self, agent_id: &str) -> AgentPermissionMode {
        self.state.modes.get(agent_id).copied().unwrap_or_default()
    }

    /// The non-gated modes of the agents in `known`.
    pub(crate) fn modes(&self, known: &[String]) -> BTreeMap<String, AgentPermissionMode> {
        self.state
            .modes
            .iter()
            .filter(|(id, mode)| **mode != AgentPermissionMode::Gated && known.contains(id))
            .map(|(id, mode)| (id.clone(), *mode))
            .collect()
    }

    /// Sets one agent's mode and drops entries for agents no longer in `known`.
    /// # Errors
    /// Refuses an agent missing from `known` and failed or uncertain storage.
    pub(crate) fn save(
        &mut self,
        agent_id: &str,
        mode: AgentPermissionMode,
        known: &[String],
    ) -> Result<(), SettingsError> {
        if !known.iter().any(|id| id == agent_id) {
            return Err(invalid("That agent no longer exists."));
        }
        let mut modes = self.modes(known);
        if mode == AgentPermissionMode::Gated {
            modes.remove(agent_id);
        } else {
            modes.insert(agent_id.to_owned(), mode);
        }
        self.commit(State { version: 1, modes })
    }

    /// Removes a deleted agent's entry.
    /// # Errors
    /// Refuses failed or uncertain storage.
    pub(crate) fn forget(&mut self, agent_id: &str) -> Result<(), SettingsError> {
        let mut state = self.state.clone();
        if state.modes.remove(agent_id).is_none() {
            return Ok(());
        }
        self.commit(state)
    }

    fn commit(&mut self, state: State) -> Result<(), SettingsError> {
        self.document.ensure_writable()?;
        if state == self.state && self.document.is_written() {
            return Ok(());
        }
        self.document.commit(&state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn modes_survive_reopen_and_gated_is_never_stored() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-permissions.v1.json");
        let agents = known(&["a", "b"]);
        {
            let mut store = AgentPermissions::open(&path).unwrap();
            assert_eq!(store.mode("a"), AgentPermissionMode::Gated);
            store.save("a", AgentPermissionMode::Auto, &agents).unwrap();
            store
                .save("b", AgentPermissionMode::Bypass, &agents)
                .unwrap();
        }
        let mut store = AgentPermissions::open(&path).unwrap();
        assert_eq!(store.mode("a"), AgentPermissionMode::Auto);
        assert_eq!(store.mode("b"), AgentPermissionMode::Bypass);
        store
            .save("a", AgentPermissionMode::Gated, &agents)
            .unwrap();
        assert_eq!(store.mode("a"), AgentPermissionMode::Gated);
        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&written).unwrap(),
            serde_json::json!({"version": 1, "modes": {"b": "bypass"}})
        );
    }

    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-permissions.v1.json");
        std::fs::write(
            &path,
            r#"{"version":1,"modes":{"gone":"bypass","a":"auto"}}"#,
        )
        .unwrap();
        let mut store = AgentPermissions::open(&path).unwrap();
        store
            .save("b", AgentPermissionMode::Bypass, &known(&["a", "b"]))
            .unwrap();
        crate::golden::assert_golden("agent-permissions.saved.json", temp.path(), &path);
        assert!(temp.path().join(".agent-permissions.lock").exists());
    }

    #[test]
    fn an_unknown_agent_is_refused_and_nothing_is_written() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-permissions.v1.json");
        let mut store = AgentPermissions::open(&path).unwrap();
        assert!(
            store
                .save("ghost", AgentPermissionMode::Bypass, &known(&["a"]))
                .is_err()
        );
        assert!(!path.exists());
        assert_eq!(store.mode("ghost"), AgentPermissionMode::Gated);
    }

    #[test]
    fn corrupt_or_foreign_files_refuse_to_open_and_are_kept() {
        let temp = tempfile::tempdir().unwrap();
        for bytes in [
            "not json",
            r#"{"version":2,"modes":{}}"#,
            r#"{"version":1,"modes":{"a":"yolo"}}"#,
            r#"{"version":1,"modes":{"":"auto"}}"#,
            r#"{"version":1,"modes":{},"extra":true}"#,
        ] {
            let path = temp.path().join("agent-permissions.v1.json");
            std::fs::write(&path, bytes).unwrap();
            assert!(AgentPermissions::open(&path).is_err(), "{bytes}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn stale_ids_are_ignored_then_dropped_by_the_next_save() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-permissions.v1.json");
        std::fs::write(
            &path,
            r#"{"version":1,"modes":{"gone":"bypass","a":"auto"}}"#,
        )
        .unwrap();
        let mut store = AgentPermissions::open(&path).unwrap();
        let agents = known(&["a", "b"]);
        assert_eq!(
            store.modes(&agents),
            BTreeMap::from([("a".to_owned(), AgentPermissionMode::Auto)])
        );
        store.save("b", AgentPermissionMode::Auto, &agents).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["modes"],
            serde_json::json!({"a": "auto", "b": "auto"})
        );
    }

    #[test]
    fn forget_removes_an_agent_and_writes_only_when_needed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("agent-permissions.v1.json");
        let mut store = AgentPermissions::open(&path).unwrap();
        store.forget("a").unwrap();
        assert!(!path.exists());
        store
            .save("a", AgentPermissionMode::Bypass, &known(&["a"]))
            .unwrap();
        store.forget("a").unwrap();
        assert_eq!(store.mode("a"), AgentPermissionMode::Gated);
        // One writer at a time: release the lease before reopening.
        drop(store);
        let reopened = AgentPermissions::open(&path).unwrap();
        assert_eq!(reopened.mode("a"), AgentPermissionMode::Gated);
    }
}
