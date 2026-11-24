//! Versioned collaboration wire contract and pure state transitions.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One validation rule: `invalid_if!(failed, path, rule)` returns the
/// `Invalid {path}: {rule}.` contract error when `failed` holds, as the
/// oracle's `if (failed) invalid(path, rule)` does. Rules run in statement
/// order, so the first failing one is the message. A macro rather than a
/// function so the path (often a `format_args!`) is built only on failure.
macro_rules! invalid_if {
    ($failed:expr, $path:expr, $rule:expr) => {
        if $failed {
            return Err(ContractError::invalid($path, $rule));
        }
    };
}

mod normalize;
mod receipt;
mod transition;
mod types;
mod validate;

pub use normalize::*;
pub use receipt::*;
pub use transition::*;
pub use types::*;
pub use validate::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum CollaborationEngineId {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "acp:grok")]
    AcpGrok,
}

impl CollaborationEngineId {
    /// Every collaboration engine, in the order tool schemas list targets.
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::AcpGrok];

    /// Parses the wire spelling [`Self::as_str`] prints; anything else is not
    /// a collaboration engine.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|engine| engine.as_str() == value)
    }

    /// Splits a `<engine>#<provider id>` session key at its engine's
    /// [`Self::session_prefix`]. The provider id is not validated here.
    #[must_use]
    pub fn split_session_key(key: &str) -> Option<(Self, &str)> {
        Self::ALL.into_iter().find_map(|engine| {
            key.strip_prefix(engine.session_prefix())
                .map(|provider_id| (engine, provider_id))
        })
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::AcpGrok => "acp:grok",
        }
    }

    #[must_use]
    pub const fn session_prefix(self) -> &'static str {
        match self {
            Self::Claude => "claude#",
            Self::Codex => "codex#",
            Self::AcpGrok => "acp:grok#",
        }
    }
}

impl fmt::Display for CollaborationEngineId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn effect_successors_are_legal_and_reserve_the_next_states_effect() {
        for &state in COLLABORATION_STATES {
            for mode in [CollaborationMode::Consult, CollaborationMode::Handoff] {
                let Some(kind) = maintenance_effect_for_state(state, mode) else {
                    continue;
                };
                let (next, reserved) = effect_successor(kind, mode);
                assert!(
                    legal_collaboration_transitions(state, mode).contains(&next),
                    "{state} -> {next} after {kind:?} in {mode:?}"
                );
                assert_eq!(
                    reserved,
                    maintenance_effect_for_state(next, mode),
                    "{next} must reserve its own effect in {mode:?}"
                );
                assert!(reserved.is_none_or(|kind| allowed_effect(mode, kind)));
            }
        }
    }

    #[test]
    fn sha256_is_the_lowercase_hex_digest_the_local_copy_printed() {
        for value in ["", "abc", "collaboration objective\n", "é😀"] {
            assert_eq!(
                sha256(value),
                format!("{:x}", Sha256::digest(value.as_bytes()))
            );
        }
        assert_eq!(
            sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn engine_ids_parse_exactly_their_wire_spellings() {
        for engine in CollaborationEngineId::ALL {
            assert_eq!(
                CollaborationEngineId::from_wire(engine.as_str()),
                Some(engine)
            );
            assert_eq!(
                serde_json::to_value(engine).unwrap(),
                Value::String(engine.as_str().to_owned())
            );
        }
        for other in ["", "Claude", "acp:qwen", "acp:grok ", "grok"] {
            assert_eq!(CollaborationEngineId::from_wire(other), None, "{other}");
        }
    }

    #[test]
    fn session_keys_split_at_their_engine_prefix() {
        for engine in CollaborationEngineId::ALL {
            let key = format!("{}thread#1", engine.session_prefix());
            assert_eq!(
                CollaborationEngineId::split_session_key(&key),
                Some((engine, "thread#1"))
            );
            assert_eq!(engine_from_session_key(&key).unwrap(), engine);
            assert_eq!(provider_id_from_session_key(&key).unwrap(), "thread#1");
        }
        for other in [
            "",
            "claude",
            "Claude#x",
            "acp:qwen#x",
            "codex:x",
            "#x",
            "grok#x",
        ] {
            assert_eq!(
                CollaborationEngineId::split_session_key(other),
                None,
                "{other}"
            );
            assert_eq!(
                engine_from_session_key(other).unwrap_err().to_string(),
                "Invalid intent.sourceSessionKey: does not use a supported engine prefix."
            );
            assert_eq!(
                provider_id_from_session_key(other).unwrap_err().to_string(),
                "Invalid record.source.sessionKey: does not use a supported engine prefix."
            );
        }
    }

    #[test]
    fn timestamps_accept_only_the_canonical_form() {
        let parsed = timestamp_millis("2026-09-27T05:56:12.123Z", "at").unwrap();
        assert_eq!(
            interface_contracts::canonical_timestamp(parsed),
            "2026-09-27T05:56:12.123Z"
        );
        for value in [
            "2026-09-27T05:56:12.123456789+00:00",
            "2026-09-27T05:56:12Z",
            "2026-09-27T05:56:12.123+00:00",
        ] {
            assert_eq!(
                timestamp_millis(value, "at").unwrap_err().to_string(),
                "Invalid at: must be a canonical UTC timestamp."
            );
        }
    }

    #[test]
    // serde_json's Map is a BTreeMap by default, so the explicit key sort is
    // insurance against a preserve_order build, not the thing under test here:
    // this pins the exact bytes the digests are taken over.
    fn canonical_json_pins_the_canonical_byte_form() {
        let value = serde_json::json!({
            "b": 1,
            "a": {
                "d": [3, {"z": null, "y": "x"}, []],
                "c": true,
                "": "empty key"
            },
            "A": "case sensitive",
            "n": 1.5
        });
        assert_eq!(
            canonical_json(&value),
            r#"{"A":"case sensitive","a":{"":"empty key","c":true,"d":[3,{"y":"x","z":null},[]]},"b":1,"n":1.5}"#
        );
    }
}
