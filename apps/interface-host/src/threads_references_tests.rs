//! Composer references and stash (T3 section 9, protocol 63) through the
//! thread host with fake ports: `iface:draftStash` over the real store, and
//! thread references resolved by the host for the real prompt-context owner,
//! from a journal the test writes.

use std::path::{Path, PathBuf};

use interface_contracts::{
    draft_stash::DraftStashView,
    ipc::PromptContextInvalidReason,
    thread_journal::{JournalEntry, TurnOrigin},
    threads::RunOutcome,
};
use interface_prompt_context::{
    PromptContextRuntimeView, PromptContextService, PromptContextServiceConfig,
};
use interface_settings::{
    draft_stash::{DRAFT_STASH_FILE_NAME, DraftStashStore},
    thread_journal::ThreadJournal,
    thread_reference::EXCERPT_LIMIT_BYTES,
};
use serde_json::{Value, json};

use super::*;
use crate::session_management::RemovalHooks;
use crate::threads::stash::{DRAFT_STASH_REFUSED_CODE, DRAFT_STASH_UNAVAILABLE_CODE};

const SENDER: &str = "claude#sender";
const SIBLING: &str = "codex#sibling";
const ELSEWHERE: &str = "claude#elsewhere";

struct Refs {
    harness: Harness,
    journal: ThreadJournal,
    root: String,
    at: u64,
}

/// A journalled host with stashes attached: two idle threads of `atlas`
/// (the sender and its sibling, whose journal holds two turns) and one of
/// `zephyr`.
async fn refs() -> Refs {
    let mut harness = journal_harness();
    let root = harness.dir("atlas");
    let other = harness.dir("zephyr");
    harness
        .git_thread(SENDER, &root, None, "lead the work")
        .await;
    harness
        .git_thread(SIBLING, &root, None, "fix the header")
        .await;
    harness
        .git_thread(ELSEWHERE, &other, None, "api work")
        .await;
    for key in [SENDER, SIBLING, ELSEWHERE] {
        let target = key.to_owned();
        harness
            .host
            .edit_quiet(move |document, now| {
                document.run_ended(&target, now, RunOutcome::Completed);
                Ok(())
            })
            .await
            .unwrap();
    }
    harness
        .sessions
        .edit(SIBLING, |row| "Fix the header".clone_into(&mut row.title));
    let store = DraftStashStore::open(&harness.state.path().join(DRAFT_STASH_FILE_NAME))
        .map_err(|error| error.to_string());
    harness.host.attach_stash(store);
    let journal = ThreadJournal::open(harness.state.path()).unwrap();
    let mut refs = Refs {
        harness,
        journal,
        root,
        at: NOW - 10_000,
    };
    refs.turn(SIBLING, "Make the header sticky", "Done: the header stays.");
    refs.turn(
        SIBLING,
        "Now shrink it on scroll",
        "The header shrinks to 48px on scroll.",
    );
    refs
}

impl Refs {
    fn turn(&mut self, key: &str, prompt: &str, answer: &str) {
        for entry in [
            JournalEntry::TurnStarted {
                origin: TurnOrigin::Owner,
                prompt: prompt.into(),
                digest: prompt_digest(prompt),
                cut: false,
            },
            JournalEntry::Message {
                text: answer.into(),
                agent_key: None,
                cut: false,
            },
            JournalEntry::TurnEnded {
                outcome: RunOutcome::Completed,
                reason: None,
            },
        ] {
            self.at += 1;
            self.journal.append(key, self.at, entry).unwrap();
        }
    }

    async fn stash(&self, command: Value) -> Result<DraftStashView, DispatchError> {
        self.harness
            .host
            .dispatch(CHANNELS.draft_stash, command)
            .await
            .map(|value| serde_json::from_value(value).unwrap())
    }

    /// The prompt-context owner, attached to this host, with `SENDER`'s
    /// session in the project.
    fn prompt_context(&self) -> PromptContextService {
        struct View(PathBuf);
        impl PromptContextRuntimeView for View {
            fn session_root(&self, _session_key: &str) -> Option<PathBuf> {
                Some(self.0.clone())
            }
            fn authorized_roots(&self) -> Vec<PathBuf> {
                vec![self.0.clone()]
            }
        }
        let root = std::fs::canonicalize(Path::new(&self.root)).unwrap();
        let service = PromptContextService::new(PromptContextServiceConfig::system(
            Arc::new(View(root)),
            None,
        ));
        service.attach_threads(Arc::new(self.harness.host.clone()));
        service
    }

    /// Validates one thread chip from `sender` as a send does: on the
    /// blocking pool.
    async fn refer(
        &self,
        sender: &str,
        target: &str,
    ) -> interface_contracts::ipc::PromptContextValidation {
        let service = Arc::new(self.prompt_context());
        let (sender, target) = (sender.to_owned(), target.to_owned());
        tokio::task::spawn_blocking(move || {
            service.validate_prompt_context(
                &json!(sender),
                &json!([{ "id": "chat-1", "kind": "chat", "label": "A thread", "value": target }]),
            )
        })
        .await
        .unwrap()
    }
}

/// T3 section 9: a thread chip becomes Rust's block at send: the thread's
/// title and key, `read_thread` only when the sending thread has team
/// tools, and its last prompt and answer from the journal, within 4 KiB.
#[tokio::test(flavor = "multi_thread")]
async fn a_thread_reference_becomes_the_block_rust_writes_from_the_journal() {
    let mut refs = refs().await;
    let validated = refs.refer(SENDER, SIBLING).await;
    assert!(validated.ok, "{:?}", validated.invalid);
    let item = &validated.items[0];
    assert_eq!(item.value, SIBLING);
    assert_eq!(
        item.safe_content.as_deref(),
        Some(
            "Thread \"Fix the header\" (Codex), key codex#sibling\nLast prompt:\nNow shrink it on scroll\nLast answer:\nThe header shrinks to 48px on scroll."
        )
    );
    // With team tools bound to the sender, the block says how to read more.
    refs.harness.host.team_bound(SENDER, true);
    let with_tools = refs.refer(SENDER, SIBLING).await;
    assert!(
        with_tools.items[0]
            .safe_content
            .as_deref()
            .unwrap()
            .contains("\nUse read_thread with this key for more.\n")
    );
    refs.harness.host.team_bound(SENDER, false);
    // A long answer is cut; the block stays bounded.
    let long = "word ".repeat(4000);
    refs.turn(SIBLING, "Explain everything", &long);
    let bounded = refs.refer(SENDER, SIBLING).await;
    let block = bounded.items[0].safe_content.clone().unwrap();
    let excerpt = block.split_once('\n').unwrap().1;
    assert!(excerpt.len() <= EXCERPT_LIMIT_BYTES, "{}", excerpt.len());
    assert!(block.ends_with('…'));
}

/// The read rule is team tools' own: another project, the sender itself, a
/// thread not listed (unknown or hidden) and a sender that is not a thread
/// are each refused before anything is sent.
#[tokio::test(flavor = "multi_thread")]
async fn a_thread_reference_is_refused_outside_its_project_to_itself_and_when_unlisted() {
    let refs = refs().await;
    let refused = |validation: interface_contracts::ipc::PromptContextValidation| {
        assert!(!validation.ok && validation.items.is_empty());
        (
            validation.invalid[0].reason,
            validation.invalid[0].message.clone(),
        )
    };
    let (reason, message) = refused(refs.refer(SENDER, ELSEWHERE).await);
    assert_eq!(reason, PromptContextInvalidReason::OutsideAuthorizedRoot);
    assert!(message.contains("another project"), "{message}");
    let (reason, message) = refused(refs.refer(SENDER, SENDER).await);
    assert_eq!(reason, PromptContextInvalidReason::InvalidShape);
    assert!(message.contains("itself"), "{message}");
    let (reason, _) = refused(refs.refer(SENDER, "claude#nobody").await);
    assert_eq!(reason, PromptContextInvalidReason::TargetMissing);
    lock(&refs.harness.hidden.0).insert(SIBLING.to_owned());
    let (reason, _) = refused(refs.refer(SENDER, SIBLING).await);
    assert_eq!(reason, PromptContextInvalidReason::TargetMissing);
    lock(&refs.harness.hidden.0).clear();
    let (reason, _) = refused(refs.refer("claude#not-a-thread", SIBLING).await);
    assert_eq!(reason, PromptContextInvalidReason::SessionUnavailable);
}

/// `iface:draftStash`: push, list, pop (stashing a non-empty composer in the
/// same commit), delete; a segment's key reaches its head; a session that is
/// not a thread is refused; and the thread's removal takes its stashes.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One stash's whole life.
async fn a_threads_stashed_drafts_push_pop_and_go_with_the_thread() {
    let refs = refs().await;
    let image = "/private/tmp/Interface/pasted-images/paste-1.png";
    let pushed = refs
        .stash(json!({"command": "push", "key": SENDER, "draft": {
            "text": "Rename the login page\n> quoted",
            "items": [{"id": "f1", "kind": "file", "label": "paste-1.png", "value": image}]
        }}))
        .await
        .unwrap();
    assert_eq!(pushed.key, SENDER);
    assert_eq!(pushed.stashes.len(), 1);
    assert_eq!(pushed.stashes[0].preview, "Rename the login page");
    assert_eq!(pushed.stashes[0].items, 1);
    assert_eq!(pushed.stashes[0].at, NOW);
    // Its pasted image is kept for as long as the stash refers to it.
    assert_eq!(
        refs.harness.host.stash_retained_paths(),
        vec![PathBuf::from(image)]
    );
    let first = pushed.stashes[0].id.clone();
    // Pop with a half-typed composer: it is stashed in the same commit.
    let popped = refs
        .stash(json!({"command": "pop", "key": SENDER, "id": first,
            "current": {"text": "half-typed", "items": []}}))
        .await
        .unwrap();
    let entry = popped.popped.unwrap();
    assert_eq!(entry.draft.text, "Rename the login page\n> quoted");
    assert_eq!(entry.draft.items[0].value, image);
    assert_eq!(
        popped
            .stashes
            .iter()
            .map(|stash| stash.preview.as_str())
            .collect::<Vec<_>>(),
        vec!["half-typed"]
    );
    assert!(refs.harness.host.stash_retained_paths().is_empty());
    let half = popped.stashes[0].id.clone();
    let listed = refs
        .stash(json!({"command": "list", "key": SENDER}))
        .await
        .unwrap();
    assert_eq!(listed.stashes.len(), 1);
    let deleted = refs
        .stash(json!({"command": "delete", "key": SENDER, "id": half}))
        .await
        .unwrap();
    assert!(deleted.stashes.is_empty());
    // Refusals are definitely unsent.
    for refused in [
        json!({"command": "push", "key": "claude#not-a-thread", "draft": {"text": "x"}}),
        json!({"command": "push", "key": SENDER, "draft": {"text": "  "}}),
        json!({"command": "pop", "key": SENDER, "id": "stash-404"}),
    ] {
        let error = refs.stash(refused.clone()).await.unwrap_err();
        assert_eq!(error.code, DRAFT_STASH_REFUSED_CODE, "{refused}");
        assert!(error.replay_safe && !error.retryable, "{refused}");
    }
    let malformed = refs
        .stash(json!({"command": "list", "key": SENDER, "extra": 1}))
        .await
        .unwrap_err();
    assert_eq!(malformed.code, INVALID_PAYLOAD_CODE);
    // Deleting the thread removes its stashes.
    refs.stash(json!({"command": "push", "key": SENDER, "draft": {"text": "keep?"}}))
        .await
        .unwrap();
    refs.stash(json!({"command": "push", "key": SIBLING, "draft": {"text": "sibling's"}}))
        .await
        .unwrap();
    RemovalHooks::session_removed(&refs.harness.host, SENDER);
    settle_tasks(&refs.harness.host).await;
    let file =
        std::fs::read_to_string(refs.harness.state.path().join(DRAFT_STASH_FILE_NAME)).unwrap();
    assert!(!file.contains("keep?") && file.contains("sibling's"));
}

/// Without the store, every stash command is definitely unsent.
#[tokio::test]
async fn a_host_without_the_stash_store_refuses_stash_commands() {
    let mut harness = journal_harness();
    let root = harness.dir("atlas");
    harness.git_thread(SENDER, &root, None, "lead").await;
    let error = harness
        .host
        .dispatch(
            CHANNELS.draft_stash,
            json!({"command": "list", "key": SENDER}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, DRAFT_STASH_UNAVAILABLE_CODE);
    assert!(error.replay_safe);
}

/// Protocols 61 and 62 beside 63: a thread with a scheduled prompt and a
/// visual in its journal still stashes and restores its drafts, a thread
/// reference to it still reads its last exchange past the visual, and
/// deleting it takes its stashes along with its schedules.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One thread's stash beside its schedule and visual.
async fn a_thread_with_schedules_and_visuals_still_stashes_refers_and_removes() {
    use interface_settings::thread_schedules::{THREAD_SCHEDULES_FILE_NAME, ThreadScheduleStore};
    let mut refs = refs().await;
    refs.harness.host.attach_schedules(
        ThreadScheduleStore::open(&refs.harness.state.path().join(THREAD_SCHEDULES_FILE_NAME))
            .map_err(|error| error.to_string()),
    );
    let response = refs
        .harness
        .command(
            json!({"kind": "scheduleSet", "key": SIBLING, "prompt": "Check the build.",
            "when": {"kind": "interval", "minutes": 60}}),
        )
        .await
        .unwrap();
    let scheduled = |view: &interface_contracts::threads::ThreadsView| {
        view.threads
            .iter()
            .find(|thread| thread.key == SIBLING)
            .and_then(|thread| thread.schedules.as_ref())
            .map_or(0, |schedules| schedules.schedules.len())
    };
    assert_eq!(scheduled(&response.view), 1);
    // A visual the sibling's agent showed, after its last answer.
    refs.at += 1;
    refs.journal
        .append(
            SIBLING,
            refs.at,
            JournalEntry::Visual {
                id: "v0000000000001-0123456789".into(),
                title: "Signups".into(),
                bytes: 120,
            },
        )
        .unwrap();
    // The sibling stashes and restores as before.
    let pushed = refs
        .stash(json!({"command": "push", "key": SIBLING, "draft": {"text": "parked beside a schedule"}}))
        .await
        .unwrap();
    let id = pushed.stashes[0].id.clone();
    let popped = refs
        .stash(json!({"command": "pop", "key": SIBLING, "id": id}))
        .await
        .unwrap();
    assert_eq!(
        popped.popped.unwrap().draft.text,
        "parked beside a schedule"
    );
    // A reference to it still carries its last exchange.
    let validated = refs.refer(SENDER, SIBLING).await;
    assert!(validated.ok, "{:?}", validated.invalid);
    assert!(
        validated.items[0]
            .safe_content
            .as_deref()
            .unwrap()
            .ends_with("Last answer:\nThe header shrinks to 48px on scroll.")
    );
    // Deleting the thread takes its stash and its schedules.
    refs.stash(
        json!({"command": "push", "key": SIBLING, "draft": {"text": "goes with the thread"}}),
    )
    .await
    .unwrap();
    RemovalHooks::session_removed(&refs.harness.host, SIBLING);
    settle_tasks(&refs.harness.host).await;
    let stashes =
        std::fs::read_to_string(refs.harness.state.path().join(DRAFT_STASH_FILE_NAME)).unwrap();
    assert!(!stashes.contains("goes with the thread"), "{stashes}");
    let after = refs.harness.command(json!({"kind": "read"})).await.unwrap();
    assert_eq!(scheduled(&after.view), 0);
}
