//! Private secret card (protocol 67, T3 section 12): the `iface:threadSecret`
//! request channel and the request a thread's view carries.
//!
//! An agent asks for a secret through team tools' `request_secret {name,
//! reason}`; the thread's view carries the open request
//! ([`ThreadSecretRequest`]) and the pane docks a card above its composer.
//! The owner's value crosses the wire once, in [`ThreadSecretCommand::Give`],
//! and is never echoed back: every answer is a [`ThreadSecretsView`] of
//! names. Rust keeps the value only in its owner-private file. Fixture:
//! `electron/src/fixtures/quick-wins-v67.json`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Every answer's `version`.
pub const THREAD_SECRETS_VIEW_VERSION: u8 = 1;
/// A secret's name: `[A-Za-z0-9_.-]`, at most this many characters.
pub const THREAD_SECRET_NAME_CHARS: usize = 64;
/// The reason an agent gives, in characters.
pub const THREAD_SECRET_REASON_CHARS: usize = 300;
/// A secret's value, in UTF-8 bytes.
pub const THREAD_SECRET_VALUE_BYTES: usize = 16 * 1024;
/// A secret is removed this long after it was given.
pub const THREAD_SECRET_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Whether `name` may name a secret: 1 to 64 of `A-Z a-z 0-9 _ . -`, not
/// starting with a dot (so never `.`, `..` or a hidden file).
#[must_use]
pub fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= THREAD_SECRET_NAME_CHARS
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// Whether `reason` may be shown on the card: not blank, one line of at
/// most [`THREAD_SECRET_REASON_CHARS`] characters.
#[must_use]
pub fn valid_secret_reason(reason: &str) -> bool {
    !reason.trim().is_empty()
        && reason.chars().count() <= THREAD_SECRET_REASON_CHARS
        && !reason.chars().any(char::is_control)
}

/// The open request a thread's view carries (`ThreadView.secretRequest`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSecretRequest {
    /// This request's id: Give and Decline name it.
    pub id: String,
    pub name: String,
    pub reason: String,
    /// Who asks, as the card says it ("Claude").
    pub agent: String,
}

/// `iface:threadSecret`'s commands.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadSecretCommand {
    /// The secrets Interface holds, by thread and name.
    List {},
    /// The owner gives thread `key`'s open request `id` its value. The value
    /// goes to its file and nowhere else; the answer never carries it.
    Give {
        key: String,
        id: String,
        value: String,
    },
    /// The owner declines thread `key`'s open request `id`.
    Decline { key: String, id: String },
    /// Removes the secret `name` thread `key` holds.
    Forget { key: String, name: String },
}

/// Never prints a given value, whoever formats a command.
impl fmt::Debug for ThreadSecretCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::List {} => f.write_str("List"),
            Self::Give { key, id, value } => f
                .debug_struct("Give")
                .field("key", key)
                .field("id", id)
                .field("value", &format_args!("[{} bytes withheld]", value.len()))
                .finish(),
            Self::Decline { key, id } => f
                .debug_struct("Decline")
                .field("key", key)
                .field("id", id)
                .finish(),
            Self::Forget { key, name } => f
                .debug_struct("Forget")
                .field("key", key)
                .field("name", name)
                .finish(),
        }
    }
}

/// One secret Interface holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeldSecret {
    /// The thread's key (its head's, for a switched thread).
    pub key: String,
    pub name: String,
    /// When it is removed (ms since the epoch).
    pub expires_at: u64,
}

/// Every `iface:threadSecret` answer: names only, never a value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSecretsView {
    pub version: u8,
    /// Oldest expiry first.
    pub secrets: Vec<HeldSecret>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/quick-wins-v67.json"
        ))
        .expect("fixture")
    }

    #[test]
    fn quick_wins_v67_secret_shapes_round_trip_and_refuse_extra_fields() {
        let fixture = fixture();
        assert_eq!(fixture["secretChannel"], crate::ipc::CHANNELS.thread_secret);
        for request in fixture["secretRequests"].as_array().expect("requests") {
            let command: ThreadSecretCommand =
                serde_json::from_value(request.clone()).expect("command");
            assert_eq!(&serde_json::to_value(&command).expect("encode"), request);
            let mut extra = request.clone();
            extra["echo"] = json!(true);
            assert!(serde_json::from_value::<ThreadSecretCommand>(extra).is_err());
        }
        let view: ThreadSecretsView =
            serde_json::from_value(fixture["secretsView"].clone()).expect("view");
        assert_eq!(
            serde_json::to_value(&view).expect("encode"),
            fixture["secretsView"]
        );
        assert_eq!(view.version, THREAD_SECRETS_VIEW_VERSION);
        let request: ThreadSecretRequest =
            serde_json::from_value(fixture["view"]["threads"][0]["secretRequest"].clone())
                .expect("request");
        assert_eq!(request.name, "STRIPE_TEST_KEY");
    }

    #[test]
    fn a_given_value_is_never_formatted() {
        let give = ThreadSecretCommand::Give {
            key: "claude#a".into(),
            id: "secret-1".into(),
            value: "sk_test_hunter2".into(),
        };
        let shown = format!("{give:?} {give:#?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("15 bytes withheld"));
    }

    #[test]
    fn names_and_reasons_keep_to_their_rules() {
        for good in ["STRIPE_KEY", "a", "db.password", "x-1", &"n".repeat(64)] {
            assert!(valid_secret_name(good), "{good}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".env",
            "a/b",
            "a b",
            "ключ",
            &"n".repeat(65),
            "a\0",
        ] {
            assert!(!valid_secret_name(bad), "{bad:?}");
        }
        assert!(valid_secret_reason("To run the payment tests."));
        assert!(!valid_secret_reason("  "));
        assert!(!valid_secret_reason("two\nlines"));
        assert!(!valid_secret_reason(&"r".repeat(301)));
    }
}
