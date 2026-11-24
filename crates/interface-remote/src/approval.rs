//! Conservative projection of a rendered, current CLI permission dialog.
//! Only known single-use choices are offered. Unknown/custom layouts stay on Mac.
use crate::replay::StreamKey;
use interface_contracts::{
    remote_approval::ApprovalV4, terminal::TerminalLaunchProfileId as Profile,
    terminal_controls::TerminalControlScreenV1 as Screen,
};
use sha2::{Digest, Sha256};

pub(crate) struct RecognizedApproval {
    pub view: ApprovalV4,
    pub allow: Option<Vec<u8>>,
}

fn choice(line: &str) -> Option<&str> {
    let line = line.trim().trim_start_matches(['›', '❯']).trim();
    let (number, label) = line.split_once(". ")?;
    (number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9')).then_some(label.trim())
}

pub(crate) fn recognize(
    profile: Profile,
    key: StreamKey,
    cursor: u64,
    screen: &Screen,
) -> Option<RecognizedApproval> {
    if !screen.at_bottom
        || screen.lines.is_empty()
        || screen.lines.len() > 100
        || screen.cursor_row >= screen.lines.len()
        || screen.cursor_column > 1000
        || screen
            .lines
            .iter()
            .any(|l| l.len() > 4000 || l.chars().any(char::is_control))
        || screen.lines.iter().map(String::len).sum::<usize>() > 12000
    {
        return None;
    }
    let lines: Vec<_> = screen.lines.iter().map(|l| l.trim()).collect();
    let first = lines.iter().position(|l| choice(l).is_some())?;
    let footer = lines.iter().rposition(|l| {
        let l = l.to_lowercase();
        l.contains("esc to cancel") && l.contains("enter to")
    })?;
    if footer <= first || lines[footer + 1..].iter().any(|l| !l.is_empty()) {
        return None;
    }
    // Reject multiple selections and comment fields; Enter must never confirm a
    // persistent permission or type into an unrelated prompt.
    let selected: Vec<_> = lines[first..footer]
        .iter()
        .filter(|l| l.starts_with('›') || l.starts_with('❯'))
        .collect();
    if selected.len() != 1 || selected.iter().any(|l| choice(l).is_none()) {
        return None;
    }
    let allow = match profile {
        Profile::Codex => {
            if !lines[first..footer]
                .iter()
                .any(|l| choice(l) == Some("No, and tell Codex what to do differently (esc)"))
            {
                return None;
            }
            // Respect custom keymaps by requiring the explicit one-use shortcut.
            lines[first..footer]
                .iter()
                .any(|l| {
                    matches!(
                        choice(l),
                        Some("Yes, proceed (y)" | "Yes, just this once (y)")
                    )
                })
                .then(|| b"y".to_vec())
        }
        Profile::Claude => {
            if !lines[first..footer].iter().any(|l| {
                matches!(
                    choice(l),
                    Some("No" | "No, and tell Claude what to do differently (esc)")
                )
            }) {
                return None;
            }
            matches!(choice(selected[0]), Some("Yes" | "Yes, allow once")).then(|| b"\r".to_vec())
        }
        _ => return None,
    };
    let heading = lines[..first].iter().position(|l| match profile {
        Profile::Codex => matches!(
            *l,
            "Would you like to run the following command?"
                | "Would you like to make the following edits?"
        ),
        Profile::Claude => matches!(*l, "Bash command" | "Edit file" | "Create file"),
        _ => false,
    })?;
    let detail = lines[heading + 1..first].join("\n").trim().to_owned();
    if detail.is_empty() || detail.contains('…') || detail.contains("... lines") {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(key.0.as_bytes());
    digest.update(key.1.to_le_bytes());
    digest.update(cursor.to_le_bytes());
    digest.update(serde_json::to_vec(screen).ok()?);
    let id = format!("{:x}", digest.finalize());
    Some(RecognizedApproval {
        view: ApprovalV4 {
            id,
            title: lines[heading].to_owned(),
            detail,
            can_allow_once: allow.is_some(),
        },
        allow,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen() -> Screen {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-approval-v4.json"
        ))
        .unwrap();
        serde_json::from_value(fixture["observation"]["screen"].clone()).unwrap()
    }
    #[test]
    fn exact_codex_shortcuts_and_full_details_are_required() {
        let key = (uuid::Uuid::new_v4(), 1);
        let s = screen();
        let recognized = recognize(Profile::Codex, key, 100, &s).unwrap();
        assert_eq!(recognized.allow, Some(b"y".to_vec()));
        assert!(recognized.view.detail.contains("printf"));
        assert_ne!(
            recognized.view.id,
            recognize(Profile::Codex, key, 101, &s).unwrap().view.id
        );
        assert_ne!(
            recognized.view.id,
            recognize(Profile::Codex, (key.0, 2), 100, &s)
                .unwrap()
                .view
                .id
        );
        for mutation in 0..6 {
            let mut s = screen();
            match mutation {
                0 => s.at_bottom = false,
                1 => s.lines[0] = "Previous approval example".into(),
                2 => s.lines[2] = "… omitted command".into(),
                3 => s.lines.push("› draft".into()),
                4 => s.lines[4] = "› 1. Yes, and don't ask again (a)".into(),
                _ => s.lines[6] = "Tab to accept".into(),
            }
            assert!(
                recognize(Profile::Codex, key, 100, &s).is_none_or(|r| r.allow.is_none()),
                "mutation {mutation}"
            );
        }
    }
    #[test]
    fn claude_enter_only_confirms_the_selected_single_use_choice() {
        let key = (uuid::Uuid::new_v4(), 1);
        let mut s = screen();
        s.lines = [
            "Bash command",
            "printf 'hello'",
            "Do you want to proceed?",
            "❯ 1. Yes",
            "  2. Yes, and don't ask again",
            "  3. No",
            "Enter to confirm · Esc to cancel",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            recognize(Profile::Claude, key, 100, &s).unwrap().allow,
            Some(b"\r".to_vec())
        );
        s.lines[3] = "  1. Yes".into();
        s.lines[4] = "❯ 2. Yes, and don't ask again".into();
        assert!(
            recognize(Profile::Claude, key, 100, &s)
                .unwrap()
                .allow
                .is_none()
        );
        assert!(recognize(Profile::Shell, key, 100, &s).is_none());
    }
}
