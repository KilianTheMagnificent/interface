//! Codex's own title projection of its model catalog and effective service tier.
//! This is display metadata, never provider-session or effect authority. The
//! native key action rechecks model support and idle/modal state inside Codex.

use interface_contracts::terminal::TerminalFastModeV1;

/// The spinner stays first so existing terminal activity detection is intact.
/// The remaining fixed fields precede the untrusted, possibly delimiter-bearing
/// thread name. Keep these in step with `codex_title` and the wire fixture.
pub const CODEX_TERMINAL_TITLE_CONFIG: &str =
    r#"tui.terminal_title=["spinner","app-name","fast-mode","model","status","thread"]"#;
pub const CODEX_FAST_MODE_KEY_CONFIG: &str = r#"tui.keymap.global.toggle_fast_mode=["shift-f12"]"#;

/// Parse only the configured title shape. Missing Fast metadata means unknown
/// or unsupported, never "off". Codex omits it on unsupported catalog models.
pub(crate) fn codex_title(title: &str) -> Option<(Option<TerminalFastModeV1>, &str)> {
    // Codex replaces the spinner AND status field while asking for approval.
    // The bracketed ASCII spinner is not removed by the generic OSC scanner.
    let title = title.trim();
    let title = title
        .strip_prefix('[')
        .and_then(|s| s.split_once(']'))
        .filter(|(spinner, _)| spinner.trim().chars().all(|c| matches!(c, '.' | '!' | ' ')))
        .map_or(title, |(_, rest)| rest.trim());
    let attention = title.strip_prefix("Action Required | ");
    let title = attention.unwrap_or(title);
    let rest = title.strip_prefix("codex | ")?;
    let (first, rest) = rest.split_once(" | ")?;
    let (enabled, model, rest) = match first {
        "Fast on" | "Fast off" => {
            let (model, rest) = rest.split_once(" | ")?;
            (Some(first == "Fast on"), model, rest)
        }
        model => (None, model, rest),
    };
    let (status, thread) = if attention.is_some() {
        ("Waiting", rest)
    } else {
        rest.split_once(" | ").unwrap_or((rest, ""))
    };
    if model.is_empty()
        || !matches!(
            status,
            "Ready" | "Starting" | "Working" | "Thinking" | "Waiting"
        )
    {
        return None;
    }
    let fast = enabled.map(|enabled| TerminalFastModeV1 {
        version: 1,
        model: model.to_owned(),
        enabled,
        ready: status == "Ready",
    });
    Some((fast, thread))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_provider_state_model_changes_and_availability() {
        let (fast, thread) =
            codex_title("codex | Fast off | GPT-6-Astra | Ready | task | details").unwrap();
        let fast = fast.unwrap();
        assert!(!fast.enabled);
        assert!(fast.ready);
        assert_eq!(fast.model, "GPT-6-Astra");
        assert_eq!(thread, "task | details");
        let (fast, _) = codex_title("codex | Fast on | Future Model | Thinking | task").unwrap();
        let fast = fast.unwrap();
        assert!(fast.enabled);
        assert!(!fast.ready);
        assert_eq!(fast.model, "Future Model");
        assert_eq!(
            codex_title("codex | Unsupported Model | Ready | task"),
            Some((None, "task"))
        );
        for title in [
            "Fast on",
            "codex | Fast off | model",
            "task | Fast on",
            "codex | Fast on | model | Unknown | task",
        ] {
            assert_eq!(codex_title(title), None);
        }
    }

    #[test]
    fn approval_titles_keep_metadata_separate_from_the_task() {
        for spinner in ["[ . ] ", "[ ! ] ", ""] {
            for mode in ["on", "off"] {
                let title = format!(
                    "{spinner}Action Required | codex | Fast {mode} | gpt-6-astra | Fix names | details"
                );
                let (fast, task) = codex_title(&title).unwrap();
                assert_eq!(task, "Fix names | details");
                let fast = fast.unwrap();
                assert_eq!(fast.enabled, mode == "on");
                assert!(!fast.ready);
            }
        }
        assert_eq!(
            codex_title("[ ! ] Action Required | codex | Unsupported | task"),
            Some((None, "task"))
        );
    }
}
