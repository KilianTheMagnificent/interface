use interface_contracts::engine::{EngineEvent, EngineEventKind};
use interface_contracts::threads::{ThreadWorkspace, WorkspaceMode};

use super::*;
use crate::threads::{ThreadRecord, ThreadsDocument};

const NOW: u64 = 1_791_300_000_000;

fn workspace(root: &str, mode: WorkspaceMode) -> ThreadWorkspace {
    ThreadWorkspace {
        mode,
        project_root: root.into(),
        worktree_path: None,
        branch: None,
        base_ref: None,
        owned: false,
        missing: None,
        kept: None,
    }
}

fn thread(key: &str, root: Option<&str>) -> ThreadRecord {
    let mut record = ThreadRecord::new(key, NOW);
    record.workspace = root.map(|root| workspace(root, WorkspaceMode::Local));
    record
}

/// Two threads of /website, one of /api, a plain chat, and a hidden
/// worker of /website.
fn document() -> ThreadsDocument {
    ThreadsDocument {
        seeded: true,
        records: vec![
            thread("claude#sender", Some("/website")),
            thread("codex#sibling", Some("/website")),
            thread("claude#elsewhere", Some("/api")),
            thread("claude#chat", None),
            thread("claude#worker", Some("/website")),
        ],
        ..ThreadsDocument::default()
    }
}

struct Hidden(&'static [&'static str]);

impl ReferenceFacts for Hidden {
    fn excluded(&self, key: &str) -> bool {
        self.0.contains(&key)
    }
}

const FACTS: Hidden = Hidden(&["claude#worker"]);

#[test]
fn a_thread_may_refer_only_to_another_listed_thread_of_its_project() {
    let document = document();
    let admitted = admit(&document, &FACTS, "claude#sender", "codex#sibling").unwrap();
    assert_eq!(admitted.target, "codex#sibling");
    assert_eq!(admitted.chain, vec!["codex#sibling".to_owned()]);
    // Another project is refused, the same rule team tools read by.
    assert_eq!(
        admit(&document, &FACTS, "claude#sender", "claude#elsewhere"),
        Err(ReferenceRefusal::OtherProject)
    );
    // A plain chat has no project to share.
    assert_eq!(
        admit(&document, &FACTS, "claude#sender", "claude#chat"),
        Err(ReferenceRefusal::OtherProject)
    );
    // Itself.
    assert_eq!(
        admit(&document, &FACTS, "claude#sender", "claude#sender"),
        Err(ReferenceRefusal::SelfReference)
    );
    // Unlisted: unknown, or hidden from Threads.
    assert_eq!(
        admit(&document, &FACTS, "claude#sender", "claude#nobody"),
        Err(ReferenceRefusal::NotListed)
    );
    assert_eq!(
        admit(&document, &FACTS, "claude#sender", "claude#worker"),
        Err(ReferenceRefusal::NotListed)
    );
    // The sender must be a project thread.
    assert_eq!(
        admit(&document, &FACTS, "claude#chat", "codex#sibling"),
        Err(ReferenceRefusal::SenderNotThread)
    );
    assert_eq!(
        admit(&document, &FACTS, "claude#unknown", "codex#sibling"),
        Err(ReferenceRefusal::SenderNotThread)
    );
    for refusal in [
        ReferenceRefusal::SenderNotThread,
        ReferenceRefusal::SelfReference,
        ReferenceRefusal::NotListed,
        ReferenceRefusal::OtherProject,
    ] {
        assert!(refusal.message().ends_with('.'));
    }
}

#[test]
fn a_switched_thread_is_referred_to_by_its_head_and_read_across_its_chain() {
    let mut document = document();
    document
        .switch_provider("codex#sibling", "claude#head", None, NOW + 10)
        .unwrap();
    // The earlier segment's key reaches the head.
    let admitted = admit(&document, &FACTS, "claude#sender", "codex#sibling").unwrap();
    assert_eq!(admitted.target, "claude#head");
    assert_eq!(
        admitted.chain,
        vec!["codex#sibling".to_owned(), "claude#head".to_owned()]
    );
    // A segment of the sender's own thread is itself.
    document
        .switch_provider("claude#sender", "codex#sender-2", None, NOW + 20)
        .unwrap();
    assert_eq!(
        admit(&document, &FACTS, "codex#sender-2", "claude#sender"),
        Err(ReferenceRefusal::SelfReference)
    );
}

fn user(text: &str) -> EngineEvent {
    EngineEvent {
        at: None,
        event: EngineEventKind::UserMessage {
            text: text.into(),
            agent_key: None,
            images: None,
            image_references: None,
        },
    }
}

fn said(text: &str, agent: Option<&str>) -> EngineEvent {
    EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text: text.into(),
            agent_key: agent.map(str::to_owned),
        },
    }
}

#[test]
fn the_excerpt_is_the_last_prompt_and_the_main_answer_after_it() {
    let events = vec![
        user("first ask"),
        said("first answer", None),
        user("Add login to the site"),
        said("Working on it.", None),
        said("a subagent's words", Some("worker-1")),
        said("Added login() and logout().", None),
        user("You are taking over a thread from Claude…"),
    ];
    let (prompt, answer) = last_exchange(&events);
    assert_eq!(prompt.as_deref(), Some("Add login to the site"));
    assert_eq!(
        answer.as_deref(),
        Some("Working on it.\n\nAdded login() and logout().")
    );
    assert_eq!(last_exchange(&[]), (None, None));
    let text = excerpt(prompt.as_deref(), answer.as_deref());
    assert_eq!(
        text,
        "Last prompt:\nAdd login to the site\nLast answer:\nWorking on it.\n\nAdded login() and logout()."
    );
    assert_eq!(excerpt(None, None), "Its record holds no messages yet.");
}

#[test]
fn the_excerpt_stays_within_four_kilobytes() {
    let prompt = "p".repeat(10_000);
    let answer = "é".repeat(10_000);
    let text = excerpt(Some(&prompt), Some(&answer));
    assert!(text.len() <= EXCERPT_LIMIT_BYTES, "{}", text.len());
    let (head, rest) = text.split_once("\nLast answer:\n").unwrap();
    assert!(head.len() <= PROMPT_LIMIT_BYTES);
    assert!(head.ends_with('…') && rest.ends_with('…'));
    // A long answer alone takes the whole excerpt.
    let alone = excerpt(None, Some(&answer));
    assert!(alone.len() <= EXCERPT_LIMIT_BYTES && alone.len() > 3 * 1024);
}

#[test]
fn the_block_names_the_thread_and_offers_read_thread_only_with_team_tools() {
    let title = reference_title("  Fix\nthe\tlogin   page  ", "codex#sibling");
    assert_eq!(title, "Fix the login page");
    assert_eq!(reference_title(" ", "codex#sibling"), "codex#sibling");
    assert!(reference_title(&"t".repeat(500), "k").chars().count() <= TITLE_LIMIT_CHARS + 1);
    let with = reference_block(&title, "codex#sibling", "Codex", true, "Last prompt:\nx");
    assert_eq!(
        with,
        "Thread \"Fix the login page\" (Codex), key codex#sibling\nUse read_thread with this key for more.\nLast prompt:\nx"
    );
    let without = reference_block(&title, "codex#sibling", "Codex", false, "Last prompt:\nx");
    assert!(!without.contains("read_thread"));
}
