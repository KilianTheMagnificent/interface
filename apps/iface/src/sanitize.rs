/// Makes untrusted text inert before it reaches a terminal.
///
/// This mirrors the legacy character scanner: CSI and terminal control
/// strings are consumed in full, carriage returns are removed, and only tabs
/// and newlines survive from the C0/C1 control ranges.
#[must_use]
pub fn for_display(value: &str) -> String {
    // Safe text, ASCII or not (an em dash, a curly quote, an emoji), is
    // copied as is instead of walking the stripper character by character.
    if is_ascii_display_safe(value) || is_display_safe(value) {
        return value.to_owned();
    }
    strip_controls(value)
}

/// Returns whether `value` already equals [`for_display`] without allocating.
///
/// This is a Unicode scalar scan, not an ASCII-only predicate: printable
/// non-ASCII text such as `café` is safe, matching `for_display`.
#[must_use]
pub fn is_display_safe(value: &str) -> bool {
    value.chars().all(is_safe_scalar)
}

fn is_ascii_display_safe(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| (0x20..0x7f).contains(&byte) || byte == b'\n' || byte == b'\t')
}

const fn is_safe_scalar(character: char) -> bool {
    let code = character as u32;
    matches!(character, '\n' | '\t')
        || (code >= 0x20 && code != 0x7f && (code < 0x80 || code > 0x9f))
}

fn strip_controls(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        let character = value[index..]
            .chars()
            .next()
            .expect("index is a scalar boundary");
        let code = character as u32;
        let next_index = index + character.len_utf8();
        if code == 0x1b {
            let next = value[next_index..].chars().next();
            match next {
                Some('[') => index = consume_csi(value, next_index + '['.len_utf8()),
                Some(']' | 'P' | '_' | '^' | 'X') => {
                    let marker = next.expect("matched control-string introducer");
                    index = consume_control_string(
                        value,
                        next_index + marker.len_utf8(),
                        next == Some(']'),
                    );
                }
                _ => index = next_index,
            }
            continue;
        }
        if code == 0x9b {
            index = consume_csi(value, next_index);
            continue;
        }
        if matches!(code, 0x9d | 0x90 | 0x9f | 0x9e | 0x98) {
            index = consume_control_string(value, next_index, code == 0x9d);
            continue;
        }
        if is_safe_scalar(character) {
            safe.push(character);
        }
        index = next_index;
    }
    safe
}

fn consume_csi(value: &str, start: usize) -> usize {
    value[start..]
        .char_indices()
        .find(|(_, character)| (0x40..=0x7e).contains(&(*character as u32)))
        .map_or(value.len(), |(offset, character)| {
            start + offset + character.len_utf8()
        })
}

fn consume_control_string(value: &str, start: usize, bel_terminates: bool) -> usize {
    let mut index = start;
    while index < value.len() {
        let character = value[index..]
            .chars()
            .next()
            .expect("index is a scalar boundary");
        let code = character as u32;
        if (bel_terminates && code == 0x07) || code == 0x9c {
            return index + character.len_utf8();
        }
        if code == 0x1b {
            let escape_end = index + character.len_utf8();
            if value[escape_end..].starts_with('\\') {
                return escape_end + 1;
            }
        }
        index += character.len_utf8();
    }
    value.len()
}

#[cfg(test)]
mod tests {
    use super::{for_display, is_display_safe};

    #[test]
    fn strips_terminal_controls_without_losing_plain_text() {
        assert_eq!(for_display("a\r\n\u{1b}[31mred\u{1b}[0m"), "a\nred");
        assert_eq!(for_display("x\u{1b}]0;secret\u{7}y"), "xy");
        assert_eq!(for_display("a\u{1b}qb"), "aqb");
    }

    #[test]
    fn consumes_unterminated_control_strings() {
        assert_eq!(for_display("safe\u{1b}]hidden\nmore"), "safe");
    }

    #[test]
    fn ascii_and_printable_unicode_are_display_safe() {
        assert!(is_display_safe("plain ASCII\tline\n"));
        assert!(is_display_safe("café"));
        assert!(is_display_safe("漢字🎉"));
        assert_eq!(for_display("café"), "café");
        assert!(!is_display_safe("a\u{1b}[31mb"));
        assert!(!is_display_safe("a\r b"));
        assert!(!is_display_safe("token\u{7f}"));
    }
}
