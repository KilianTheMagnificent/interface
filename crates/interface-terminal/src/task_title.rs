//! Local task labels for automatic provider titles. Explicit renames bypass this.

pub(crate) fn bounded_codex_name(text: &str) -> String {
    let clean = text.chars().filter(|c| !c.is_control()).collect::<String>();
    let mut result = String::new();
    for word in clean.split_whitespace() {
        if result.chars().count() + word.chars().count() + usize::from(!result.is_empty()) > 64 {
            if result.is_empty() {
                result.extend(word.chars().take(63));
            }
            result.push('…');
            break;
        }
        if !result.is_empty() {
            result.push(' ');
        }
        result.push_str(word);
    }
    result
}

pub(crate) fn codex_request_title(text: &str) -> String {
    let text = text.trim().trim_start_matches("</image>").trim();
    // Condense common request framing before the existing word-boundary label
    // formatter clips it. No model request or conversation mutation is needed.
    let text = text
        .replace("run deep research on all of the best ", "research ")
        .replace("run deep research on ", "research ")
        .replace("Do deep research into ", "Research ")
        .replace("do deep research into ", "research ")
        .replace("run a comprehensive code review of ", "review ")
        .replace("do a comprehensive review of ", "review ");
    bounded_codex_name(&task_title(&text))
}

/// Drop conversational lead-ins only when there is a useful task left. Keep
/// already named tasks, identifiers and non-English text intact. This is a
/// bounded label formatter, not a model-generated summary of the conversation.
pub(crate) fn task_title(title: &str) -> String {
    let mut task = title.trim();
    let mut changed = false;
    // Attachment markers carry no task identity. Only automatic provider names
    // pass through this formatter; explicit Interface renames remain exact.
    while let Some(end) = task.find(']') {
        let marker = &task[..end];
        let Some(digits) = marker.get(7..).filter(|_| {
            marker
                .get(..7)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("[Image "))
        }) else {
            break;
        };
        let digits = digits.trim().strip_prefix('#').unwrap_or(digits.trim());
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let remaining = task[end + 1..].trim();
        if remaining.is_empty() {
            break;
        }
        task = remaining;
        changed = true;
    }
    for _ in 0..5 {
        let prefix = [
            "i would like you to ",
            "i'd like you to ",
            "i want you to ",
            "i need you to ",
            "could you ",
            "would you ",
            "can you ",
            "please ",
            "help me ",
            "give me an ",
            "give me a ",
            "give me ",
            "do a ",
            "do an ",
        ]
        .into_iter()
        .find(|prefix| {
            task.get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        });
        let Some(prefix) = prefix else { break };
        let remaining = task[prefix.len()..].trim();
        if remaining.is_empty() {
            break;
        }
        task = remaining;
        changed = true;
    }
    // A provider's concise chat name is already useful; do not rewrite it.
    if !changed {
        return title.to_owned();
    }
    let mut label = String::new();
    for (index, word) in task.split_whitespace().enumerate() {
        if index >= 7
            || (!label.is_empty() && label.chars().count() + 1 + word.chars().count() > 48)
        {
            label.push('…');
            break;
        }
        if !label.is_empty() {
            label.push(' ');
        }
        label.push_str(word);
    }
    // Preserve case-sensitive identifiers such as iOS and useCache.
    if let Some(first) = label.split_whitespace().next()
        && first
            .trim_end_matches('…')
            .chars()
            .all(|c| c.is_ascii_lowercase())
    {
        label[..1].make_ascii_uppercase();
    }
    label
}

#[cfg(test)]
mod tests {
    use super::task_title;

    #[test]
    fn request_lead_ins_yield_task_labels() {
        for (input, expected) in [
            ("I want you to fix session recovery", "Fix session recovery"),
            (
                "[Image #1] [Image #2] I want you to fix resume",
                "Fix resume",
            ),
            ("[Image #1] iOS Simulator", "iOS Simulator"),
            (
                "Could you please help me review the terminal layout",
                "Review the terminal layout",
            ),
            (
                "I'd like you to do a full review of terminal startup",
                "Full review of terminal startup",
            ),
            (
                "Give me an overall portfolio performance report",
                "Overall portfolio performance report",
            ),
            (
                "Please fix iOS Simulator startup",
                "Fix iOS Simulator startup",
            ),
            ("Can you iOS Simulator", "iOS Simulator"),
            ("Please 修复终端恢复", "修复终端恢复"),
        ] {
            assert_eq!(task_title(input), expected);
        }
    }

    #[test]
    fn concise_names_and_incomplete_requests_are_not_destroyed() {
        for title in [
            "[Image #1]",
            "Keep [Image #1] visible",
            "Snapshot repair",
            "BRI-89",
            "iOS Simulator",
            "Can you",
            "Please",
            "修复恢复",
            "I want you to ",
        ] {
            assert_eq!(task_title(title), title);
        }
    }

    #[test]
    fn long_request_labels_end_at_whole_words() {
        assert_eq!(
            task_title(
                "I want you to review terminal startup and recovery across all supported providers"
            ),
            "Review terminal startup and recovery across all…"
        );
        assert_eq!(
            task_title("Please review extraordinarily-long-component-identifiers and recovery"),
            "Review…"
        );
    }
}
