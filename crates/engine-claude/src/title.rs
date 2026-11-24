//! One-shot conversation titles (Threads "Titles"): a single `claude -p`
//! call with no tools, no settings sources, no customization and no session
//! left behind (`--no-session-persistence`), reading its excerpt from stdin.
//! The installed CLI stays the authentication boundary, and the child gets
//! the same scrubbed environment as every other Claude spawn here.

use std::{path::Path, process::Stdio, time::Duration};

use interface_engine::worker::ProcessGroup;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

use crate::transport::configure_claude_environment;

/// Claude Haiku 5.5 by its exact id: the alias (`haiku`) moves to whatever
/// the installed CLI calls latest, and titles are named as Haiku 5.5's.
/// Write with AI shares it through [`one_shot_arguments`].
pub const TITLE_MODEL: &str = "claude-haiku-5-5";
/// A title call never takes longer than this, start to finish.
pub const TITLE_TIMEOUT: Duration = Duration::from_secs(30);
/// The most of each excerpt (first prompt, first answer) a title reads.
pub const TITLE_EXCERPT_BYTES: usize = 2 * 1024;
/// A title longer than this is cut at a word.
const TITLE_MAX_CHARS: usize = 60;
const OUTPUT_BYTES: usize = 64 * 1024;
const SYSTEM_PROMPT: &str = "You name conversations. Reply with the title only.";
/// Works from the first prompt alone (a title asked for as the prompt goes
/// out) and uses the first answer when there is one (the retry after the
/// first completed run).
const INSTRUCTION: &str = "Write a 3-8 word title, fewer than 40 characters, naming the task or subject of the first message below (use its answer, if one is given, only to make the title more exact); no quotes, no trailing period. Reply with the title only.";

/// The arguments of a title call, after the executable. Print mode, one JSON
/// result, Claude Haiku 5.5 ([`TITLE_MODEL`]), nothing persisted, every built-in tool off,
/// no settings sources or MCP servers, and customization off (`--safe-mode`).
#[must_use]
pub fn title_arguments() -> [&'static str; 19] {
    one_shot_arguments(SYSTEM_PROMPT)
}

/// The arguments every one-shot call shares (titles, and protocol 67's
/// Write with AI), with its own system prompt.
#[must_use]
pub(crate) fn one_shot_arguments(system_prompt: &'static str) -> [&'static str; 19] {
    [
        "-p",
        "--output-format",
        "json",
        "--model",
        TITLE_MODEL,
        "--no-session-persistence",
        "--tools",
        "",
        "--setting-sources=",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--safe-mode",
        "--disable-slash-commands",
        "--no-chrome",
        "--permission-mode",
        "dontAsk",
        "--system-prompt",
        system_prompt,
    ]
}

/// Asks the Claude CLI at `executable` (run in `cwd`) for a short title of a
/// conversation that began with `first_prompt` and was answered with
/// `first_answer` (may be empty). Each excerpt is cut to
/// [`TITLE_EXCERPT_BYTES`]. Answers the cleaned title.
///
/// # Errors
///
/// Why no title came back: the CLI could not start, failed, timed out
/// (after [`TITLE_TIMEOUT`]; the process group is killed) or answered
/// nothing usable. Nothing is retried.
pub async fn generate_title(
    executable: &Path,
    cwd: &Path,
    first_prompt: &str,
    first_answer: &str,
) -> Result<String, String> {
    generate_title_within(
        executable,
        cwd,
        first_prompt,
        first_answer,
        &[],
        TITLE_TIMEOUT,
    )
    .await
}

/// [`generate_title`] run with an added provider account's environment
/// (`CLAUDE_CONFIG_DIR`), so the thread's own account answers.
///
/// # Errors
/// As [`generate_title`].
pub async fn generate_title_with_environment(
    executable: &Path,
    cwd: &Path,
    first_prompt: &str,
    first_answer: &str,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Result<String, String> {
    generate_title_within(
        executable,
        cwd,
        first_prompt,
        first_answer,
        environment,
        TITLE_TIMEOUT,
    )
    .await
}

pub(crate) async fn generate_title_within(
    executable: &Path,
    cwd: &Path,
    first_prompt: &str,
    first_answer: &str,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    timeout: Duration,
) -> Result<String, String> {
    if !executable.is_absolute() {
        return Err("the Claude executable is not an absolute path".into());
    }
    let input = title_input(first_prompt, first_answer)?;
    let output = run_one_shot(
        executable,
        cwd,
        &title_arguments(),
        &input,
        environment,
        timeout,
    )
    .await?;
    parse_result(&output)?
        .as_deref()
        .and_then(clean_title)
        .ok_or_else(|| "Claude answered without a usable title".into())
}

/// Runs one print-mode call: `input` on stdin, the process group killed on
/// `timeout`, at most [`OUTPUT_BYTES`] read. Answers what it printed.
pub(crate) async fn run_one_shot(
    executable: &Path,
    cwd: &Path,
    arguments: &[&'static str; 19],
    input: &str,
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    if !executable.is_absolute() {
        return Err("the Claude executable is not an absolute path".into());
    }
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    configure_claude_environment(&mut command, executable)
        .map_err(|_| "Claude's search path could not be built".to_owned())?;
    command.envs(environment.iter().map(|(name, value)| (name, value)));
    let mut child = command
        .spawn()
        .map_err(|_| "Claude could not start; check the installed CLI".to_owned())?;
    let mut group = ProcessGroup::new(child.id());
    let mut stdin = child.stdin.take().ok_or("Claude's input was not opened")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Claude's output was not opened")?;
    let run = async {
        let send = async {
            stdin.write_all(input.as_bytes()).await?;
            stdin.shutdown().await?;
            drop(stdin);
            Ok::<(), std::io::Error>(())
        };
        let mut output = Vec::new();
        let read = async {
            let mut limited = stdout.take(u64::try_from(OUTPUT_BYTES + 1).unwrap_or(u64::MAX));
            limited.read_to_end(&mut output).await
        };
        let (delivered, received) = tokio::join!(send, read);
        delivered.map_err(|_| "Claude did not take its input".to_owned())?;
        received.map_err(|_| "Claude's answer could not be read".to_owned())?;
        let status = child
            .wait()
            .await
            .map_err(|_| "Claude's exit status was lost".to_owned())?;
        Ok::<_, String>((status, output))
    };
    let (status, output) = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| format!("Claude did not answer within {} seconds", timeout.as_secs()))??;
    group.disarm();
    if output.len() > OUTPUT_BYTES {
        return Err("Claude's answer was too long".into());
    }
    if !status.success() {
        return Err("Claude exited with an error".into());
    }
    Ok(output)
}

/// The prompt a title call reads from stdin.
fn title_input(first_prompt: &str, first_answer: &str) -> Result<String, String> {
    let prompt = clip(first_prompt.trim(), TITLE_EXCERPT_BYTES);
    if prompt.is_empty() {
        return Err("the conversation has no prompt to name it from".into());
    }
    let answer = clip(first_answer.trim(), TITLE_EXCERPT_BYTES);
    let answer = if answer.is_empty() {
        String::new()
    } else {
        format!("\n<first-answer>\n{answer}\n</first-answer>\n")
    };
    Ok(format!(
        "{INSTRUCTION}\n\n<first-prompt>\n{prompt}\n</first-prompt>\n{answer}"
    ))
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

/// The text of a successful `--output-format json` result; `None` when the
/// result carries no text.
pub(crate) fn parse_result(output: &[u8]) -> Result<Option<String>, String> {
    let text = String::from_utf8_lossy(output);
    let value = serde_json::from_str::<Value>(text.trim())
        .ok()
        .or_else(|| {
            text.lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .and_then(|line| serde_json::from_str::<Value>(line.trim()).ok())
        })
        .ok_or("Claude did not answer with a result")?;
    if value["type"] != "result" || value["subtype"] != "success" || value["is_error"] != false {
        return Err("Claude did not complete the call".into());
    }
    Ok(value["result"].as_str().map(str::to_owned))
}

/// The first line of `answer` without a `Title:` lead, wrapping quotes or
/// emphasis, or a trailing period; whitespace collapsed; cut at a word past
/// [`TITLE_MAX_CHARS`]. `None` when nothing is left.
pub(crate) fn clean_title(answer: &str) -> Option<String> {
    const WRAPPERS: &[char] = &['"', '\'', '`', '*', '_', '“', '”', '‘', '’', '#'];
    let line = answer
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let line = ["title:", "title -"]
        .iter()
        .find_map(|lead| {
            line.get(..lead.len())
                .filter(|head| head.eq_ignore_ascii_case(lead))
                .map(|_| &line[lead.len()..])
        })
        .unwrap_or(line);
    let line = line
        .trim()
        .trim_matches(WRAPPERS)
        .trim()
        .trim_end_matches('.')
        .trim_matches(WRAPPERS)
        .trim();
    let mut title = String::new();
    for word in line.split_whitespace() {
        if !title.is_empty() {
            if title.chars().count() + 1 + word.chars().count() > TITLE_MAX_CHARS {
                break;
            }
            title.push(' ');
        }
        title.push_str(word);
    }
    let title = title.chars().take(TITLE_MAX_CHARS).collect::<String>();
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
