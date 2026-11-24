//! Recognition of a rendered CLI screen for workspace sleep: a bounded
//! screen, an empty message prompt, and the terminal's own reports. The
//! renderer supplies the rows; Rust alone judges them.
use interface_contracts::{
    terminal::TerminalLaunchProfileId as Profile,
    terminal_controls::TerminalControlScreenV1 as Screen,
};

pub(crate) fn valid_screen(screen: &Screen) -> bool {
    !screen.lines.is_empty()
        && screen.lines.len() <= 100
        && screen.cursor_row < screen.lines.len()
        && screen.cursor_column <= 1_000
        && screen
            .lines
            .iter()
            .all(|line| line.len() <= 4_000 && !line.chars().any(char::is_control))
}

pub(crate) fn idle(screen: &Screen, profile: Profile) -> bool {
    if !screen.at_bottom || !screen.bracketed_paste || yes_no_approval(screen) {
        return false;
    }
    let line = &screen.lines[screen.cursor_row];
    let prefix: String = line.chars().take(screen.cursor_column).collect();
    let prompt = if profile == Profile::Codex {
        "›"
    } else {
        "❯"
    };
    prefix.trim() == prompt
        && (screen.ghost_text
            || line
                .chars()
                .skip(screen.cursor_column)
                .all(char::is_whitespace))
        && !screen
            .lines
            .iter()
            .skip(screen.cursor_row.saturating_sub(6))
            .any(|line| {
                let lower = line.to_lowercase();
                lower.contains("allow once")
                    || lower.contains("approve once")
                    || lower.contains("esc to cancel")
            })
        && !screen.lines.iter().skip(screen.cursor_row).any(|line| {
            let lower = line.to_lowercase();
            lower.contains("enter to confirm")
                || lower.contains("esc to cancel")
                || numbered(line).is_some()
        })
}

fn yes_no_approval(screen: &Screen) -> bool {
    let Some(previous) = screen.cursor_row.checked_sub(1) else {
        return false;
    };
    let has_keys = screen.lines[previous]
        .split(['[', '('])
        .skip(1)
        .filter_map(|part| part.split_once([']', ')']))
        .filter_map(|(keys, _)| keys.split_once('/'))
        .any(|(yes, no)| {
            matches!(yes.trim().to_ascii_lowercase().as_str(), "y" | "yes")
                && matches!(no.trim().to_ascii_lowercase().as_str(), "n" | "no")
        });
    if !has_keys {
        return false;
    }
    screen.lines[screen.cursor_row.saturating_sub(6)..screen.cursor_row]
        .iter()
        .any(|line| {
            let text = line.to_lowercase();
            [
                "would you like to",
                "do you want to",
                "select an option",
                "permission required",
                "permission request",
            ]
            .iter()
            .any(|phrase| text.contains(phrase))
        })
}

fn numbered(line: &str) -> Option<(u8, &str)> {
    let line = line.trim().trim_start_matches(['›', '❯']).trim();
    let (number, label) = line.split_once(". ")?;
    let number: u8 = number.parse().ok()?;
    (number > 0 && number <= 9).then_some((number, label.trim()))
}

/// Whether a write is only the terminal's own report: focus in or out, or a
/// capability reply. xterm sends these without anyone typing, so these exact
/// writes are not human input; keys, including a report with a key after it,
/// are.
pub(crate) fn is_terminal_report(bytes: &[u8]) -> bool {
    matches!(bytes, b"\x1b[I" | b"\x1b[O")
        || bytes.starts_with(b"\x1b[?")
        || bytes.starts_with(b"\x1b[>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(lines: &[&str], cursor_row: usize) -> Screen {
        Screen {
            lines: lines.iter().map(|s| (*s).into()).collect(),
            cursor_row,
            cursor_column: 2,
            at_bottom: true,
            bracketed_paste: true,
            ghost_text: false,
        }
    }

    #[test]
    fn an_empty_prompt_is_idle_and_drafts_approvals_and_menus_are_not() {
        assert!(idle(
            &screen(&["› ", "gpt-6-astra high · /repo"], 0),
            Profile::Codex
        ));
        assert!(idle(
            &screen(&["❯ ", "✻ Opus 5 │ 0 / 1.0M tokens"], 0),
            Profile::Claude
        ));
        for current in [
            screen(&["› native draft"], 0),
            screen(&["› 1. Approve", "2. Deny"], 0),
            screen(&["Allow once or always", "› "], 1),
            screen(&["shell$ "], 0),
        ] {
            assert!(!idle(&current, Profile::Codex), "{:?}", current.lines);
        }
    }

    #[test]
    fn a_yes_no_approval_prompt_is_never_idle() {
        for (profile, prompt, footer) in [
            (Profile::Claude, "❯ ", "✻ Opus 5 │ 0 / 1.0M tokens"),
            (Profile::Codex, "› ", "gpt-6-astra high · /repo"),
        ] {
            for question in [
                "Do you want to overwrite it? (y/n)",
                "Would you like to run this command? [yes/no]",
            ] {
                let current = screen(&[question, prompt, footer], 1);
                assert!(!idle(&current, profile), "{question}");
            }
        }
    }

    #[test]
    fn screens_are_bounded_and_free_of_control_characters() {
        let ready = || screen(&["› ", "gpt-6-astra high · /repo"], 0);
        assert!(valid_screen(&ready()));
        let mut invalid = ready();
        invalid.lines[0] = "\x1b[31m› ".into();
        assert!(!valid_screen(&invalid));
        invalid = ready();
        invalid.cursor_row = 100;
        assert!(!valid_screen(&invalid));
    }

    #[test]
    fn focus_and_capability_reports_are_not_keys() {
        for report in [
            b"\x1b[I".as_slice(),
            b"\x1b[O",
            b"\x1b[?1;2c",
            b"\x1b[>0;1c",
        ] {
            assert!(is_terminal_report(report));
        }
        for input in [b"\r".as_slice(), b"\x1b", b"3", b"\x1b[I\r", b"\x1b[O3"] {
            assert!(!is_terminal_report(input));
        }
    }
}
