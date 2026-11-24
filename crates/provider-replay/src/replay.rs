//! The replayer: serves a transcript's frames by what arrives, never by time.
//!
//! A process the adapter starts is either an auxiliary invocation, answered
//! from `probes` by its argv, or a session launch. A launch may be any
//! recorded launch with the same flags; the candidates advance together
//! frame by frame, and a candidate an arriving frame does not match drops
//! out (a resume and a fresh start share their handshake, and differ at
//! `thread/resume` or `--resume`). Within a launch, the `in` frames that
//! follow an `out` frame are written once a frame matching it arrives; a
//! frame the host sends a little earlier than recorded (a request from a
//! separate host task) is matched ahead and its place skipped when the
//! cursor reaches it.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::{
    matcher::{Bindings, bind, difference, excerpt, signature, substitute},
    transcript::{Entry, Transcript, flag_shape, probe_key},
};

/// How many later `out` frames an early frame may match.
const LOOKAHEAD: usize = 8;
/// How much of a frame a failure message shows.
const EXCERPT: usize = 1_500;

/// What a started process does.
#[derive(Debug)]
pub enum Start {
    /// An auxiliary invocation: print this and exit with this code.
    Probe { stdout: String, code: i32 },
    /// A session launch: write these lines first, then feed it stdin.
    Session { replay: Replay, lines: Vec<String> },
}

/// One launch being replayed (or several still indistinguishable).
#[derive(Debug)]
pub struct Replay {
    candidates: Vec<Cursor>,
}

#[derive(Clone, Debug)]
struct Cursor {
    /// 1-based launch number, for messages.
    launch: usize,
    entries: Vec<Entry>,
    /// The next entry not yet served or matched.
    position: usize,
    bindings: Bindings,
    /// `out` entries matched ahead of the cursor.
    early: BTreeSet<usize>,
}

/// Decides what a process started with `argv` (program excluded) in `cwd`
/// is, and for a launch writes the `in` frames recorded before the host's
/// first frame.
///
/// # Errors
/// No probe answers `argv` and no recorded launch has its flags.
pub fn start(transcript: &Transcript, argv: &[String], cwd: &str) -> Result<Start, String> {
    if let Some(probe) = transcript.probes.get(&probe_key(argv)) {
        return Ok(Start::Probe {
            stdout: probe.stdout.clone(),
            code: probe.code,
        });
    }
    let shape = flag_shape(argv);
    let mut candidates: Vec<Cursor> = transcript
        .segments()
        .into_iter()
        .filter(|segment| flag_shape(segment.argv) == shape)
        .map(|segment| {
            let mut bindings = Bindings::default();
            bind_argv(segment.argv, argv, &mut bindings);
            bindings.bind_string(segment.cwd, cwd);
            Cursor {
                launch: segment.number,
                entries: segment.entries.to_vec(),
                position: 0,
                bindings,
                early: BTreeSet::new(),
            }
        })
        .collect();
    if candidates.is_empty() {
        let launches: Vec<String> = transcript
            .segments()
            .iter()
            .map(|segment| format!("{:?}", segment.argv))
            .collect();
        let probes: Vec<&String> = transcript.probes.keys().collect();
        return Err(format!(
            "no recorded launch or probe matches argv {argv:?} (flags {shape:?}); \
             recorded launches: [{}]; probes: {probes:?}",
            launches.join(", ")
        ));
    }
    let lines = candidates
        .iter_mut()
        .map(Cursor::drain)
        .next()
        .map(|frames| frames.iter().map(line).collect())
        .unwrap_or_default();
    Ok(Start::Session {
        replay: Replay { candidates },
        lines,
    })
}

/// Binds the values of an argv that differ from the recording: a
/// `--flag=value` by its value, any other token whole.
fn bind_argv(recorded: &[String], arrived: &[String], bindings: &mut Bindings) {
    if recorded.len() != arrived.len() {
        return;
    }
    for (recorded, arrived) in recorded.iter().zip(arrived) {
        match (recorded.split_once('='), arrived.split_once('=')) {
            (Some((flag, recorded)), Some((other, arrived)))
                if flag == other && flag.starts_with('-') =>
            {
                bindings.bind_string(recorded, arrived);
            }
            _ => bindings.bind_string(recorded, arrived),
        }
    }
}

/// How a frame is written: compact JSON, or a recorded text line as is.
fn line(frame: &Value) -> String {
    match frame {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Reads one stdin line as a frame: JSON, or the text itself.
#[must_use]
pub fn parse_line(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

impl Replay {
    /// Matches one frame from the host and answers the lines to write.
    ///
    /// # Errors
    /// The frame matches no candidate launch: the message names the
    /// expected and the received frame.
    pub fn accept(&mut self, frame: &Value) -> Result<Vec<String>, String> {
        let mut first_error = None;
        let mut output = None;
        let mut survivors = Vec::with_capacity(self.candidates.len());
        for mut candidate in std::mem::take(&mut self.candidates) {
            match candidate.accept(frame) {
                Ok(frames) => {
                    if output.is_none() {
                        output = Some(frames.iter().map(line).collect());
                    }
                    survivors.push(candidate);
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        self.candidates = survivors;
        match output {
            Some(lines) => Ok(lines),
            None => Err(first_error.unwrap_or_else(|| "no launch left to replay".to_owned())),
        }
    }

    /// Stdin closed: the exit code to leave with, the recorded one when the
    /// recording's process exited. A host that closes before frames the
    /// recording still holds (an advisory request it had not sent yet) is
    /// not a failure: what it did send all matched.
    #[must_use]
    pub fn finish(&self) -> i32 {
        self.candidates.first().map_or(0, Cursor::finish)
    }

    /// The bindings of the first candidate (for tests).
    #[must_use]
    pub fn bindings(&self) -> Option<&Bindings> {
        self.candidates.first().map(|candidate| &candidate.bindings)
    }
}

impl Cursor {
    /// Serves the `in` frames from the cursor on, skipping `out` frames that
    /// already arrived, up to the next `out` frame still awaited.
    fn drain(&mut self) -> Vec<Value> {
        let mut frames = Vec::new();
        while let Some(entry) = self.entries.get(self.position) {
            match entry {
                Entry::In { frame } => frames.push(substitute(frame, &self.bindings)),
                Entry::Out { .. } if self.early.contains(&self.position) => {}
                Entry::Out { .. } | Entry::Exit { .. } | Entry::Launch { .. } => break,
            }
            self.position += 1;
        }
        frames
    }

    fn accept(&mut self, frame: &Value) -> Result<Vec<Value>, String> {
        let expected = match self.entries.get(self.position) {
            Some(Entry::Out { frame }) => frame.clone(),
            Some(Entry::Exit { code }) => {
                return Err(self.unexpected_after_end(frame, &format!("exited with code {code}")));
            }
            _ => return Err(self.unexpected_after_end(frame, "ended")),
        };
        let Some(reason) = difference(&expected, frame) else {
            bind(&expected, frame, &mut self.bindings);
            self.position += 1;
            return Ok(self.drain());
        };
        if let Some(index) = self.match_ahead(frame) {
            if let Some(Entry::Out { frame: recorded }) = self.entries.get(index) {
                let recorded = recorded.clone();
                bind(&recorded, frame, &mut self.bindings);
            }
            self.early.insert(index);
            return Ok(Vec::new());
        }
        Err(format!(
            "launch {}, step {}: unexpected frame from Interface\n  expected: {}: {}\n  \
             received: {}: {}\n  difference: {reason}",
            self.launch,
            self.position + 1,
            signature(&expected),
            excerpt(&expected, EXCERPT),
            signature(frame),
            excerpt(frame, EXCERPT),
        ))
    }

    /// A later awaited `out` frame `frame` matches, within [`LOOKAHEAD`].
    fn match_ahead(&self, frame: &Value) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .skip(self.position + 1)
            .take_while(|(_, entry)| !matches!(entry, Entry::Exit { .. }))
            .filter(|(index, entry)| {
                matches!(entry, Entry::Out { .. }) && !self.early.contains(index)
            })
            .take(LOOKAHEAD)
            .find(|(_, entry)| {
                matches!(entry, Entry::Out { frame: recorded } if difference(recorded, frame).is_none())
            })
            .map(|(index, _)| index)
    }

    fn unexpected_after_end(&self, frame: &Value, how: &str) -> String {
        format!(
            "launch {}: unexpected frame from Interface after the recorded process {how}\n  \
             expected: no more frames\n  received: {}: {}",
            self.launch,
            signature(frame),
            excerpt(frame, EXCERPT),
        )
    }

    fn finish(&self) -> i32 {
        self.entries[self.position..]
            .iter()
            .find_map(|entry| match entry {
                Entry::Exit { code } => Some(*code),
                _ => None,
            })
            .unwrap_or(0)
    }
}
