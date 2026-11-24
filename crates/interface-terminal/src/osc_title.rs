//! Incremental OSC title extraction from a raw PTY byte stream.
//!
//! This is deliberately not a terminal emulator. It recognizes exactly the
//! window-title operating system commands — `ESC ] 0 ; text BEL` (icon and
//! title) and `ESC ] 2 ; text BEL`, either terminator — and ignores every
//! other escape sequence by scanning past it. OSC 1 sets the icon only and is
//! read as a title on no terminal, so it is skipped.
//!
//! The scanner is fed the same chunks the reader thread pushes downstream, so
//! a sequence split across two reads must survive the boundary; all parse
//! state lives in the struct rather than on the stack.

/// Longest OSC payload worth accumulating. A title is a tab label, not a
/// document: anything past this is a runaway or binary stream, and the
/// sequence is abandoned rather than buffered.
const MAX_PAYLOAD_BYTES: usize = 512;

/// Upper bound on a returned title, in characters. Kept equal to the terminal
/// service's `MAX_TITLE_CHARS` so a scanned title can never be rejected by
/// `normalize_title` for length alone.
const MAX_TITLE_CHARS: usize = 120;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Result of removing one private title channel from a PTY chunk.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct FilteredOscChunk {
    pub visible: Vec<u8>,
    /// Bytes following the reserved title prefix, without its terminator.
    pub reserved_payloads: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReservedFilterState {
    Ground,
    Candidate,
    Reserved,
    ReservedEscape,
    DiscardReserved,
    DiscardReservedEscape,
}

/// Strip a private OSC 0/2 title channel before terminal bytes cross into the
/// renderer, while preserving every unrelated byte exactly.
///
/// The generation correlation token is private control-plane material.
/// Forwarding the raw OSC would expose it to the renderer and replay buffer,
/// so this filter removes complete, malformed, and split reserved frames
/// before output is queued.
#[derive(Debug)]
pub(crate) struct ReservedOscFilter {
    state: ReservedFilterState,
    introducer_zero: Vec<u8>,
    introducer_two: Vec<u8>,
    candidate: Vec<u8>,
    payload: Vec<u8>,
    max_payload_bytes: usize,
}

impl ReservedOscFilter {
    const MAX_RESERVED_PAYLOAD_BYTES: usize = 512;

    pub(crate) fn new(prefix: &str) -> Self {
        Self::with_limit(prefix, Self::MAX_RESERVED_PAYLOAD_BYTES)
    }

    pub(crate) fn with_limit(prefix: &str, max_payload_bytes: usize) -> Self {
        let mut introducer_zero = Vec::from(&b"\x1b]0;"[..]);
        introducer_zero.extend_from_slice(prefix.as_bytes());
        let mut introducer_two = Vec::from(&b"\x1b]2;"[..]);
        introducer_two.extend_from_slice(prefix.as_bytes());
        Self {
            state: ReservedFilterState::Ground,
            introducer_zero,
            introducer_two,
            candidate: Vec::new(),
            payload: Vec::new(),
            max_payload_bytes,
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> FilteredOscChunk {
        let mut visible = Vec::with_capacity(bytes.len());
        let mut reserved_payloads = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            // Ordinary PTY text cannot open a private channel. Copy each run
            // once instead of growing the output one byte at a time.
            if self.state == ReservedFilterState::Ground {
                let end = bytes[index..]
                    .iter()
                    .position(|byte| *byte == ESC)
                    .map_or(bytes.len(), |offset| index + offset);
                visible.extend_from_slice(&bytes[index..end]);
                index = end;
                if index == bytes.len() {
                    break;
                }
            }
            let byte = bytes[index];
            index += 1;
            match self.state {
                ReservedFilterState::Ground => {
                    // The bulk copy above stops only on ESC.
                    debug_assert_eq!(byte, ESC);
                    self.candidate.clear();
                    self.candidate.push(ESC);
                    self.state = ReservedFilterState::Candidate;
                }
                ReservedFilterState::Candidate => {
                    self.candidate.push(byte);
                    if self.candidate == self.introducer_zero
                        || self.candidate == self.introducer_two
                    {
                        self.payload.clear();
                        self.state = ReservedFilterState::Reserved;
                    } else if !self.introducer_zero.starts_with(&self.candidate)
                        && !self.introducer_two.starts_with(&self.candidate)
                    {
                        self.flush_candidate(&mut visible);
                    }
                }
                ReservedFilterState::Reserved => match byte {
                    BEL => self.finish_reserved(&mut reserved_payloads),
                    ESC => self.state = ReservedFilterState::ReservedEscape,
                    _ if self.payload.len() < self.max_payload_bytes => {
                        self.payload.push(byte);
                    }
                    _ => {
                        self.payload.clear();
                        self.state = ReservedFilterState::DiscardReserved;
                    }
                },
                ReservedFilterState::ReservedEscape => {
                    if byte == b'\\' {
                        self.finish_reserved(&mut reserved_payloads);
                    } else if byte == BEL {
                        // ESC BEL is neither of the two legal OSC
                        // terminators. Keep the private payload private, but
                        // do not let malformed bytes trigger a wake.
                        self.reset();
                    } else {
                        self.payload.clear();
                        self.state = ReservedFilterState::DiscardReserved;
                    }
                }
                ReservedFilterState::DiscardReserved => {
                    if byte == BEL {
                        self.reset();
                    } else if byte == ESC {
                        self.state = ReservedFilterState::DiscardReservedEscape;
                    }
                }
                ReservedFilterState::DiscardReservedEscape => {
                    if byte == b'\\' || byte == BEL {
                        self.reset();
                    } else {
                        self.state = ReservedFilterState::DiscardReserved;
                    }
                }
            }
        }
        FilteredOscChunk {
            visible,
            reserved_payloads,
        }
    }

    /// Flush an incomplete non-reserved introducer at EOF. Once the complete
    /// private prefix has matched, the remainder stays private even when the
    /// child exits without a terminator.
    pub(crate) fn finish(&mut self) -> Vec<u8> {
        let mut visible = Vec::new();
        if self.state == ReservedFilterState::Candidate {
            visible.append(&mut self.candidate);
        }
        self.reset();
        visible
    }

    fn flush_candidate(&mut self, visible: &mut Vec<u8>) {
        let trailing_escape = self.candidate.last() == Some(&ESC);
        let visible_len = self
            .candidate
            .len()
            .saturating_sub(usize::from(trailing_escape));
        visible.extend_from_slice(&self.candidate[..visible_len]);
        self.candidate.clear();
        if trailing_escape {
            self.candidate.push(ESC);
            self.state = ReservedFilterState::Candidate;
        } else {
            self.state = ReservedFilterState::Ground;
        }
    }

    fn finish_reserved(&mut self, reserved_payloads: &mut Vec<String>) {
        if let Ok(payload) = String::from_utf8(std::mem::take(&mut self.payload)) {
            reserved_payloads.push(payload);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.state = ReservedFilterState::Ground;
        self.candidate.clear();
        self.payload.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// Ordinary output. Only ESC is interesting.
    Ground,
    /// Saw ESC, waiting to see whether this opens an OSC.
    Escape,
    /// Inside an OSC, still reading the numeric parameter before the `;`.
    Parameter,
    /// Inside an OSC whose parameter selects a title, collecting the payload.
    Payload,
    /// Inside an OSC we do not want, or one that overflowed. Scan to the
    /// terminator and drop it.
    Discard,
    /// Saw ESC inside an OSC payload: `ESC \` ends it, anything else does not.
    PayloadEscape,
    /// Saw ESC inside a discarded OSC.
    DiscardEscape,
}

/// Streaming extractor for terminal window titles.
#[derive(Debug)]
pub struct OscTitleScanner {
    activity: bool,
    state: State,
    parameter: u16,
    parameter_digits: u8,
    payload: Vec<u8>,
}

impl Default for OscTitleScanner {
    fn default() -> Self {
        Self::new()
    }
}

impl OscTitleScanner {
    #[must_use]
    pub fn new() -> Self {
        Self {
            activity: false,
            state: State::Ground,
            parameter: 0,
            parameter_digits: 0,
            payload: Vec::new(),
        }
    }

    /// Internal activity observer: preserve raw 0/2/9 payloads and their order.
    pub(crate) fn activity() -> Self {
        Self {
            activity: true,
            ..Self::new()
        }
    }

    /// Feed one chunk and return every complete title it closed, in order.
    ///
    /// A chunk may close a sequence opened by an earlier chunk, close several
    /// sequences, or close none. Titles that sanitize to nothing are dropped
    /// here rather than pushed onto the caller.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut titles = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if self.state == State::Ground {
                let Some(offset) = bytes[index..].iter().position(|byte| *byte == ESC) else {
                    break;
                };
                index += offset;
            }
            let byte = bytes[index];
            index += 1;
            match self.state {
                State::Ground => {
                    // The skip above stops only on ESC.
                    debug_assert_eq!(byte, ESC);
                    self.state = State::Escape;
                }
                State::Escape => {
                    if byte == b']' {
                        self.state = State::Parameter;
                        self.parameter = 0;
                        self.parameter_digits = 0;
                    } else {
                        // Any other escape — CSI, charset select, a bare ESC —
                        // is not ours. One byte of lookahead is enough because
                        // an OSC always begins `ESC ]`.
                        self.state = State::Ground;
                    }
                }
                State::Parameter => self.consume_parameter(byte),
                State::Payload => {
                    if let Some(title) = self.consume_payload(byte) {
                        titles.push(title);
                    }
                }
                State::PayloadEscape => {
                    if byte == b'\\' {
                        if let Some(title) = self.take_title() {
                            titles.push(title);
                        }
                        self.state = State::Ground;
                    } else {
                        // A lone ESC inside a payload is malformed. Treat the
                        // sequence as lost rather than swallowing the rest of
                        // the stream looking for a terminator.
                        self.reset();
                    }
                }
                State::Discard => {
                    if byte == BEL {
                        self.state = State::Ground;
                    } else if byte == ESC {
                        self.state = State::DiscardEscape;
                    }
                }
                State::DiscardEscape => {
                    if byte == b'\\' {
                        self.state = State::Ground;
                    } else {
                        self.reset();
                    }
                }
            }
        }
        titles
    }

    fn consume_parameter(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                // A parameter longer than a few digits is not a title command.
                if self.parameter_digits >= 4 {
                    self.state = State::Discard;
                    return;
                }
                self.parameter = self
                    .parameter
                    .saturating_mul(10)
                    .saturating_add(u16::from(byte - b'0'));
                self.parameter_digits = self.parameter_digits.saturating_add(1);
            }
            b';' => {
                // 0 sets icon name and title together, 2 sets the title. 1 is
                // icon-only and never shows in a tab.
                if self.parameter_digits > 0
                    && (matches!(self.parameter, 0 | 2)
                        || (self.activity && matches!(self.parameter, 9 | 7501)))
                {
                    self.payload.clear();
                    self.state = State::Payload;
                } else {
                    self.state = State::Discard;
                }
            }
            BEL => self.state = State::Ground,
            ESC => self.state = State::DiscardEscape,
            _ => self.state = State::Discard,
        }
    }

    fn consume_payload(&mut self, byte: u8) -> Option<String> {
        match byte {
            BEL => {
                let title = self.take_title();
                self.state = State::Ground;
                title
            }
            ESC => {
                self.state = State::PayloadEscape;
                None
            }
            _ => {
                if self.payload.len() >= MAX_PAYLOAD_BYTES {
                    self.payload.clear();
                    self.state = State::Discard;
                } else {
                    self.payload.push(byte);
                }
                None
            }
        }
    }

    fn take_title(&mut self) -> Option<String> {
        let raw = String::from_utf8_lossy(&self.payload);
        // A hook's activity frame is a signal for the renderer, not a name
        // for the tab.
        let title = if self.activity {
            Some(format!("{};{raw}", self.parameter))
        } else if raw.starts_with(crate::launch::CLAUDE_ACTIVITY_TITLE_PREFIX)
            || raw.starts_with(crate::opencode_activity::TERMINAL_ACTIVITY_PREFIX)
        {
            None
        } else {
            sanitize_title(&raw)
        };
        // Keep the bounded scratch allocation for the next animated title.
        self.payload.clear();
        title
    }

    fn reset(&mut self) {
        self.state = State::Ground;
        self.payload.clear();
        self.parameter = 0;
        self.parameter_digits = 0;
    }
}

/// True for the decorative glyphs agent CLIs prefix onto a title to signal
/// activity: the braille spinner block, and the handful of status marks
/// observed in practice. Stripping these is what makes a ~10 Hz animated
/// title collapse to one stable string instead of a rename per frame.
fn is_decoration(character: char) -> bool {
    matches!(character, '\u{2800}'..='\u{28ff}')
        || matches!(
            character,
            '✳' | '✻'
                | '✽'
                | '·'
                | '•'
                | '●'
                | '○'
                | '◐'
                | '◓'
                | '◑'
                | '◒'
                | '◆'
                | '▪'
                | '★'
                | '*'
                | '⏳'
                | '⚡'
        )
}

/// Reduce a raw OSC payload to a tab label, or `None` when nothing usable
/// survives.
///
/// Controls are dropped, the leading activity decoration is removed, internal
/// whitespace is collapsed so two spinner frames compare equal, and the result
/// is clamped to the service's own title limit.
#[must_use]
pub fn sanitize_title(raw: &str) -> Option<String> {
    let undecorated = raw
        .trim_matches(|character: char| {
            character.is_whitespace() || character.is_control() || is_decoration(character)
        })
        .trim();

    let mut title = String::new();
    let mut pending_space = false;
    let mut used = 0_usize;
    for character in undecorated.chars() {
        // Whitespace first: tab and newline are both control and whitespace,
        // and treating them as controls would join the words they separate.
        if character.is_whitespace() {
            pending_space = !title.is_empty();
            continue;
        }
        if character.is_control() {
            continue;
        }
        if pending_space {
            title.push(' ');
            used += 1;
            pending_space = false;
        }
        if used >= MAX_TITLE_CHARS {
            break;
        }
        title.push(character);
        used += 1;
    }

    if title.is_empty() { None } else { Some(title) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<String> {
        let mut scanner = OscTitleScanner::new();
        let mut titles = Vec::new();
        for chunk in chunks {
            titles.extend(scanner.feed(chunk));
        }
        titles
    }

    #[test]
    fn reads_a_bel_terminated_title() {
        assert_eq!(
            scan(&[b"\x1b]0;Parser rewrite\x07"]),
            vec!["Parser rewrite"]
        );
    }

    #[test]
    fn reads_a_string_terminated_title() {
        assert_eq!(
            scan(&[b"\x1b]2;Parser rewrite\x1b\\"]),
            vec!["Parser rewrite"]
        );
    }

    #[test]
    fn survives_a_split_across_chunks() {
        // The boundary falls inside the payload, which is the case a
        // stack-local parser would lose.
        assert_eq!(
            scan(&[b"noise\x1b]0;Parser re", b"write\x07more noise"]),
            vec!["Parser rewrite"]
        );
    }

    #[test]
    fn survives_a_split_inside_the_introducer() {
        assert_eq!(
            scan(&[b"\x1b", b"]0;Fixing tests\x07"]),
            vec!["Fixing tests"]
        );
    }

    #[test]
    fn ignores_icon_only_and_unrelated_commands() {
        // OSC 1 is icon-only; OSC 7 reports the cwd. Neither is a tab title.
        assert_eq!(scan(&[b"\x1b]1;icon\x07"]), Vec::<String>::new());
        assert_eq!(scan(&[b"\x1b]7;file:///tmp\x07"]), Vec::<String>::new());
    }

    #[test]
    fn ignores_csi_sequences() {
        assert_eq!(scan(&[b"\x1b[2J\x1b[1;1H\x1b[m"]), Vec::<String>::new());
    }

    #[test]
    fn reads_several_titles_from_one_chunk() {
        assert_eq!(
            scan(&[b"\x1b]0;first\x07body\x1b]0;second\x07"]),
            vec!["first", "second"]
        );
    }

    #[test]
    fn drops_a_claude_hook_activity_frame() {
        let mut scanner = OscTitleScanner::new();
        let titles = scanner
            .feed(b"\x1b]2;interface-claude-activity:working\x07\x1b]0;\xe2\x9c\xb3 Zebra\x07");
        assert_eq!(titles, vec!["Zebra".to_owned()]);
    }

    #[test]
    fn strips_the_claude_activity_mark() {
        // Observed from Claude Code 2.1.251.
        assert_eq!(
            scan(&[b"\x1b]0;\xe2\x9c\xb3 Zebra Widget\x07"]),
            vec!["Zebra Widget"]
        );
    }

    #[test]
    fn collapses_spinner_frames_to_one_title() {
        // Observed from Codex 0.150.1, which animates a braille spinner at
        // roughly 10 Hz. Every frame must sanitize to the same string or the
        // service would emit a rename per frame.
        let frames: Vec<String> = ["\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}"]
            .iter()
            .map(|spinner| format!("\x1b]0;{spinner} zebra-parser\x07"))
            .collect();
        let titles = scan(&frames.iter().map(String::as_bytes).collect::<Vec<_>>());
        assert_eq!(titles, vec!["zebra-parser"; 4]);
    }

    #[test]
    fn drops_a_title_that_is_only_decoration() {
        assert_eq!(scan(&[b"\x1b]0;\xe2\xa0\x8b\x07"]), Vec::<String>::new());
        assert_eq!(scan(&[b"\x1b]0;   \x07"]), Vec::<String>::new());
    }

    #[test]
    fn abandons_an_overlong_payload_without_growing() {
        let mut scanner = OscTitleScanner::new();
        let flood = vec![b'x'; MAX_PAYLOAD_BYTES * 4];
        assert!(scanner.feed(b"\x1b]0;").is_empty());
        assert!(scanner.feed(&flood).is_empty());
        assert!(scanner.payload.len() <= MAX_PAYLOAD_BYTES);
        // The stream recovers on the next well-formed sequence.
        assert_eq!(scanner.feed(b"\x07\x1b]0;after\x07"), vec!["after"]);
    }

    #[test]
    fn clamps_a_long_title_to_the_service_limit() {
        let long = "a".repeat(MAX_TITLE_CHARS * 2);
        let titles = scan(&[format!("\x1b]0;{long}\x07").as_bytes()]);
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn counts_pending_spaces_toward_the_title_limit() {
        let almost = "a".repeat(MAX_TITLE_CHARS - 1);
        let titles = scan(&[format!("\x1b]0;{almost} more\x07").as_bytes()]);
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0].chars().count(), MAX_TITLE_CHARS);
        assert!(titles[0].ends_with(' '));

        let exact = "a".repeat(MAX_TITLE_CHARS);
        let titles = scan(&[format!("\x1b]0;{exact} more\x07").as_bytes()]);
        assert_eq!(titles.len(), 1);
        assert_eq!(titles[0], format!("{exact} "));
        assert_eq!(titles[0].chars().count(), MAX_TITLE_CHARS + 1);
    }

    #[test]
    fn collapses_internal_whitespace() {
        assert_eq!(scan(&[b"\x1b]0;two   words\x07"]), vec!["two words"]);
        assert_eq!(scan(&[b"\x1b]0;tab\there\x07"]), vec!["tab here"]);
    }

    #[test]
    fn tolerates_invalid_utf8_without_panicking() {
        assert_eq!(scan(&[b"\x1b]0;bad\xff\xfename\x07"]).len(), 1);
    }

    #[test]
    fn a_lone_escape_in_a_payload_does_not_swallow_the_stream() {
        // Malformed input must not leave the scanner hunting for a terminator
        // forever; the following well-formed title still lands.
        assert_eq!(
            scan(&[b"\x1b]0;broken\x1bX\x1b]0;recovered\x07"]),
            vec!["recovered"]
        );
    }

    #[test]
    fn bulk_plain_runs_preserve_private_filtering_at_every_split() {
        let input = b"plain\x1b[0mtext\x1b]2;private:receipt\x07middle\x1b]0;public\x1b\\tail\x1b";
        for split in 0..=input.len() {
            let mut filter = ReservedOscFilter::new("private:");
            let first = filter.feed(&input[..split]);
            let second = filter.feed(&input[split..]);
            let mut visible = first.visible;
            visible.extend(second.visible);
            visible.extend(filter.finish());
            assert_eq!(
                visible,
                b"plain\x1b[0mtextmiddle\x1b]0;public\x1b\\tail\x1b"
            );
            let payloads = [first.reserved_payloads, second.reserved_payloads].concat();
            assert_eq!(payloads, ["receipt"]);
        }
    }

    #[test]
    fn title_scanner_reuses_bounded_payload_storage() {
        let mut scanner = OscTitleScanner::new();
        assert_eq!(
            scanner.feed(b"plain\x1b]2;first title\x07tail"),
            ["first title"]
        );
        let capacity = scanner.payload.capacity();
        assert!(capacity >= b"first title".len());
        for _ in 0..100 {
            assert_eq!(scanner.feed(b"\x1b]2;next title\x07"), ["next title"]);
            assert_eq!(scanner.payload.capacity(), capacity);
            assert!(scanner.payload.is_empty());
        }
    }

    #[test]
    fn reserved_filter_removes_split_private_titles_and_preserves_other_bytes() {
        let mut filter = ReservedOscFilter::new("private:");
        let first = filter.feed(b"before\x1b]2;priv");
        assert_eq!(first.visible, b"before");
        assert!(first.reserved_payloads.is_empty());

        let second = filter.feed(b"ate:token:session\x07middle\x1b]0;ordinary\x07after");
        assert_eq!(
            second.visible, b"middle\x1b]0;ordinary\x07after",
            "only the exact reserved channel is removed"
        );
        assert_eq!(second.reserved_payloads, ["token:session"]);
        assert!(filter.finish().is_empty());
    }

    #[test]
    fn reserved_filter_drops_malformed_private_frames_without_leaking_payload() {
        let mut filter = ReservedOscFilter::new("private:");
        let filtered = filter.feed(b"a\x1b]2;private:secret\x1bXignored\x07b");
        assert_eq!(filtered.visible, b"ab");
        assert!(filtered.reserved_payloads.is_empty());

        let incomplete = filter.feed(b"c\x1b]2;private:another-secret");
        assert_eq!(incomplete.visible, b"c");
        assert!(incomplete.reserved_payloads.is_empty());
        assert!(filter.finish().is_empty());
    }

    #[test]
    fn reserved_filter_treats_bel_after_escape_as_a_private_terminator() {
        let mut filter = ReservedOscFilter::new("private:");
        let filtered = filter.feed(b"a\x1b]2;private:token\x1b\x07visible");
        assert_eq!(filtered.visible, b"avisible");
        assert!(filtered.reserved_payloads.is_empty());
    }

    #[test]
    fn overlong_reserved_frame_never_leaks_its_tail() {
        let mut filter = ReservedOscFilter::new("private:");
        let mut frame = Vec::from(&b"before\x1b]2;private:"[..]);
        frame.extend(std::iter::repeat_n(b'x', 2_048));
        frame.extend_from_slice(b"\x07after");
        let filtered = filter.feed(&frame);
        assert_eq!(filtered.visible, b"beforeafter");
        assert!(filtered.reserved_payloads.is_empty());
    }

    #[test]
    fn reserved_filter_flushes_incomplete_unrelated_escape_at_eof() {
        let mut filter = ReservedOscFilter::new("private:");
        let filtered = filter.feed(b"plain\x1b]2;other");
        assert_eq!(filtered.visible, b"plain\x1b]2;other");
        assert!(filtered.reserved_payloads.is_empty());

        let pending = filter.feed(b"\x1b");
        assert!(pending.visible.is_empty());
        assert_eq!(filter.finish(), b"\x1b");
    }
}
