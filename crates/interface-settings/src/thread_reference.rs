//! Thread references (T3 section 9, protocol 63): a thread's composer
//! refers to another thread of its project, and at send the chip becomes a
//! block Rust writes: the thread's title and key, `read_thread` when the
//! sending thread has team tools, and a bounded excerpt of its journal (last
//! prompt and last answer). Who may refer to what is team tools' read rule
//! ([`crate::thread_team::may_read`]): same project root only.
//!
//! Everything here is pure; the host gathers the facts (the Threads
//! document, the registry's titles and kinds, hidden sessions, the journal)
//! and asks.

use interface_contracts::engine::{EngineEvent, EngineEventKind};
use interface_contracts::threads::WorkspaceMode;

use crate::thread_team::{cut_bytes, may_read};
use crate::threads::ThreadsDocument;
use crate::threads::is_handoff_preview;

/// The excerpt (last prompt and last answer together), in UTF-8 bytes.
pub const EXCERPT_LIMIT_BYTES: usize = 4 * 1024;
/// The last prompt's share of the excerpt; the answer gets the rest.
pub const PROMPT_LIMIT_BYTES: usize = 1024;
/// A referenced thread's title, in characters.
pub const TITLE_LIMIT_CHARS: usize = 120;
/// How many of the chain's newest journal lines the excerpt is read from.
pub const EXCERPT_JOURNAL_LINES: usize = 2000;

/// Why a reference is refused. Each says so in [`Self::message`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceRefusal {
    /// The sender is not a project thread (a plain chat, an unknown session,
    /// a new thread's launch).
    SenderNotThread,
    /// The target is the sending thread itself (or one of its segments).
    SelfReference,
    /// The target is not a listed thread: unknown, hidden, an agent's.
    NotListed,
    /// The target belongs to another project, or to none.
    OtherProject,
}

impl ReferenceRefusal {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::SenderNotThread => {
                "Only a thread in a project can refer to another thread. Remove the thread chip before sending."
            }
            Self::SelfReference => {
                "A thread cannot refer to itself. Remove the thread chip before sending."
            }
            Self::NotListed => {
                "That thread is no longer in Threads. Remove its chip before sending."
            }
            Self::OtherProject => {
                "That thread belongs to another project; a thread can refer only to threads of its own project."
            }
        }
    }
}

/// What the host knows about sessions beyond the Threads document.
pub trait ReferenceFacts {
    /// Hidden from Threads (Away rounds, orchestration workers) or an
    /// agent's session: never referred to.
    fn excluded(&self, key: &str) -> bool;
}

/// A reference both ends of which the rules admit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedReference {
    /// The sending thread's head.
    pub sender: String,
    /// The referenced thread's head: the key the chip resolves to.
    pub target: String,
    /// The referenced thread's segments, oldest activation first, the head
    /// last (its journal is read across all of them).
    pub chain: Vec<String>,
}

fn project_of(document: &ThreadsDocument, key: &str) -> Option<String> {
    document
        .record(key)?
        .workspace
        .as_ref()
        .filter(|workspace| workspace.mode != WorkspaceMode::None)
        .map(|workspace| workspace.project_root.clone())
        .filter(|root| !root.is_empty())
}

/// Whether `sender` may refer to `target`, both read through their heads.
///
/// # Errors
/// The rule that refuses it.
pub fn admit(
    document: &ThreadsDocument,
    facts: &dyn ReferenceFacts,
    sender: &str,
    target: &str,
) -> Result<AdmittedReference, ReferenceRefusal> {
    let sender = document
        .head_of(sender)
        .ok_or(ReferenceRefusal::SenderNotThread)?
        .to_owned();
    let sender_project = project_of(document, &sender).ok_or(ReferenceRefusal::SenderNotThread)?;
    let target = document
        .head_of(target)
        .ok_or(ReferenceRefusal::NotListed)?
        .to_owned();
    if target == sender {
        return Err(ReferenceRefusal::SelfReference);
    }
    if !document
        .record(&target)
        .is_some_and(crate::threads::ThreadRecord::listed)
        || facts.excluded(&target)
    {
        return Err(ReferenceRefusal::NotListed);
    }
    if !may_read(
        Some(&sender_project),
        project_of(document, &target).as_deref(),
    ) {
        return Err(ReferenceRefusal::OtherProject);
    }
    let chain = document
        .chain_members(&target)
        .into_iter()
        .map(|member| member.key.clone())
        .collect();
    Ok(AdmittedReference {
        sender,
        target,
        chain,
    })
}

/// The thread's newest prompt and the main agent's answer after it, from
/// its journal's replay (a switched thread's whole chain). A switch's
/// handoff is not the owner's prompt and is skipped.
#[must_use]
pub fn last_exchange(events: &[EngineEvent]) -> (Option<String>, Option<String>) {
    let mut prompt = None;
    let mut answer = String::new();
    for event in events {
        match &event.event {
            EngineEventKind::UserMessage {
                text,
                agent_key: None,
                ..
            } if !is_handoff_preview(text) => {
                prompt = Some(text.clone());
                answer.clear();
            }
            EngineEventKind::MessageDelta {
                text,
                agent_key: None,
            } => {
                if !answer.is_empty() && !answer.ends_with('\n') {
                    answer.push_str("\n\n");
                }
                answer.push_str(text);
            }
            _ => {}
        }
    }
    let answer = answer.trim();
    (
        prompt.map(|prompt| prompt.trim().to_owned()),
        (!answer.is_empty()).then(|| answer.to_owned()),
    )
}

fn bounded(text: &str, limit: usize) -> String {
    let (kept, cut) = cut_bytes(text, limit.saturating_sub("…".len()));
    if cut {
        format!("{}…", kept.trim_end())
    } else {
        text.to_owned()
    }
}

/// A title as one bounded line.
#[must_use]
pub fn reference_title(title: &str, key: &str) -> String {
    let line = title
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return key.to_owned();
    }
    match line.char_indices().nth(TITLE_LIMIT_CHARS) {
        Some((end, _)) => format!("{}…", line[..end].trim_end()),
        None => line,
    }
}

/// The excerpt: the last prompt (at most [`PROMPT_LIMIT_BYTES`]) and the
/// last answer (the rest of [`EXCERPT_LIMIT_BYTES`]), each cut at a
/// character with "…".
#[must_use]
pub fn excerpt(prompt: Option<&str>, answer: Option<&str>) -> String {
    let mut parts = Vec::new();
    let mut left = EXCERPT_LIMIT_BYTES;
    if let Some(prompt) = prompt {
        let head = "Last prompt:\n";
        let body = bounded(prompt, PROMPT_LIMIT_BYTES.min(left) - head.len());
        left = left.saturating_sub(head.len() + body.len() + 1);
        parts.push(format!("{head}{body}"));
    }
    if let Some(answer) = answer {
        let head = "Last answer:\n";
        if left > head.len() + 8 {
            parts.push(format!("{head}{}", bounded(answer, left - head.len())));
        }
    }
    if parts.is_empty() {
        return "Its record holds no messages yet.".into();
    }
    parts.join("\n")
}

/// The block the chip becomes at send.
#[must_use]
pub fn reference_block(
    title: &str,
    key: &str,
    provider: &str,
    team_tools: bool,
    excerpt: &str,
) -> String {
    let mut lines = vec![format!("Thread \"{title}\" ({provider}), key {key}")];
    if team_tools {
        lines.push("Use read_thread with this key for more.".into());
    }
    lines.push(excerpt.to_owned());
    lines.join("\n")
}

#[cfg(test)]
#[path = "thread_reference_tests.rs"]
mod tests;
