//! Byte-bounded text clipping that never splits a UTF-8 character.
//!
//! `str::floor_char_boundary` would do this, but it needs Rust 1.91 and the
//! workspace MSRV is 1.88.

/// The longest prefix of `text` that is at most `bytes` long.
pub(crate) fn head(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The longest suffix of `text` that is at most `bytes` long.
pub(crate) fn tail(text: &str, bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// Deterministic strings mixing one- to four-byte characters, line endings,
/// control characters and sequences NFC composes, for comparing clippers.
#[cfg(test)]
pub(crate) fn sample_texts() -> Vec<String> {
    const PIECES: [&str; 18] = [
        "a",
        "Z",
        " ",
        "\n",
        "\r\n",
        "\r",
        "\u{e9}",
        "e\u{301}",
        "\u{20ac}",
        "\u{1f600}",
        "\u{7}",
        "\u{85}",
        "\u{fb03}",
        "\u{d55c}",
        "\u{1100}\u{1161}",
        "\0",
        "\u{7f}",
        "\t",
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % 1_000_003).unwrap()
    };
    let mut texts = vec![String::new()];
    texts.extend(PIECES.iter().map(|piece| (*piece).to_owned()));
    for _ in 0..1_500 {
        let length = next() % 40;
        texts.push((0..length).map(|_| PIECES[next() % PIECES.len()]).collect());
    }
    texts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The char-by-char prefix the crate's callers used to build.
    fn legacy_head(text: &str, bytes: usize) -> String {
        let mut output = String::new();
        for character in text.chars() {
            if output.len().saturating_add(character.len_utf8()) > bytes {
                break;
            }
            output.push(character);
        }
        output
    }

    /// The end index `away::refinement_feedback` used to compute.
    fn legacy_prefix_end(text: &str, bytes: usize) -> usize {
        text.char_indices()
            .map(|(index, character)| index + character.len_utf8())
            .take_while(|end| *end <= bytes)
            .last()
            .unwrap_or(0)
    }

    fn legacy_tail(text: &str, bytes: usize) -> String {
        let mut kept = Vec::new();
        let mut length = 0_usize;
        for character in text.chars().rev() {
            if length + character.len_utf8() > bytes {
                break;
            }
            length += character.len_utf8();
            kept.push(character);
        }
        kept.into_iter().rev().collect()
    }

    #[test]
    fn clipping_matches_char_by_char_clipping_at_every_budget() {
        for text in sample_texts() {
            for bytes in 0..=text.len() + 2 {
                assert_eq!(
                    head(&text, bytes),
                    legacy_head(&text, bytes),
                    "{text:?} {bytes}"
                );
                assert_eq!(
                    head(&text, bytes).len(),
                    legacy_prefix_end(&text, bytes),
                    "{text:?} {bytes}"
                );
                assert_eq!(
                    tail(&text, bytes),
                    legacy_tail(&text, bytes),
                    "{text:?} {bytes}"
                );
            }
            assert_eq!(head(&text, usize::MAX), text);
            assert_eq!(tail(&text, usize::MAX), text);
        }
    }

    #[test]
    fn clipping_keeps_whole_characters() {
        assert_eq!(head("🙂🙂", 5), "🙂");
        assert_eq!(tail("🙂🙂", 5), "🙂");
        assert_eq!(tail("a🙂b", 3), "b");
        assert_eq!(tail("short", 32), "short");
    }
}
