use interface_contracts::ipc::{PromptContextDescriptor, PromptContextDescriptorKind};
use serde_json::json;

use super::*;

fn private_temp() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::set_permissions(
        temp.path(),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    temp
}

fn item(kind: PromptContextDescriptorKind, value: &str) -> PromptContextDescriptor {
    PromptContextDescriptor {
        id: format!("id-{value}"),
        kind,
        label: "label".into(),
        value: value.into(),
        detail: None,
        estimated_tokens: None,
        safe_content: None,
    }
}

fn draft(text: &str) -> StashDraft {
    StashDraft {
        text: text.into(),
        items: Vec::new(),
    }
}

#[test]
fn stashes_survive_a_reopen_newest_first_with_their_attachments() {
    let temp = private_temp();
    let file = temp.path().join(DRAFT_STASH_FILE_NAME);
    let mut store = DraftStashStore::open(&file).unwrap();
    let image = "/private/tmp/Interface/pasted-images/paste-1.png";
    let first = store
        .push(
            "claude#a",
            StashDraft {
                text: "Rename the login page\nand its tests".into(),
                items: vec![item(PromptContextDescriptorKind::File, image)],
            },
            10,
        )
        .unwrap();
    let second = store.push("claude#a", draft("Second idea"), 20).unwrap();
    store.push("codex#b", draft("Other thread"), 30).unwrap();
    drop(store);

    let mut store = DraftStashStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    let view = store.view("claude#a");
    assert_eq!(view.version, 1);
    assert_eq!(
        view.stashes
            .iter()
            .map(|stash| (
                stash.id.as_str(),
                stash.preview.as_str(),
                stash.at,
                stash.items
            ))
            .collect::<Vec<_>>(),
        vec![
            (second.as_str(), "Second idea", 20, 0),
            (first.as_str(), "Rename the login page", 10, 1)
        ]
    );
    // The pasted image is retained while the stash refers to it.
    assert_eq!(store.retained_paths(), vec![PathBuf::from(image)]);
    let popped = store.pop("claude#a", &first, None, 40).unwrap();
    assert_eq!(popped.draft.text, "Rename the login page\nand its tests");
    assert_eq!(popped.draft.items[0].value, image);
    assert!(store.retained_paths().is_empty());
    assert_eq!(store.count("claude#a"), 1);
    // A popped id is gone.
    assert!(store.pop("claude#a", &first, None, 41).is_err());
}

#[test]
fn pop_with_a_non_empty_composer_stashes_it_in_the_same_commit() {
    let temp = private_temp();
    let file = temp.path().join(DRAFT_STASH_FILE_NAME);
    let mut store = DraftStashStore::open(&file).unwrap();
    let parked = store.push("claude#a", draft("Parked"), 10).unwrap();
    let popped = store
        .pop("claude#a", &parked, Some(draft("Half-typed answer")), 20)
        .unwrap();
    assert_eq!(popped.draft.text, "Parked");
    let view = store.view("claude#a");
    assert_eq!(view.stashes.len(), 1);
    assert_eq!(view.stashes[0].preview, "Half-typed answer");
    assert_eq!(view.stashes[0].at, 20);
    // An empty composer stashes nothing.
    let next = view.stashes[0].id.clone();
    store.pop("claude#a", &next, Some(draft("  ")), 30).unwrap();
    assert_eq!(store.count("claude#a"), 0);
    // A refused pop changes nothing: the current draft over its bound keeps
    // the chosen stash where it was.
    let kept = store.push("claude#a", draft("Keep me"), 40).unwrap();
    let huge = draft(&"x".repeat(64 * 1024 + 1));
    assert!(store.pop("claude#a", &kept, Some(huge), 50).is_err());
    assert_eq!(store.view("claude#a").stashes[0].id, kept);
    drop(store);
    let store = DraftStashStore::open(&file).unwrap();
    assert_eq!(store.count("claude#a"), 1);
}

#[test]
fn a_stash_keeps_to_its_bounds() {
    let temp = private_temp();
    let file = temp.path().join(DRAFT_STASH_FILE_NAME);
    let mut store = DraftStashStore::open(&file).unwrap();
    assert!(store.push("claude#a", draft(" \n "), 1).is_err());
    assert!(store.push("", draft("x"), 1).is_err());
    assert!(
        store
            .push("claude#a", draft(&"x".repeat(64 * 1024 + 1)), 1)
            .is_err()
    );
    let seventeen = StashDraft {
        text: "x".into(),
        items: (0..17)
            .map(|index| {
                item(
                    PromptContextDescriptorKind::Url,
                    &format!("https://e.test/{index}"),
                )
            })
            .collect(),
    };
    assert!(store.push("claude#a", seventeen, 1).is_err());
    for index in 0..DRAFT_STASH_PER_THREAD {
        store
            .push("claude#a", draft(&format!("draft {index}")), index as u64)
            .unwrap();
    }
    let full = store
        .push("claude#a", draft("one too many"), 99)
        .unwrap_err();
    assert!(full.to_string().contains("20 stashed drafts"), "{full}");
    // Another thread still has room.
    store.push("codex#b", draft("fine"), 100).unwrap();
    // A diff chip is kept as a reference: its Review bytes are not stored.
    let mut diff = item(PromptContextDescriptorKind::Diff, "/repo");
    diff.safe_content = Some("+secret patch".into());
    store
        .push(
            "codex#b",
            StashDraft {
                text: "with diff".into(),
                items: vec![diff],
            },
            101,
        )
        .unwrap();
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(!on_disk.contains("secret patch"));
    // The whole file is bounded at 8 MiB: past it a push is refused and
    // nothing changes. Filled directly (one write), as the bound is the
    // file's.
    drop(store);
    let mut document: serde_json::Value = serde_json::from_str(&on_disk).unwrap();
    let stashes = document["stashes"].as_array_mut().unwrap();
    let big = "y".repeat(60 * 1024);
    for index in 0..136 {
        stashes.push(json!({
            "key": format!("acp:grok#t{index}"),
            "id": format!("bulk-{index}"),
            "at": 200 + index,
            "draft": {"text": big, "items": []},
        }));
    }
    std::fs::write(&file, serde_json::to_vec(&document).unwrap()).unwrap();
    let mut store = DraftStashStore::open(&file).unwrap();
    assert!(store.quarantined_artifact().is_none());
    let refused = store.push("codex#c", draft(&big), 999).unwrap_err();
    assert!(refused.to_string().contains("8 MB"), "{refused}");
    assert_eq!(store.count("codex#c"), 0);
}

#[test]
fn a_thread_removed_or_switched_takes_its_stashes_along() {
    let temp = private_temp();
    let file = temp.path().join(DRAFT_STASH_FILE_NAME);
    let mut store = DraftStashStore::open(&file).unwrap();
    store.push("claude#a", draft("one"), 1).unwrap();
    store.push("claude#a", draft("two"), 2).unwrap();
    store.push("codex#b", draft("other"), 3).unwrap();
    // Switching provider moves them to the new head.
    store.follow("claude#a", "acp:grok#c").unwrap();
    assert_eq!(store.count("claude#a"), 0);
    assert_eq!(
        store
            .view("acp:grok#c")
            .stashes
            .iter()
            .map(|stash| stash.preview.as_str())
            .collect::<Vec<_>>(),
        vec!["two", "one"]
    );
    // And back, merging with nothing lost under the bound.
    store.follow("acp:grok#c", "claude#a").unwrap();
    assert_eq!(store.count("claude#a"), 2);
    // Deleting the thread removes them; other threads keep theirs.
    store.forget("claude#a").unwrap();
    assert_eq!(store.count("claude#a"), 0);
    assert_eq!(store.count("codex#b"), 1);
    drop(store);
    let store = DraftStashStore::open(&file).unwrap();
    assert_eq!(store.count("claude#a"), 0);
    assert_eq!(store.count("codex#b"), 1);
}

#[test]
fn an_unreadable_stash_file_is_quarantined_and_the_store_starts_empty() {
    let temp = private_temp();
    let file = temp.path().join(DRAFT_STASH_FILE_NAME);
    for bytes in [
        b"{".to_vec(),
        br#"{"version":2,"next":1,"stashes":[]}"#.to_vec(),
        br#"{"version":1,"next":1,"stashes":[{"key":"a","id":"x","at":1,"draft":{"text":" ","items":[]}}]}"#.to_vec(),
        br#"{"version":1,"next":1,"stashes":[],"extra":1}"#.to_vec(),
    ] {
        std::fs::write(&file, &bytes).unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        let store = DraftStashStore::open(&file).unwrap();
        assert!(
            store.quarantined_artifact().is_some(),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
        assert_eq!(store.count("a"), 0);
        drop(store);
        let _ = std::fs::remove_file(&file);
    }
}

#[test]
fn a_preview_is_the_first_line_bounded() {
    assert_eq!(stash_preview("\n\n  Fix   the\tbug  \nmore"), "Fix the bug");
    assert_eq!(stash_preview(""), "");
    let long = stash_preview(&"word ".repeat(100));
    assert_eq!(long.chars().count(), 120);
    assert!(long.ends_with('…'));
}
