//! Whether a pane's program is waiting for the user ("needs you").
//!
//! Rust owns this answer (host protocol v44, `TerminalSummary.attention`):
//! the desktop renderer and the phone read it instead of each deciding. The
//! rules are the renderer's former ones (`electron/src/renderer/
//! terminal-activity.ts`), held to the shared fixture
//! `electron/src/fixtures/terminal-attention-v44.json` on both sides:
//!
//! - Claude's host-installed hooks answer `interface-claude-activity:waiting`
//!   while a permission dialog or question is on screen; `working` or `idle`
//!   ends it.
//! - Codex panes run with `tui.notification_method=osc9`: an approval-family
//!   OSC 9 message waits; any other message is a finished turn.
//! - The host's private activity titles (`interface-terminal-activity:v1:`,
//!   `OpenCode`) say `waiting` outright; they own activity except for AGY,
//!   whose OSC 7501 TUI status is emitted after its synchronous hooks finish.
//! - A bare BEL waits, except from Codex (its BEL is turn-complete noise) and
//!   from Claude right after it reported a finished turn.
//! - A spinner title (work resumed) or the user's input ends a wait. Terminal
//!   protocol replies (focus and capability reports) are not input.
//!
//! The scanner mirrors the renderer's byte for byte, so a BEL inside a CSI or
//! a string terminator never reads as a bell on one side only.
use interface_contracts::terminal::TerminalLaunchProfileId;

use crate::{
    launch::CLAUDE_ACTIVITY_TITLE_PREFIX, opencode_activity::TERMINAL_ACTIVITY_PREFIX,
    screen::is_terminal_report,
};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
/// The renderer's `MAX_PAYLOAD_BYTES`: longer titles are discarded whole.
const MAX_PAYLOAD_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Scan {
    Ground,
    Escape,
    EscapeIntermediate,
    CsiParameter,
    CsiIntermediate,
    Parameter,
    Payload,
    Discard,
    PayloadEscape,
    DiscardEscape,
    ControlString,
    ControlStringEscape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivitySource {
    Fallback,
    HostHooks,
    /// AGY's final TUI state follows its synchronous, potentially vetoed hooks.
    AgyStatus,
}

/// One terminal generation's "needs you" state, fed every visible PTY byte
/// and every input write.
#[derive(Clone, Debug)]
pub struct AttentionMonitor {
    profile: TerminalLaunchProfileId,
    activity_source: ActivitySource,
    /// The CLI reported a finished turn and nothing has started since.
    completion_reported: bool,
    waiting: bool,
    scan: Scan,
    parameter: u32,
    parameter_digits: u8,
    payload: Vec<u8>,
}

impl AttentionMonitor {
    #[must_use]
    pub fn new(profile: TerminalLaunchProfileId) -> Self {
        Self {
            profile,
            activity_source: ActivitySource::Fallback,
            completion_reported: false,
            waiting: false,
            scan: Scan::Ground,
            parameter: 0,
            parameter_digits: 0,
            payload: Vec::new(),
        }
    }

    /// Whether the program is waiting for the user now.
    #[must_use]
    pub const fn waiting(&self) -> bool {
        self.waiting
    }

    /// The pane's launch profile changed (a shell pane now runs an agent):
    /// the private activity titles are for the old program.
    pub fn set_profile(&mut self, profile: TerminalLaunchProfileId) {
        if self.profile != profile {
            self.profile = profile;
            self.activity_source = ActivitySource::Fallback;
        }
    }

    /// Feeds visible PTY output. Answers whether `waiting` changed.
    pub fn feed(&mut self, bytes: &[u8]) -> bool {
        let before = self.waiting;
        let mut index = 0;
        while index < bytes.len() {
            if self.scan == Scan::Ground {
                // Plain text never changes a wait: jump to the next ESC or BEL.
                match bytes[index..]
                    .iter()
                    .position(|byte| *byte == ESC || *byte == BEL)
                {
                    Some(offset) => index += offset,
                    None => break,
                }
            }
            self.byte(bytes[index]);
            index += 1;
        }
        self.waiting != before
    }

    /// Notes input written to the pane. Answers whether `waiting` changed.
    pub fn input(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() || is_terminal_report(bytes) {
            return false;
        }
        self.completion_reported = false;
        if self.activity_source == ActivitySource::HostHooks || !self.waiting {
            return false;
        }
        self.waiting = false;
        true
    }

    fn byte(&mut self, byte: u8) {
        match self.scan {
            Scan::Ground => {
                if byte == ESC {
                    self.scan = Scan::Escape;
                } else if byte == BEL {
                    self.bell();
                }
            }
            Scan::Escape => {
                self.scan = match byte {
                    ESC => Scan::Escape,
                    CAN | SUB => Scan::Ground,
                    b']' => {
                        self.parameter = 0;
                        self.parameter_digits = 0;
                        Scan::Parameter
                    }
                    b'[' => Scan::CsiParameter,
                    b'P' | b'X' | b'^' | b'_' => Scan::ControlString,
                    0x20..=0x2f => Scan::EscapeIntermediate,
                    _ => Scan::Ground,
                };
            }
            Scan::EscapeIntermediate => {
                if byte == ESC {
                    self.scan = Scan::Escape;
                } else if matches!(byte, CAN | SUB | 0x30..=0x7e) {
                    self.scan = Scan::Ground;
                }
            }
            Scan::CsiParameter => {
                if byte == ESC {
                    self.scan = Scan::Escape;
                } else if matches!(byte, CAN | SUB | 0x40..=0x7e) {
                    self.scan = Scan::Ground;
                } else if (0x20..=0x2f).contains(&byte) {
                    self.scan = Scan::CsiIntermediate;
                }
            }
            Scan::CsiIntermediate => {
                if byte == ESC {
                    self.scan = Scan::Escape;
                } else if matches!(byte, CAN | SUB | 0x40..=0x7e) {
                    self.scan = Scan::Ground;
                }
            }
            Scan::Parameter => self.parameter_byte(byte),
            Scan::Payload => self.payload_byte(byte),
            Scan::PayloadEscape => {
                if byte == b'\\' {
                    self.take_title();
                    self.scan = Scan::Ground;
                } else {
                    self.reset_scan();
                }
            }
            Scan::Discard => {
                if byte == BEL {
                    self.scan = Scan::Ground;
                } else if byte == ESC {
                    self.scan = Scan::DiscardEscape;
                }
            }
            Scan::DiscardEscape => {
                if byte == b'\\' {
                    self.scan = Scan::Ground;
                } else {
                    self.reset_scan();
                }
            }
            Scan::ControlString => {
                if byte == ESC {
                    self.scan = Scan::ControlStringEscape;
                }
            }
            Scan::ControlStringEscape => {
                if byte == b'\\' {
                    self.scan = Scan::Ground;
                } else if byte != ESC {
                    self.scan = Scan::ControlString;
                }
            }
        }
    }

    fn parameter_byte(&mut self, byte: u8) {
        if byte.is_ascii_digit() {
            if self.parameter_digits >= 4 {
                self.scan = Scan::Discard;
                return;
            }
            self.parameter = self.parameter * 10 + u32::from(byte - b'0');
            self.parameter_digits += 1;
            return;
        }
        self.scan = match byte {
            b';' if self.parameter_digits > 0 && matches!(self.parameter, 0 | 2 | 9 | 7501) => {
                self.payload.clear();
                Scan::Payload
            }
            BEL => Scan::Ground,
            ESC => Scan::DiscardEscape,
            _ => Scan::Discard,
        };
    }

    fn payload_byte(&mut self, byte: u8) {
        if byte == BEL {
            self.take_title();
            self.scan = Scan::Ground;
        } else if byte == ESC {
            self.scan = Scan::PayloadEscape;
        } else if self.payload.len() >= MAX_PAYLOAD_BYTES {
            self.payload.clear();
            self.scan = Scan::Discard;
        } else {
            self.payload.push(byte);
        }
    }

    fn reset_scan(&mut self) {
        self.scan = Scan::Ground;
        self.payload.clear();
        self.parameter = 0;
        self.parameter_digits = 0;
    }

    fn take_title(&mut self) {
        let payload = std::mem::take(&mut self.payload);
        let raw = String::from_utf8_lossy(&payload);
        let parameter = self.parameter;
        if self.activity_source == ActivitySource::AgyStatus && parameter != 7501 {
            return;
        }
        if matches!(parameter, 0 | 2)
            && let Some(signal) = raw.strip_prefix(TERMINAL_ACTIVITY_PREFIX)
        {
            if !matches!(signal, "working" | "waiting" | "idle" | "complete") {
                return;
            }
            self.activity_source = ActivitySource::HostHooks;
            if signal == "complete" {
                self.completion_reported = true;
            } else if matches!(signal, "working" | "waiting") {
                self.completion_reported = false;
            }
            self.waiting = signal == "waiting";
            return;
        }
        if self.activity_source == ActivitySource::HostHooks
            && !(self.profile == TerminalLaunchProfileId::Gemini && parameter == 7501)
        {
            return;
        }
        if parameter == 9 {
            // Only Codex panes are launched with OSC 9 notifications.
            if self.profile != TerminalLaunchProfileId::Codex {
                return;
            }
            match codex_osc9_waits(&raw) {
                Some(true) => {
                    self.completion_reported = false;
                    self.waiting = true;
                }
                Some(false) => {
                    self.completion_reported = true;
                    self.waiting = false;
                }
                None => {}
            }
            return;
        }
        if parameter == 7501 {
            let status = osc7501_state(&raw);
            if status.is_some() && self.profile == TerminalLaunchProfileId::Gemini {
                self.activity_source = ActivitySource::AgyStatus;
            }
            match status {
                Some("working") => {
                    self.completion_reported = false;
                    self.waiting = false;
                }
                Some("blocked") => {
                    self.completion_reported = false;
                    self.waiting = true;
                }
                Some("done") => {
                    self.completion_reported = true;
                    self.waiting = false;
                }
                Some("idle" | "error" | "clear") => {
                    self.waiting = false;
                }
                _ => {}
            }
            return;
        }
        if let Some(turn) = raw.strip_prefix(CLAUDE_ACTIVITY_TITLE_PREFIX) {
            match turn {
                "working" => {
                    self.completion_reported = false;
                    self.waiting = false;
                }
                "waiting" => {
                    self.completion_reported = false;
                    self.waiting = true;
                }
                "idle" => {
                    self.completion_reported = true;
                    self.waiting = false;
                }
                _ => {}
            }
            return;
        }
        // A spinner title is work resumed; a brand mark or a task name leaves
        // a wait alone (Claude renames its session mid-dialog).
        if title_is_spinner(&raw) {
            self.completion_reported = false;
            self.waiting = false;
        }
    }

    fn bell(&mut self) {
        // Codex is forced onto OSC 9; bare BEL there is turn-complete noise.
        // Claude may ring beside its Stop frame too.
        if self.activity_source != ActivitySource::Fallback
            || self.profile == TerminalLaunchProfileId::Codex
            || (self.profile == TerminalLaunchProfileId::Claude && self.completion_reported)
        {
            return;
        }
        self.waiting = true;
    }
}

/// Antigravity (`agy`) OSC 7501 `ProgramStatus` frames: colon-separated
/// `key=value` fields with `state=working|blocked|done|error|idle|clear`.
/// Mirrors `osc7501State`.
pub(crate) fn osc7501_state(raw: &str) -> Option<&str> {
    raw.split(':')
        .find_map(|field| field.strip_prefix("state="))
        .filter(|state| {
            matches!(
                *state,
                "working" | "blocked" | "done" | "error" | "idle" | "clear"
            )
        })
}

/// Codex OSC 9 payloads: approval-family messages wait, any other non-empty
/// message is a finished turn. Mirrors `codexOsc9NotificationKind`.
fn codex_osc9_waits(raw: &str) -> Option<bool> {
    let message = trim_title(raw);
    if message.is_empty() {
        return None;
    }
    let lower = message.to_lowercase();
    Some(
        lower.starts_with("approval requested")
            || lower.starts_with("codex wants to edit")
            || lower.starts_with("plan mode prompt:"),
    )
}

/// `titleKind(raw) === "working"`: a transient spinner leads the title, or
/// follows the agent's brand marks.
fn title_is_spinner(raw: &str) -> bool {
    let title = trim_title(raw);
    let mut characters = title.chars().peekable();
    let Some(&first) = characters.peek() else {
        return false;
    };
    if is_transient_spinner(first) {
        return true;
    }
    while characters.peek().is_some_and(|c| is_brand_mark(*c)) {
        characters.next();
        while characters.peek().is_some_and(|c| is_trimmable(*c)) {
            characters.next();
        }
    }
    characters.peek().is_some_and(|c| is_transient_spinner(*c))
}

fn is_transient_spinner(character: char) -> bool {
    matches!(character, '\u{2800}'..='\u{28ff}') || "●○◐◓◑◒◆▪★*⏳⚡".contains(character)
}

fn is_brand_mark(character: char) -> bool {
    "✳✻✽·•".contains(character)
}

/// JavaScript's `c.trim() === "" || c.charCodeAt(0) < 32`.
fn is_trimmable(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}')
        || character == '\u{feff}'
        || u32::from(character) < 32
}

fn trim_title(raw: &str) -> &str {
    raw.trim_matches(is_trimmable)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str =
        include_str!("../../../electron/src/fixtures/terminal-attention-v44.json");

    fn profile(name: &str) -> TerminalLaunchProfileId {
        serde_json::from_value(serde_json::Value::String(name.to_owned())).unwrap()
    }

    #[test]
    fn terminal_attention_follows_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(fixture["version"], 44);
        let cases = fixture["cases"].as_array().unwrap();
        assert!(cases.len() >= 12);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let mut monitor = AttentionMonitor::new(profile(case["profile"].as_str().unwrap()));
            for (index, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                if let Some(output) = step["output"].as_str() {
                    monitor.feed(output.as_bytes());
                } else if let Some(input) = step["input"].as_str() {
                    monitor.input(input.as_bytes());
                } else if let Some(next) = step["profile"].as_str() {
                    monitor.set_profile(profile(next));
                }
                assert_eq!(
                    monitor.waiting(),
                    step["waiting"].as_bool().unwrap(),
                    "{name}, step {index}"
                );
            }
        }
    }

    #[test]
    fn a_split_frame_and_a_bell_inside_escapes_never_read_as_a_wait() {
        let mut monitor = AttentionMonitor::new(TerminalLaunchProfileId::Claude);
        assert!(!monitor.feed(b"\x1b]2;interface-claude-activity:wai"));
        assert!(monitor.feed(b"ting\x07"));
        assert!(monitor.waiting());
        assert!(monitor.feed(b"\x1b]2;interface-claude-activity:idle\x1b\\"));
        // A string terminator and a BEL inside a CSI are not bells.
        let mut shell = AttentionMonitor::new(TerminalLaunchProfileId::Shell);
        assert!(!shell.feed(b"\x1b]0;title\x07\x1b[3\x07m"));
        assert!(!shell.waiting());
        assert!(shell.feed(b"done\x07"));
        // A focus report is not an answer; a keystroke is.
        assert!(!shell.input(b"\x1b[I"));
        assert!(shell.input(b"y"));
        assert!(!shell.waiting());
    }

    #[test]
    fn an_overlong_title_is_dropped_whole() {
        let mut monitor = AttentionMonitor::new(TerminalLaunchProfileId::Claude);
        let mut frame = b"\x1b]2;interface-claude-activity:waiting".to_vec();
        frame.extend(std::iter::repeat_n(b' ', MAX_PAYLOAD_BYTES));
        frame.push(BEL);
        assert!(!monitor.feed(&frame));
        assert!(!monitor.waiting());
    }
}
