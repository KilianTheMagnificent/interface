//! Bounded ring of raw terminal output that renders as plain text lines.

use std::collections::VecDeque;

/// Keeps the newest `capacity` bytes of a terminal's output.
#[derive(Debug)]
pub struct TailBuffer {
    capacity: usize,
    bytes: VecDeque<u8>,
    total: u64,
}

impl TailBuffer {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            bytes: VecDeque::with_capacity(capacity.min(4096)),
            total: 0,
        }
    }

    /// Every byte ever pushed, counted from the first push.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The kept bytes, oldest first: the last ones before [`Self::total`].
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        let (head, rest) = self.bytes.as_slices();
        [head, rest].concat()
    }

    /// Appends output, dropping the oldest bytes past the capacity.
    pub fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len() as u64);
        let bytes = if bytes.len() > self.capacity {
            &bytes[bytes.len() - self.capacity..]
        } else {
            bytes
        };
        let overflow = (self.bytes.len() + bytes.len()).saturating_sub(self.capacity);
        self.bytes.drain(..overflow);
        self.bytes.extend(bytes);
    }

    /// The last `n` lines as plain text: ANSI CSI/OSC sequences stripped,
    /// a bare carriage return overwrites the current line, and BEL is dropped.
    #[must_use]
    pub fn tail_lines(&self, n: usize) -> String {
        if n == 0 {
            return String::new();
        }
        let (head, rest) = self.bytes.as_slices();
        let mut raw = Vec::with_capacity(self.bytes.len());
        raw.extend_from_slice(head);
        raw.extend_from_slice(rest);
        let text = String::from_utf8_lossy(&raw);
        let lines = plain_lines(&text, n);
        let mut output = String::new();
        for (index, line) in lines.iter().enumerate() {
            if index != 0 {
                output.push('\n');
            }
            output.push_str(line);
        }
        output
    }
}

fn plain_lines(text: &str, limit: usize) -> VecDeque<String> {
    let mut lines = VecDeque::from([String::new()]);
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\x1b' => skip_escape(&mut chars),
            '\x07' => {}
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    next_line(&mut lines, limit);
                } else {
                    // ponytail: a bare CR replaces the whole line rather than
                    // overwriting column by column; good enough for progress bars.
                    lines.back_mut().map(String::clear);
                }
            }
            '\n' => next_line(&mut lines, limit),
            other => lines
                .back_mut()
                .map(|line| line.push(other))
                .unwrap_or_default(),
        }
    }
    lines
}

fn next_line(lines: &mut VecDeque<String>, limit: usize) {
    // Reuse an evicted line's allocation, so a long transcript does not
    // allocate one String for every line merely to return its last few.
    let mut line = if lines.len() == limit {
        lines.pop_front().unwrap_or_default()
    } else {
        String::new()
    };
    line.clear();
    lines.push_back(line);
}

fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        // CSI: parameters and intermediates, then one final byte in 0x40..=0x7E.
        Some('[') => {
            for character in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&character) {
                    break;
                }
            }
        }
        // OSC: until BEL or ESC \.
        Some(']') => {
            while let Some(character) = chars.next() {
                if character == '\x07' {
                    break;
                }
                if character == '\x1b' {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    break;
                }
            }
        }
        // Two-character escapes (ESC 7, ESC =) and charset designations
        // (ESC ( B) with intermediates in 0x20..=0x2F.
        Some(character) if ('\x20'..='\x2f').contains(&character) => {
            for character in chars.by_ref() {
                if !('\x20'..='\x2f').contains(&character) {
                    break;
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_oldest_bytes_at_capacity() {
        let mut tail = TailBuffer::new(8);
        tail.push(b"abcdef\n");
        tail.push(b"gh\nij");
        assert_eq!(tail.tail_lines(10), "ef\ngh\nij");
        tail.push(b"0123456789xy");
        assert_eq!(tail.tail_lines(10), "456789xy");
    }

    #[test]
    fn counts_every_pushed_byte_and_keeps_the_newest() {
        let mut tail = TailBuffer::new(8);
        assert_eq!((tail.total(), tail.bytes()), (0, Vec::new()));
        tail.push(b"abcdef");
        tail.push(b"ghij");
        assert_eq!((tail.total(), tail.bytes()), (10, b"cdefghij".to_vec()));
        tail.push(b"0123456789xy");
        assert_eq!((tail.total(), tail.bytes()), (22, b"456789xy".to_vec()));
    }

    #[test]
    fn strips_ansi_and_treats_carriage_return_as_overwrite() {
        let mut tail = TailBuffer::new(1024);
        tail.push(b"\x1b[31mred\x1b[0m\x07\n\x1b]0;title\x07one\r\ntwo\rthree\n");
        assert_eq!(tail.tail_lines(10), "red\none\nthree\n");
        assert_eq!(tail.tail_lines(2), "three\n");
    }

    #[test]
    fn tail_limits_preserve_escape_and_carriage_return_semantics() {
        let text = "old\n\x1b]0;hidden\ninside title\x1b\\kept\rreplaced\r\nlast\n";
        let mut tail = TailBuffer::new(4096);
        tail.push(text.as_bytes());
        assert_eq!(tail.tail_lines(0), "");
        assert_eq!(tail.tail_lines(1), "");
        assert_eq!(tail.tail_lines(2), "last\n");
        assert_eq!(tail.tail_lines(3), "replaced\nlast\n");
        assert_eq!(tail.tail_lines(usize::MAX), "old\nreplaced\nlast\n");
        assert_eq!(plain_lines(&"line\n".repeat(100_000), 3).len(), 3);
    }
}
