//! Takes the owner's identity out of a recording before it is written.
//!
//! Every string in a transcript goes through [`Scrubber::text`]: credential
//! shapes (`interface_review::thread_git::ship::redact_credentials`), then the
//! home directory, the host name, the user name, email addresses, temp
//! directories and `org-` ids, then UUIDs and long token-shaped runs, which
//! become placeholders numbered by first appearance across the whole
//! recording, so a session id still matches itself in every frame and every
//! launch and two different ids stay different. Values under account,
//! organisation, device, user and email keys are replaced
//! whole by fixed placeholders, and Claude's `/context` grid (unread by
//! Interface, about 26 KB a frame) is emptied.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;
use serde_json::{Map, Value};

/// What the owner's home directory becomes.
pub const OWNER_HOME: &str = "/Users/owner";
/// What the owner's user name becomes.
pub const OWNER_NAME: &str = "owner";
/// What every email address becomes.
pub const OWNER_EMAIL: &str = "owner@example.com";
/// What the machine's host name becomes.
pub const OWNER_HOST: &str = "owner-mac";

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}")
        .expect("email pattern")
});
static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
        .expect("uuid pattern")
});
/// macOS per-user temp directories: `/var/folders/xx/yyyy/T/`.
static TEMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:/private)?/var/folders/[A-Za-z0-9_+-]+/[A-Za-z0-9_+-]+/T/")
        .expect("temp pattern")
});
/// The same directories as Claude names a folder after its path.
static DASHED_TEMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:-private)?-var-folders-[A-Za-z0-9_+]+-[A-Za-z0-9_+]+-T-")
        .expect("dashed temp pattern")
});
static ORG_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\borg-[A-Za-z0-9]{8,}").expect("org id pattern"));
/// A run with no spaces or path separators, at least 40 characters, of
/// letters and digits (base64 and URL-safe alphabets included): API keys,
/// bearer and refresh tokens, encrypted reasoning blobs.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_\-+]{40,}={0,2}|[A-Za-z0-9+/]{80,}={0,2}").expect("token pattern")
});

/// How long an unbroken stretch makes a long run a token.
const LONG_STRETCH: usize = 24;

/// Object keys whose string value is replaced whole, compared with case and
/// `_`/`-` ignored, and the placeholder each gets.
const IDENTITY_KEYS: [(&str, &str); 14] = [
    ("accountid", "account-placeholder"),
    ("accountuuid", "account-placeholder"),
    ("organizationid", "organization-placeholder"),
    ("organizationuuid", "organization-placeholder"),
    ("orgid", "organization-placeholder"),
    ("organizationname", "Owner Organization"),
    ("organization", "Owner Organization"),
    ("deviceid", "device-placeholder"),
    ("userid", "user-placeholder"),
    ("useruuid", "user-placeholder"),
    ("installationid", "installation-placeholder"),
    ("machineid", "machine-placeholder"),
    ("email", OWNER_EMAIL),
    ("emailaddress", OWNER_EMAIL),
];

/// Bulky display data the provider sends and Interface never reads, emptied
/// (kept as an empty array, so the frame keeps its shape) to keep fixtures
/// small: Claude's `/context` grid, about 26 KB in every context-usage
/// answer.
const UNREAD_DISPLAY_KEYS: [&str; 1] = ["gridRows"];

/// Whose identity is taken out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    /// The home directory, e.g. `/Users/jane`.
    pub home: String,
    /// The login name, e.g. `jane`.
    pub user: String,
    /// The machine's host name, e.g. `Janes-MacBook.local` (its first label
    /// is taken out too).
    pub host: String,
}

impl Identity {
    /// The identity of the user running this process (`HOME`, `USER`, or the
    /// home directory's last component).
    #[must_use]
    pub fn current() -> Self {
        let home = std::env::var("HOME").unwrap_or_default();
        let user = std::env::var("USER")
            .ok()
            .filter(|user| !user.is_empty())
            .or_else(|| {
                std::path::Path::new(&home)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_default();
        let host = std::process::Command::new("/bin/hostname")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        Self { home, user, host }
    }
}

/// A scrubber for one recording: its UUID placeholders stay consistent
/// across every value it is given.
#[derive(Debug)]
pub struct Scrubber {
    identity: Identity,
    /// The user name, any case.
    user: Option<Regex>,
    /// The host name and its first label, any case.
    host: Option<Regex>,
    uuids: BTreeMap<String, String>,
    tokens: BTreeMap<String, String>,
}

impl Scrubber {
    /// # Panics
    /// Never: the user name is escaped before it becomes a pattern.
    #[must_use]
    pub fn new(identity: Identity) -> Self {
        let user = (identity.user.len() > 2).then(|| {
            Regex::new(&format!("(?i){}", regex::escape(&identity.user))).expect("escaped pattern")
        });
        let short_host = identity.host.split('.').next().unwrap_or_default();
        // A first label as short as `mac` would also hit ordinary words.
        let host = (short_host.len() >= 5).then(|| {
            Regex::new(&format!(
                r"(?i)\b(?:{}|{})\b",
                regex::escape(&identity.host),
                regex::escape(short_host)
            ))
            .expect("escaped pattern")
        });
        Self {
            identity,
            user,
            host,
            uuids: BTreeMap::new(),
            tokens: BTreeMap::new(),
        }
    }

    /// One string with the owner's identity taken out.
    pub fn text(&mut self, text: &str) -> String {
        let mut text = interface_review::thread_git::ship::redact_credentials(text);
        // Claude names a project folder after its path with `/` as `-`.
        let home = self.identity.home.trim_end_matches('/');
        if home.len() > 1 {
            text = text.replace(home, OWNER_HOME);
            let dashed = home.replace(['/', '.'], "-");
            text = text.replace(&dashed, &OWNER_HOME.replace('/', "-"));
        }
        if let Some(host) = &self.host {
            text = host.replace_all(&text, OWNER_HOST).into_owned();
        }
        if let Some(user) = &self.user {
            text = user.replace_all(&text, OWNER_NAME).into_owned();
        }
        text = EMAIL.replace_all(&text, OWNER_EMAIL).into_owned();
        text = TEMP.replace_all(&text, "/tmp/").into_owned();
        text = DASHED_TEMP.replace_all(&text, "-tmp-").into_owned();
        text = ORG_ID.replace_all(&text, "org-placeholder").into_owned();
        text = UUID
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                let found = captures[0].to_ascii_lowercase();
                let next = self.uuids.len() + 1;
                self.uuids
                    .entry(found)
                    .or_insert_with(|| format!("00000000-0000-4000-8000-{next:012}"))
                    .clone()
            })
            .into_owned();
        TOKEN
            .replace_all(&text, |captures: &regex::Captures<'_>| {
                let found = &captures[0];
                if token_shaped(found) {
                    let next = self.tokens.len() + 1;
                    self.tokens
                        .entry(found.to_owned())
                        .or_insert_with(|| format!("[redacted-token-{next}]"))
                        .clone()
                } else {
                    found.to_owned()
                }
            })
            .into_owned()
    }

    /// A JSON value with every string (and object key) scrubbed, and the
    /// values of identity keys replaced whole.
    pub fn value(&mut self, value: &Value) -> Value {
        match value {
            Value::Object(object) => {
                let mut scrubbed = Map::new();
                for (key, value) in object {
                    if UNREAD_DISPLAY_KEYS.contains(&key.as_str()) && value.is_array() {
                        scrubbed.insert(key.clone(), Value::Array(Vec::new()));
                        continue;
                    }
                    let replaced = match (identity_placeholder(key), value) {
                        (Some(placeholder), Value::String(text)) if !text.is_empty() => {
                            Value::String(placeholder.to_owned())
                        }
                        _ => self.value(value),
                    };
                    scrubbed.insert(self.text(key), replaced);
                }
                Value::Object(scrubbed)
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.value(item)).collect())
            }
            Value::String(text) => Value::String(self.text(text)),
            other => other.clone(),
        }
    }
}

fn identity_placeholder(key: &str) -> Option<&'static str> {
    let compact: String = key
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .collect::<String>()
        .to_ascii_lowercase();
    IDENTITY_KEYS
        .iter()
        .find(|(name, _)| *name == compact)
        .map(|(_, placeholder)| *placeholder)
}

/// Whether a long run is a token rather than a dashed name: it mixes
/// letters and digits and has one long unbroken stretch (a project folder
/// named after its path has only short ones).
fn token_shaped(run: &str) -> bool {
    run.chars().any(|c| c.is_ascii_digit())
        && run.chars().any(|c| c.is_ascii_alphabetic())
        && run
            .split(['-', '_'])
            .any(|stretch| stretch.len() >= LONG_STRETCH)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn scrubber() -> Scrubber {
        Scrubber::new(Identity {
            home: "/Users/janedoe".into(),
            user: "janedoe".into(),
            host: "Janes-Studio.local".into(),
        })
    }

    #[test]
    fn hostile_samples_lose_the_owner_identity() {
        let mut scrub = scrubber();
        let samples = [
            "/Users/janedoe/.claude/projects/-Users-janedoe-Developer-x/abc.jsonl",
            "Logged in as JaneDoe (jane.doe+work@gmail.com)",
            "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789",
            "key sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345",
            "token=ghp_0123456789abcdefghijABCDEFGHIJ",
            "cwd /private/var/folders/x7/k2jd93_d0000gn/T/.tmpAbC123/work",
            "/private/tmp/claude-501/-private-var-folders-x7-k2jd93_d0000gn-T--tmpAbC123-work/s",
            "organization org-AbCdEf0123456789 billing",
            "refresh eyJhbGciOiJIUzI1NiJ9eyJzdWIiOiIxMjM0NTY3ODkwIn0abcdEFGH12345",
            "serverName janes-studio.local, prompt janes-studio",
        ];
        for sample in samples {
            let scrubbed = scrub.text(sample);
            for leak in [
                "janedoe",
                "JaneDoe",
                "gmail",
                "sk-ant",
                "ghp_",
                "var/folders",
                "var-folders",
                "k2jd93",
                "AbCdEf0123",
                "eyJhbGci",
                "abcdefghijklmnopqrstuvwxyz0123456789",
                "janes-studio",
            ] {
                assert!(
                    !scrubbed.contains(leak),
                    "{sample:?} kept {leak:?}: {scrubbed:?}"
                );
            }
        }
        assert_eq!(
            scrub.text("/Users/janedoe/.claude/projects/-Users-janedoe-Developer-x"),
            "/Users/owner/.claude/projects/-Users-owner-Developer-x"
        );
        assert_eq!(
            scrub.text("mail jane@corp.example.org"),
            "mail owner@example.com"
        );
        assert_eq!(scrub.text("host Janes-Studio.local"), "host owner-mac");
    }

    #[test]
    fn distinct_tokens_stay_distinct_and_one_token_keeps_its_placeholder() {
        let mut scrub = scrubber();
        let first = "msg_0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f6071";
        let second = "rs_0a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f6072";
        assert_eq!(scrub.text(first), "[redacted-token-1]");
        assert_eq!(scrub.text(second), "[redacted-token-2]");
        assert_eq!(
            scrub.text(&format!("item {first}")),
            "item [redacted-token-1]"
        );
    }

    #[test]
    fn uuids_become_stable_placeholders_in_first_seen_order() {
        let mut scrub = scrubber();
        let first = "0F3C2B1A-1111-4222-8333-444455556666";
        let second = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeffff0000";
        assert_eq!(
            scrub.text(&format!("session {first} then {second}")),
            "session 00000000-0000-4000-8000-000000000001 then 00000000-0000-4000-8000-000000000002"
        );
        assert_eq!(
            scrub.text(&format!("{}.jsonl", first.to_ascii_lowercase())),
            "00000000-0000-4000-8000-000000000001.jsonl",
            "the same id, whatever its case, keeps its placeholder"
        );
    }

    #[test]
    fn identity_keys_are_replaced_whole_and_frames_keep_their_shape() {
        let mut scrub = scrubber();
        let frame = json!({
            "type": "control_response",
            "response": {"account": {
                "email": "someone@else.test",
                "organization": "Jane's Workshop",
                "accountUuid": "not-a-uuid-but-an-id",
                "subscriptionType": "max"
            }, "session_id": "0f3c2b1a-1111-4222-8333-444455556666", "count": 3, "empty": "",
               "gridRows": [["x", "y"]]}
        });
        let scrubbed = scrub.value(&frame);
        assert_eq!(
            scrubbed,
            json!({
                "type": "control_response",
                "response": {"account": {
                    "email": OWNER_EMAIL,
                    "organization": "Owner Organization",
                    "accountUuid": "account-placeholder",
                    "subscriptionType": "max"
                }, "session_id": "00000000-0000-4000-8000-000000000001", "count": 3, "empty": "",
                   "gridRows": []}
            })
        );
    }

    #[test]
    fn ordinary_text_and_ids_are_left_alone() {
        let mut scrub = scrubber();
        for text in [
            "Reply with the single word: ready",
            "msg_01XyZ",
            "toolu_01ABCdef",
            "/tmp/interface-replay/work/ready.txt",
            "claude-haiku-4-5-20251001",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "-Users-owner-Developer-Projects-interface-2026-11-14-replay-work",
        ] {
            assert_eq!(scrub.text(text), text);
        }
    }
}
