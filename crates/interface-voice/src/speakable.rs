//! Turns markdown-flavoured assistant text into plain speakable prose.

const CODE_OMITTED: &str = "code omitted";

/// Strips markdown markers a speech synthesizer would read aloud.
///
/// Fenced code blocks become the words `code omitted`; headings, list
/// markers, emphasis, backticks and link syntax are removed; whitespace is
/// collapsed to single spaces.
#[must_use]
pub fn speakable(text: &str) -> String {
    let mut words = Vec::new();
    for line in without_fences(text).lines() {
        let line = strip_line_markers(line);
        let line = strip_links(&line);
        words.extend(
            line.split_whitespace()
                .map(strip_inline_markers)
                .filter(|word| !word.is_empty()),
        );
    }
    words.join(" ")
}

fn without_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        out.push_str(&rest[..start]);
        out.push_str(CODE_OMITTED);
        // ponytail: an unterminated fence swallows the rest of the text.
        let Some(end) = rest[start + 3..].find("```") else {
            return out;
        };
        rest = &rest[start + 3 + end + 3..];
    }
    out.push_str(rest);
    out
}

fn strip_line_markers(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix('#') {
        let rest = rest.trim_start_matches('#');
        if rest.starts_with(' ') || rest.is_empty() {
            return rest.to_owned();
        }
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return rest.to_owned();
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest.to_owned();
        }
    }
    trimmed.to_owned()
}

fn strip_links(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("](") else {
            break;
        };
        let Some(end) = rest[open + close..].find(')') else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&rest[open + 1..open + close]);
        rest = &rest[open + close + end + 1..];
    }
    out.push_str(rest);
    out
}

fn strip_inline_markers(word: &str) -> String {
    let stripped: String = word
        .chars()
        .filter(|character| !matches!(character, '*' | '`'))
        .collect();
    // Underscores only mark emphasis at word edges; inside a word they are
    // part of an identifier such as `read_pane`.
    stripped.trim_matches('_').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_emphasis_backticks_and_fenced_code() {
        assert_eq!(
            speakable("**Done.** See `foo.rs`.\n\n```rust\nlet x=1;\n```"),
            "Done. See foo.rs. code omitted"
        );
    }

    #[test]
    fn strips_headings_lists_and_links_but_keeps_identifiers() {
        assert_eq!(
            speakable(
                "# Title\n- one\n* two\n1. three\nsee [docs](https://x.y/z) and _read_pane_ now"
            ),
            "Title one two three see docs and read_pane now"
        );
        assert_eq!(speakable("```\nunterminated"), "code omitted");
    }
}
