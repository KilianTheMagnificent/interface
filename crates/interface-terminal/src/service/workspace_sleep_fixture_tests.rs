//! The terminal activity rules the renderer implements too, held to the one
//! fixture `electron/src/renderer/terminal-activity.test.ts` reads.
use super::tests::power;
use std::collections::BTreeSet;

const FIXTURE: &str =
    include_str!("../../../../electron/src/fixtures/terminal-agent-activity-v1.txt");

/// `(class, sample)` for each row of one rule.
fn rows(rule: &str) -> Vec<(&'static str, &'static str)> {
    FIXTURE
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let row = (fields.next()?, fields.next()?, fields.next()?);
            (row.0 == rule).then_some((row.1, row.2))
        })
        .collect()
}

fn glyphs(class: &str) -> BTreeSet<char> {
    let code = |text: &str| u32::from_str_radix(text.trim_start_matches("U+"), 16).unwrap();
    rows("glyph")
        .into_iter()
        .filter(|(glyph_class, _)| *glyph_class == class)
        .flat_map(|(_, sample)| match sample.split_once("..") {
            Some((first, last)) => (code(first)..=code(last))
                .filter_map(char::from_u32)
                .collect::<Vec<_>>(),
            None => sample.chars().take(1).collect(),
        })
        .collect()
}

/// `\xHH` is one byte; everything else is UTF-8.
fn sample_bytes(sample: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut rest = sample;
    while let Some(at) = rest.find("\\x") {
        bytes.extend_from_slice(&rest.as_bytes()[..at]);
        bytes.push(u8::from_str_radix(&rest[at + 2..at + 4], 16).unwrap());
        rest = &rest[at + 4..];
    }
    bytes.extend_from_slice(rest.as_bytes());
    bytes
}

/// Every BMP title head the renderer checks, less controls that would end or
/// corrupt the OSC frame itself.
fn title_heads() -> impl Iterator<Item = char> {
    (0x21..=0xffff)
        .filter_map(char::from_u32)
        .filter(|character| !character.is_control())
}

#[test]
fn codex_titles_mean_work_for_exactly_the_fixture_spinners() {
    let spinners = glyphs("spinner");
    assert!(spinners.len() > 256);
    let mut power = power("codex");
    let wrong = title_heads()
        .filter(|character| {
            power.busy = false;
            power.output(format!("\x1b]0;{character}Task\x07").as_bytes());
            power.busy != spinners.contains(character)
        })
        .collect::<Vec<_>>();
    assert!(wrong.is_empty(), "{wrong:?}");
}

#[test]
fn title_decorations_are_exactly_the_fixture_spinners_and_brand_marks() {
    let decorations = glyphs("spinner")
        .into_iter()
        .chain(glyphs("brand"))
        .collect::<BTreeSet<_>>();
    let wrong = title_heads()
        .filter(|character| !character.is_whitespace())
        .filter(|character| {
            let stripped = crate::osc_title::sanitize_title(&format!("{character}Task"))
                .is_some_and(|title| title == "Task");
            stripped != decorations.contains(character)
        })
        .collect::<Vec<_>>();
    assert!(wrong.is_empty(), "{wrong:?}");
}

#[test]
fn codex_osc9_notifications_match_the_fixture() {
    let samples = rows("osc9");
    assert!(!samples.is_empty());
    for (class, message) in samples {
        let mut power = power("codex");
        power.output(format!("\x1b]9;{message}\x07").as_bytes());
        let seen = if power.busy {
            "waiting"
        } else if power.idle_since.is_some() {
            "idle"
        } else {
            "unchanged"
        };
        assert_eq!(seen, class, "{message}");
    }
}

#[test]
fn only_the_fixture_replies_are_terminal_reports() {
    let samples = rows("input");
    assert!(samples.iter().any(|(class, _)| *class == "reply"));
    for (class, sample) in samples {
        assert_eq!(
            crate::screen::is_terminal_report(&sample_bytes(sample)),
            class == "reply",
            "{sample}"
        );
    }
}

#[test]
fn agy_native_status_overrides_hooks_for_attention_and_workspace_sleep() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../electron/src/fixtures/terminal-agy-activity-v1.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        for split in [false, true] {
            let mut power = power(case["profile"].as_str().unwrap());
            for step in case["steps"].as_array().unwrap() {
                let bytes = step["output"].as_str().unwrap().as_bytes();
                if split {
                    for byte in bytes {
                        power.output(&[*byte]);
                    }
                } else {
                    power.output(bytes);
                }
                let expected = step["activity"].as_str().unwrap();
                assert_eq!(power.attention.waiting(), expected == "waiting", "{case}");
                assert_eq!(power.idle_since.is_some(), expected == "idle", "{case}");
            }
        }
    }
}
