//! Cursor-aware composer primitives for the interactive CLI.
//!
//! The legacy 0.1.169 surface measured its cursor in UTF-16/graphemes because
//! Ink ran in JavaScript. Rust keeps a UTF-8 byte offset internally, but every
//! public movement operation lands on a Unicode scalar boundary and therefore
//! never splits user input or pasted text.

use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Composer {
    text: String,
    cursor: usize,
}

impl Composer {
    #[must_use]
    pub(crate) fn new(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    #[must_use]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub(crate) const fn cursor(&self) -> usize {
        self.cursor
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub(crate) fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub(crate) fn replace(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
    }

    pub(crate) fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub(crate) fn insert(&mut self, value: &str) {
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
    }

    pub(crate) fn backspace(&mut self) {
        let start = previous_boundary(&self.text, self.cursor);
        if start < self.cursor {
            self.text.drain(start..self.cursor);
            self.cursor = start;
        }
    }

    pub(crate) fn delete(&mut self) {
        let end = next_boundary(&self.text, self.cursor);
        if end > self.cursor {
            self.text.drain(self.cursor..end);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub(crate) fn move_left(&mut self) {
        self.cursor = previous_boundary(&self.text, self.cursor);
    }

    pub(crate) fn move_right(&mut self) {
        self.cursor = next_boundary(&self.text, self.cursor);
    }

    pub(crate) fn move_start(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub(crate) fn move_line_start(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
    }

    pub(crate) fn move_line_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |offset| self.cursor + offset);
    }

    pub(crate) fn move_word_left(&mut self) {
        let head = self.text[..self.cursor].trim_end();
        self.cursor = head
            .trim_end_matches(|character: char| !character.is_whitespace())
            .len();
    }

    pub(crate) fn move_word_right(&mut self) {
        let tail = self.text[self.cursor..].trim_start();
        self.cursor = self.text.len()
            - tail
                .trim_start_matches(|character: char| !character.is_whitespace())
                .len();
    }
}

/// The cursor movement a key makes in the composer, if it only moves the
/// cursor. Super wins over Alt, and both over the unmodified key, as the
/// interactive key handler has always ordered them. End is not here: it also
/// returns a fullscreen transcript to the newest output.
pub(crate) fn navigation(code: KeyCode, modifiers: KeyModifiers) -> Option<fn(&mut Composer)> {
    let control = modifiers.contains(KeyModifiers::CONTROL);
    let alt = modifiers.contains(KeyModifiers::ALT);
    let command = modifiers.contains(KeyModifiers::SUPER);
    let movement: fn(&mut Composer) = match code {
        KeyCode::Char('a' | 'A') if control => Composer::move_line_start,
        KeyCode::Char('e' | 'E') if control => Composer::move_line_end,
        KeyCode::Left if command => Composer::move_line_start,
        KeyCode::Right if command => Composer::move_line_end,
        KeyCode::Up if command => Composer::move_start,
        KeyCode::Down if command => Composer::move_end,
        KeyCode::Up if alt => Composer::move_line_start,
        KeyCode::Down if alt => Composer::move_line_end,
        KeyCode::Left | KeyCode::Char('b' | 'B') if alt => Composer::move_word_left,
        KeyCode::Right | KeyCode::Char('f' | 'F') if alt => Composer::move_word_right,
        KeyCode::Home => Composer::move_start,
        KeyCode::Left => Composer::move_left,
        KeyCode::Right => Composer::move_right,
        _ => return None,
    };
    Some(movement)
}

fn previous_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(offset, _)| cursor + offset)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::{Composer, navigation};

    #[test]
    fn insertion_deletion_and_paste_follow_cursor() {
        let mut composer = Composer::new("aéc".into());
        composer.move_left();
        composer.insert(" 🦀 ");
        assert_eq!(composer.text(), "aé 🦀 c");
        composer.backspace();
        composer.delete();
        assert_eq!(composer.text(), "aé 🦀");
        assert!(composer.text().is_char_boundary(composer.cursor()));
    }

    #[test]
    fn line_and_document_navigation_match_multiline_editor() {
        let mut composer = Composer::new("one\ntwo three\nfour".into());
        composer.move_word_left();
        assert_eq!(composer.cursor(), "one\ntwo three\n".len());
        composer.move_line_start();
        assert_eq!(composer.cursor(), "one\ntwo three\n".len());
        composer.move_start();
        composer.move_word_right();
        assert_eq!(composer.cursor(), 3);
        composer.move_end();
        composer.move_line_start();
        assert_eq!(composer.cursor(), "one\ntwo three\n".len());
        composer.move_line_end();
        assert_eq!(composer.cursor(), composer.text().len());
    }

    #[test]
    fn navigation_keys_keep_their_modifier_precedence() {
        let cursor_after = |code, modifiers| {
            let mut composer = Composer::new("one two\nthree four".into());
            composer.move_left();
            navigation(code, modifiers).map(|movement| {
                movement(&mut composer);
                composer.cursor()
            })
        };
        let (line_start, end) = ("one two\n".len(), "one two\nthree four".len());
        let both = KeyModifiers::SUPER | KeyModifiers::ALT;
        assert_eq!(cursor_after(KeyCode::Left, both), Some(line_start));
        assert_eq!(cursor_after(KeyCode::Up, both), Some(0));
        assert_eq!(cursor_after(KeyCode::Down, KeyModifiers::SUPER), Some(end));
        assert_eq!(
            cursor_after(KeyCode::Up, KeyModifiers::ALT),
            Some(line_start)
        );
        assert_eq!(cursor_after(KeyCode::Down, KeyModifiers::ALT), Some(end));
        assert_eq!(cursor_after(KeyCode::Right, KeyModifiers::SUPER), Some(end));
        assert_eq!(
            cursor_after(KeyCode::Char('B'), KeyModifiers::ALT),
            Some("one two\nthree ".len())
        );
        assert_eq!(
            cursor_after(KeyCode::Char('f'), KeyModifiers::ALT),
            Some(end)
        );
        assert_eq!(
            cursor_after(
                KeyCode::Char('a'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            ),
            Some(line_start)
        );
        assert_eq!(
            cursor_after(KeyCode::Char('E'), KeyModifiers::CONTROL),
            Some(end)
        );
        assert_eq!(cursor_after(KeyCode::Home, KeyModifiers::SHIFT), Some(0));
        assert_eq!(
            cursor_after(KeyCode::Left, KeyModifiers::CONTROL),
            Some(end - 2)
        );
        assert_eq!(cursor_after(KeyCode::Right, KeyModifiers::NONE), Some(end));
        // Keys that do more than move the cursor stay with the key handler.
        for (code, modifiers) in [
            (KeyCode::Up, KeyModifiers::NONE),
            (KeyCode::Down, KeyModifiers::CONTROL),
            (KeyCode::End, KeyModifiers::NONE),
            (KeyCode::Char('a'), KeyModifiers::NONE),
            (KeyCode::Char('a'), KeyModifiers::SUPER),
            (KeyCode::Char('b'), KeyModifiers::CONTROL),
            (KeyCode::Char('u'), KeyModifiers::CONTROL),
        ] {
            assert!(
                navigation(code, modifiers).is_none(),
                "{code:?} {modifiers:?}"
            );
        }
    }

    #[test]
    fn take_and_clear_reset_cursor() {
        let mut composer = Composer::new("draft".into());
        assert_eq!(composer.take(), "draft");
        assert_eq!(composer.cursor(), 0);
        composer.insert("next");
        composer.clear();
        assert!(composer.is_empty());
        assert_eq!(composer.cursor(), 0);
    }
}
