//! Splits streamed assistant text into speakable sentences.

/// Force a speech chunk even when a provider streams unpunctuated text.
pub const MAX_PENDING_SENTENCE_BYTES: usize = 16 * 1024;

/// Accumulates text deltas and emits complete sentences.
///
/// A sentence ends at `.`, `!` or `?` followed by whitespace, or at a newline.
/// A punctuation boundary is skipped while the pending chunk is shorter than
/// `min_chars`; a newline always ends the chunk. Punctuation at the very end
/// of the buffered text is not a boundary until [`flush`](Self::flush): the
/// next delta may continue the token (`3.` then `14`).
#[derive(Debug)]
pub struct SentenceChunker {
    min_chars: usize,
    buffer: String,
    chars: usize,
}

impl SentenceChunker {
    #[must_use]
    pub fn new(min_chars: usize) -> Self {
        Self {
            min_chars,
            buffer: String::new(),
            chars: 0,
        }
    }

    /// Appends a delta and returns every sentence it completed, in order.
    pub fn push(&mut self, delta: &str) -> Vec<String> {
        let mut out = Vec::new();
        for character in delta.chars() {
            let sentence_end = character.is_whitespace()
                && self.buffer.ends_with(['.', '!', '?'])
                && self.chars >= self.min_chars;
            if character == '\n' || sentence_end {
                out.extend(self.flush());
            } else {
                if self.buffer.len() + character.len_utf8() > MAX_PENDING_SENTENCE_BYTES {
                    out.extend(self.flush());
                }
                if !self.buffer.is_empty() || !character.is_whitespace() {
                    self.buffer.push(character);
                    self.chars += 1;
                }
            }
        }
        out
    }

    /// Returns whatever is still buffered, trimmed, and clears the buffer.
    pub fn flush(&mut self) -> Option<String> {
        self.chars = 0;
        let rest = std::mem::take(&mut self.buffer);
        let rest = rest.trim();
        (!rest.is_empty()).then(|| rest.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_boundaries_do_not_change_sentences() {
        let text = "  Yes. I agree!\u{2003}Pi is 3.14.\n次は日本語。 No? Done.";
        for minimum in [0, 4, 20, 100] {
            let mut whole = SentenceChunker::new(minimum);
            let mut expected = whole.push(text);
            expected.extend(whole.flush());
            let mut streamed = SentenceChunker::new(minimum);
            let mut actual = Vec::new();
            for character in text.chars() {
                actual.extend(streamed.push(character.encode_utf8(&mut [0; 4])));
            }
            actual.extend(streamed.flush());
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn unpunctuated_unicode_is_bounded_and_lossless() {
        let text = "界".repeat(MAX_PENDING_SENTENCE_BYTES);
        let mut chunker = SentenceChunker::new(usize::MAX);
        let mut chunks = chunker.push(&text);
        assert!(chunker.buffer.len() <= MAX_PENDING_SENTENCE_BYTES);
        chunks.extend(chunker.flush());
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= MAX_PENDING_SENTENCE_BYTES)
        );
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn splits_at_sentence_punctuation_and_flushes_the_remainder() {
        let mut chunker = SentenceChunker::new(4);
        let mut chunks = chunker.push("Hello there. How are");
        chunks.extend(chunker.push(" you? Fine"));
        chunks.extend(chunker.flush());
        assert_eq!(chunks, ["Hello there.", "How are you?", "Fine"]);
        assert_eq!(chunker.flush(), None);
    }

    #[test]
    fn min_chars_holds_short_sentences_together() {
        let mut chunker = SentenceChunker::new(20);
        assert!(chunker.push("Yes. I did it. ").is_empty());
        assert_eq!(chunker.flush().as_deref(), Some("Yes. I did it."));
    }

    #[test]
    fn newline_always_ends_a_chunk_and_trailing_punctuation_waits() {
        let mut chunker = SentenceChunker::new(20);
        assert_eq!(chunker.push("Ok.\n\nPi is 3."), ["Ok."]);
        assert!(chunker.push("14 exactly. ").is_empty());
        assert_eq!(
            chunker.push("Thanks a lot for asking!\n"),
            ["Pi is 3.14 exactly. Thanks a lot for asking!"]
        );
        assert_eq!(chunker.flush(), None);
    }
}
