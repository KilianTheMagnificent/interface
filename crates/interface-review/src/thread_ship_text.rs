//! What Write with AI reads (protocol 67, T3 section 12): a thread's
//! changes and prompts, bounded and credential-filtered, as the material of
//! one tool-less Claude call (`engine_claude::ship_text`).
//!
//! The changes come from [`super::diff`], so the ship code's path classifier
//! already applies: a credential-shaped or binary file is named in the stat
//! but its content is never read. Every patch and prompt is then passed
//! through [`super::ship::redact_credentials`]. Bounds: the stat lists at
//! most [`STAT_FILES`] files, the patch is the first [`PATCH_BYTES`] of the
//! files' patches, and the whole material is at most [`MATERIAL_BYTES`],
//! the prompts taking what is left (newest last, each one line).

use std::fmt::Write as _;

use interface_contracts::threads::{DiffFileStatus, DiffOmitted, ThreadDiff};

use super::ship::redact_credentials;

/// The patch excerpt, in bytes.
pub const PATCH_BYTES: usize = 6 * 1024;
/// The whole material, in bytes.
pub const MATERIAL_BYTES: usize = 8 * 1024;
/// Files the stat names one by one; the rest are counted.
pub const STAT_FILES: usize = 60;
/// A prompt keeps at most this many characters.
const PROMPT_CHARS: usize = 400;

/// The material for `diff` and the thread's `prompts` (oldest first).
#[must_use]
pub fn ship_text_material(diff: &ThreadDiff, prompts: &[String]) -> String {
    let mut stat = String::from("<changes>\n");
    for file in diff.files.iter().take(STAT_FILES) {
        let mark = match file.status {
            DiffFileStatus::Added => 'A',
            DiffFileStatus::Modified => 'M',
            DiffFileStatus::Deleted => 'D',
            DiffFileStatus::Renamed => 'R',
        };
        let note = match file.omitted {
            Some(DiffOmitted::Sensitive) => " (credential-shaped: content left out)".to_owned(),
            Some(DiffOmitted::Binary) => " (binary)".to_owned(),
            _ => format!(" | +{} -{}", file.added, file.removed),
        };
        let path = one_line(&file.path);
        let _ = writeln!(stat, " {mark} {path}{note}");
    }
    if diff.files.len() > STAT_FILES {
        let _ = writeln!(stat, " … and {} more files", diff.files.len() - STAT_FILES);
    }
    stat.push_str("</changes>\n");

    let mut patch = String::new();
    for file in &diff.files {
        if file.omitted.is_some() {
            continue;
        }
        let Some(text) = file.patch.as_deref() else {
            continue;
        };
        patch.push_str(&redact_credentials(text));
        if !patch.ends_with('\n') {
            patch.push('\n');
        }
        if patch.len() >= PATCH_BYTES {
            break;
        }
    }
    let patch = clip(&patch, PATCH_BYTES);
    let mut material = stat;
    if !patch.trim().is_empty() {
        let _ = write!(material, "<patch>\n{}\n</patch>\n", patch.trim_end());
    }
    let material = clip(&material, MATERIAL_BYTES).to_owned();

    // The prompts take what is left, newest first, then read oldest first.
    let mut room = MATERIAL_BYTES.saturating_sub(material.len() + "<prompts>\n</prompts>\n".len());
    let mut kept = Vec::new();
    for prompt in prompts.iter().rev() {
        let line = redact_credentials(&one_line(prompt));
        let line = clip_chars(&line, PROMPT_CHARS);
        if line.is_empty() {
            continue;
        }
        let entry = format!("- {line}\n");
        if entry.len() > room {
            break;
        }
        room -= entry.len();
        kept.push(entry);
    }
    if kept.is_empty() {
        return material;
    }
    kept.reverse();
    format!("{material}<prompts>\n{}</prompts>\n", kept.concat())
}

/// `text` on one line: control characters and runs of whitespace become one
/// space.
fn one_line(text: &str) -> String {
    text.split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` cut to `max` characters, "…" when cut.
fn clip_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

#[cfg(test)]
#[path = "thread_ship_text_tests.rs"]
mod tests;
