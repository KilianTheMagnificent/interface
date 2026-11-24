//! POSIX shell-word quoting for the command text engines hand the approval
//! policy.
//!
//! Every engine joins an argv into one line for the policy to read, so each
//! must quote an element the same way or the policy sees one command two ways.

use std::borrow::Cow;

/// A byte that means itself unquoted in any POSIX shell word.
#[must_use]
pub fn is_plain_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte)
}

/// One argv element as a POSIX shell word, so a quote-aware reader of the
/// joined command sees the same argument boundaries the provider will exec.
#[must_use]
pub fn shell_word(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty() && word.bytes().all(is_plain_byte);
    if plain {
        Cow::Borrowed(word)
    } else {
        Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{is_plain_byte, shell_word};

    /// The ACP adapter's copy, which inlined the plain-byte set.
    fn acp_copy(word: &str) -> Cow<'_, str> {
        let plain = !word.is_empty()
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte));
        if plain {
            Cow::Borrowed(word)
        } else {
            Cow::Owned(format!("'{}'", word.replace('\'', r"'\''")))
        }
    }

    #[test]
    fn quoting_matches_the_adapter_copies_it_replaced() {
        for word in [
            "",
            "ls",
            "-la",
            "a/b.c:d,e=f+g%h@i_j",
            "two words",
            "it's",
            "'",
            "$HOME",
            "`id`",
            "a\\b",
            "tab\there",
            "new\nline",
            "é",
            "*",
            "semi;colon",
        ] {
            assert_eq!(shell_word(word), acp_copy(word), "{word:?}");
        }
        assert!(matches!(shell_word("plain"), Cow::Borrowed("plain")));
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn plain_bytes_are_the_posix_portable_unquoted_set() {
        let plain: Vec<u8> = (0..=u8::MAX).filter(|byte| is_plain_byte(*byte)).collect();
        let mut expected: Vec<u8> = (b'0'..=b'9')
            .chain(b'A'..=b'Z')
            .chain(b'a'..=b'z')
            .chain(b"_@%+=:,./-".iter().copied())
            .collect();
        expected.sort_unstable();
        assert_eq!(plain, expected);
    }
}
