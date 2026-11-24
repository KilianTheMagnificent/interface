//! Write with AI (protocol 67, T3 section 12): a commit message or a pull
//! request's text from one tool-less `claude -p` call, made exactly as a
//! title is (`title.rs`: the cheapest model, no tools, settings sources or
//! MCP servers, nothing persisted, its own process group, killed on
//! timeout). The input is assembled and bounded by the caller
//! (`interface_review::thread_git::ship_text`); this module adds only the
//! instruction and cleans the answer.

use std::{path::Path, time::Duration};

use interface_contracts::threads::ShipTextKind;

use crate::title::{one_shot_arguments, parse_result, run_one_shot};

/// A Write with AI call never takes longer than this, start to finish.
pub const SHIP_TEXT_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest text kept from the answer, in bytes.
pub const SHIP_TEXT_MAX_BYTES: usize = 8 * 1024;
const SYSTEM_PROMPT: &str =
    "You write git commit messages and pull request descriptions. Reply with the text only.";
const COMMIT_INSTRUCTION: &str = "Write a git commit message for the changes below: a subject line of at most 72 characters in the imperative mood, a blank line, then a short body saying what changed and why. No Markdown headings, no code fences. Reply with the message only.";
const PULL_REQUEST_INSTRUCTION: &str = "Write a pull request for the changes below: its title on the first line (at most 72 characters), a blank line, then a short Markdown body: one sentence of summary and a list of the main changes. Reply with the text only.";

/// The arguments of a Write with AI call, after the executable.
#[must_use]
pub fn ship_text_arguments() -> [&'static str; 19] {
    one_shot_arguments(SYSTEM_PROMPT)
}

/// What the call reads from stdin: the instruction, then `material` (the
/// changes and prompts, already bounded and credential-filtered).
#[must_use]
pub fn ship_text_input(what: ShipTextKind, material: &str) -> String {
    let instruction = match what {
        ShipTextKind::Commit => COMMIT_INSTRUCTION,
        ShipTextKind::PullRequest => PULL_REQUEST_INSTRUCTION,
    };
    format!("{instruction}\n\n{material}")
}

/// Asks the Claude CLI at `executable` (run in `cwd`, with `environment`
/// added) for `what` from `material`. Answers the cleaned text.
///
/// # Errors
///
/// Why no text came back: the CLI could not start, failed, timed out (after
/// [`SHIP_TEXT_TIMEOUT`]; its process group is killed) or answered nothing
/// usable. Nothing is retried.
pub async fn write_ship_text(
    executable: &Path,
    cwd: &Path,
    what: ShipTextKind,
    material: &str,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Result<String, String> {
    write_ship_text_within(
        executable,
        cwd,
        what,
        material,
        environment,
        SHIP_TEXT_TIMEOUT,
    )
    .await
}

pub(crate) async fn write_ship_text_within(
    executable: &Path,
    cwd: &Path,
    what: ShipTextKind,
    material: &str,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    timeout: Duration,
) -> Result<String, String> {
    if material.trim().is_empty() {
        return Err("there are no changes to write about".into());
    }
    let input = ship_text_input(what, material);
    let output = run_one_shot(
        executable,
        cwd,
        &ship_text_arguments(),
        &input,
        environment,
        timeout,
    )
    .await?;
    parse_result(&output)?
        .as_deref()
        .and_then(clean_ship_text)
        .ok_or_else(|| "Claude answered without usable text".into())
}

/// `answer` without a code fence around it or surrounding blank lines, cut
/// to [`SHIP_TEXT_MAX_BYTES`] at a character. `None` when nothing is left.
#[must_use]
pub fn clean_ship_text(answer: &str) -> Option<String> {
    let mut text = answer.trim();
    if let Some(inner) = text.strip_prefix("```") {
        // The fence's language word, if any, ends its first line.
        let inner = inner.split_once('\n').map_or("", |(_, rest)| rest);
        text = inner.trim_end().strip_suffix("```").unwrap_or(inner).trim();
    }
    let mut end = text.len().min(SHIP_TEXT_MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = text[..end].trim_end();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
#[path = "ship_text_tests.rs"]
mod tests;
