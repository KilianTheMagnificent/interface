//! The transcript file: one recorded provider scenario.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The only transcript version this crate reads and writes.
pub const TRANSCRIPT_VERSION: u32 = 1;

/// `crates/provider-replay/fixtures/<engine>/<scenario>.json`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Transcript {
    pub version: u32,
    /// `claude`, `codex` or `grok`.
    pub engine: String,
    /// `stream-json` (Claude), `app-server` (Codex JSON-RPC) or `acp`.
    pub protocol: String,
    /// What the recorded CLI's `--version` printed.
    pub cli_version: String,
    pub scenario: String,
    /// `YYYY-MM-DD`.
    pub recorded_at: String,
    /// Answers to auxiliary invocations, keyed by their argv joined with
    /// single spaces (`--version`, `auth status --json`).
    #[serde(default)]
    pub probes: BTreeMap<String, Probe>,
    /// Every session process the scenario launched, in launch order: each
    /// starts with a `launch` entry, then its frames in the order observed.
    pub entries: Vec<Entry>,
}

/// What one auxiliary invocation printed and how it exited.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Probe {
    pub stdout: String,
    pub code: i32,
}

/// One observed step. `out` is a frame Interface wrote to the provider's
/// stdin, `in` a line the provider wrote to stdout (a JSON frame, or a
/// string for a line that was not JSON), `exit` the process's own exit.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "dir", rename_all = "lowercase", deny_unknown_fields)]
pub enum Entry {
    /// A session process started with this argv (program excluded) in `cwd`.
    Launch {
        argv: Vec<String>,
        cwd: String,
    },
    Out {
        frame: Value,
    },
    In {
        frame: Value,
    },
    Exit {
        code: i32,
    },
}

impl Transcript {
    /// Reads and checks a transcript file.
    ///
    /// # Errors
    /// The file is unreadable, is not a version-1 transcript, or holds
    /// frames before its first launch.
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let transcript: Self = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not a transcript: {error}", path.display()))?;
        if transcript.version != TRANSCRIPT_VERSION {
            return Err(format!(
                "{} is transcript version {}; this replayer reads version {TRANSCRIPT_VERSION}",
                path.display(),
                transcript.version
            ));
        }
        if transcript
            .entries
            .first()
            .is_some_and(|entry| !matches!(entry, Entry::Launch { .. }))
        {
            return Err(format!(
                "{}: the first entry must be a launch",
                path.display()
            ));
        }
        Ok(transcript)
    }

    /// The launches, each as its argv, cwd and the entries after it up to
    /// the next launch.
    #[must_use]
    pub fn segments(&self) -> Vec<Segment<'_>> {
        let mut segments = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if let Entry::Launch { argv, cwd } = entry {
                let end = self.entries[index + 1..]
                    .iter()
                    .position(|entry| matches!(entry, Entry::Launch { .. }))
                    .map_or(self.entries.len(), |offset| index + 1 + offset);
                segments.push(Segment {
                    number: segments.len() + 1,
                    argv,
                    cwd,
                    entries: &self.entries[index + 1..end],
                });
            }
        }
        segments
    }

    /// The transcript as the fixture files hold it: pretty, one trailing
    /// newline.
    ///
    /// # Panics
    /// Never: a transcript is plain JSON.
    #[must_use]
    pub fn to_file_text(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a transcript serializes");
        text.push('\n');
        text
    }
}

/// One launch of a transcript.
#[derive(Clone, Copy, Debug)]
pub struct Segment<'a> {
    /// 1-based, in launch order.
    pub number: usize,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub entries: &'a [Entry],
}

/// The key a probe answers under.
#[must_use]
pub fn probe_key(argv: &[String]) -> String {
    argv.join(" ")
}

/// The flags in an argv, without their values, sorted: what a launch's argv
/// must share with the recorded one. Values differ run to run (session ids,
/// paths); a flag added or dropped by an adapter is a framing change.
#[must_use]
pub fn flag_shape(argv: &[String]) -> Vec<String> {
    let mut flags: Vec<String> = argv
        .iter()
        .filter(|argument| argument.starts_with('-'))
        .map(|argument| {
            argument
                .split_once('=')
                .map_or(argument.as_str(), |(flag, _)| flag)
                .to_owned()
        })
        .collect();
    flags.sort();
    flags
}
