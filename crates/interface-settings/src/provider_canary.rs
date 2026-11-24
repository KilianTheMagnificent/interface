//! What the host last saw of each provider CLI, kept in
//! `provider-canary.v1.json` (protocol 51, review F2, 2026-10-15): the
//! `--version` line, the canary expectations it was checked against, when,
//! and the last change the canary found. Older builds never read this file.
//! A file that does not decode is quarantined: the cost is one more check of
//! every CLI, never a host that cannot start.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};

use crate::{SettingsError, leased::LeasedDocument};

const TOOLS: usize = 32;
const BYTES: usize = 256 * 1024;
const TEXT_CHARS: usize = 160;
const FINDINGS: usize = 32;
const FINDING_CHARS: usize = 1_000;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    tools: BTreeMap<String, ProviderCanarySeen>,
}

/// One CLI as the canary last checked it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCanarySeen {
    /// The first line its `--version` printed.
    pub version: String,
    /// The canary expectations it was checked against.
    pub expectations: String,
    /// Unix milliseconds.
    pub checked_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<ProviderCanaryChange>,
}

/// The last change the canary found in one CLI.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCanaryChange {
    /// The version seen before, when the version is what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub to: String,
    /// Unix milliseconds.
    pub at: u64,
    /// What no longer matches; empty when everything still does.
    pub findings: Vec<String>,
}

fn limits() -> Result<JsonLimits, SettingsError> {
    Ok(JsonLimits::new(BYTES, 6, 64 * TOOLS + 16)?)
}

fn short(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= TEXT_CHARS && !text.contains('\0')
}

fn valid_seen(seen: &ProviderCanarySeen) -> bool {
    short(&seen.version)
        && short(&seen.expectations)
        && seen.change.as_ref().is_none_or(|change| {
            change.from.as_deref().is_none_or(short)
                && short(&change.to)
                && change.findings.len() <= FINDINGS
                && change
                    .findings
                    .iter()
                    .all(|finding| !finding.is_empty() && finding.chars().count() <= FINDING_CHARS)
        })
}

fn valid_state(state: &State) -> bool {
    state.version == 1
        && state.tools.len() <= TOOLS
        && state
            .tools
            .iter()
            .all(|(tool, seen)| short(tool) && valid_seen(seen))
}

/// The provider CLIs the host last checked, by launch profile id.
pub struct ProviderCanaryStore {
    document: LeasedDocument,
    state: State,
    quarantined: Option<PathBuf>,
}

impl ProviderCanaryStore {
    /// # Errors
    /// Refuses unsafe or already leased storage. A file that does not decode
    /// is quarantined and the store starts empty.
    pub fn open(path: &Path) -> Result<Self, SettingsError> {
        let (document, state, quarantined) = LeasedDocument::open_or_quarantine::<State>(
            path,
            ".provider-canary.lock",
            limits()?,
            "The provider CLI check could not confirm its last write; restart Interface.",
            valid_state,
        )?;
        Ok(Self {
            document,
            state: state.unwrap_or_else(|| State {
                version: 1,
                tools: BTreeMap::new(),
            }),
            quarantined,
        })
    }

    /// Whether opening found an unreadable file and set it aside.
    #[must_use]
    pub const fn quarantined(&self) -> bool {
        self.quarantined.is_some()
    }

    #[must_use]
    pub fn seen(&self, tool: &str) -> Option<&ProviderCanarySeen> {
        self.state.tools.get(tool)
    }

    /// Records what the canary just saw of `tool`. Long findings are cut to
    /// fit, and only the first ones are kept.
    ///
    /// # Errors
    /// Refuses an invalid record and failed or uncertain storage.
    pub fn record(
        &mut self,
        tool: &str,
        mut seen: ProviderCanarySeen,
    ) -> Result<(), SettingsError> {
        if let Some(change) = seen.change.as_mut() {
            change.findings.truncate(FINDINGS);
            for finding in &mut change.findings {
                if finding.chars().count() > FINDING_CHARS {
                    *finding = finding.chars().take(FINDING_CHARS - 1).collect::<String>() + "…";
                }
            }
        }
        if !short(tool) || !valid_seen(&seen) {
            return Err(SettingsError::InvalidAgent(
                "Invalid provider CLI check record.".into(),
            ));
        }
        let mut state = self.state.clone();
        state.tools.insert(tool.to_owned(), seen);
        if state.tools.len() > TOOLS {
            return Err(SettingsError::InvalidAgent(
                "Too many provider CLIs to remember.".into(),
            ));
        }
        self.document.ensure_writable()?;
        if state == self.state {
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

    fn seen(version: &str, change: Option<ProviderCanaryChange>) -> ProviderCanarySeen {
        ProviderCanarySeen {
            version: version.into(),
            expectations: "0123456789abcdef".into(),
            checked_at: 1,
            change,
        }
    }

    #[test]
    fn provider_canary_store_keeps_what_it_saw_across_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider-canary.v1.json");
        let mut store = ProviderCanaryStore::open(&path).unwrap();
        assert_eq!(store.seen("claude"), None);
        let change = ProviderCanaryChange {
            from: Some("2.1.299 (Claude Code)".into()),
            to: "2.1.300 (Claude Code)".into(),
            at: 2,
            findings: vec!["x".repeat(FINDING_CHARS + 50)],
        };
        store
            .record("claude", seen("2.1.300 (Claude Code)", Some(change)))
            .unwrap();
        assert!(store.record("", seen("1", None)).is_err());
        drop(store);
        let store = ProviderCanaryStore::open(&path).unwrap();
        let kept = store.seen("claude").unwrap();
        assert_eq!(kept.version, "2.1.300 (Claude Code)");
        let finding = &kept.change.as_ref().unwrap().findings[0];
        assert_eq!(finding.chars().count(), FINDING_CHARS);
        assert!(finding.ends_with('…'));
        assert!(!store.quarantined());
    }

    #[test]
    fn provider_canary_store_sets_an_unreadable_file_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("provider-canary.v1.json");
        std::fs::write(&path, b"{\"version\":9,\"tools\":{}}").unwrap();
        let store = ProviderCanaryStore::open(&path).unwrap();
        assert!(store.quarantined());
        assert_eq!(store.seen("claude"), None);
    }
}
