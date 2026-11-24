use super::*;

fn side(parent: &str, key: &str) -> SideQuestion {
    SideQuestion {
        parent: parent.into(),
        side: key.into(),
        question: "Which test covers the login timeout?".into(),
        asked_at: 1_791_298_800_000,
    }
}

#[test]
fn one_side_question_per_thread_kept_across_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join(THREAD_SIDES_FILE_NAME);
    let mut store = ThreadSideStore::open(&file).unwrap();
    store.open_side(side("claude#a", "claude#side-1")).unwrap();
    let again = store
        .open_side(side("claude#a", "claude#side-2"))
        .unwrap_err();
    assert!(
        again.to_string().contains("already has a side question"),
        "{again}"
    );
    // A side thread is never a thread of its own side question.
    assert!(store.open_side(side("claude#b", "claude#side-1")).is_err());
    assert!(store.open_side(side("claude#c", "claude#c")).is_err());
    let mut blank = side("codex#d", "codex#side-3");
    blank.question = "   ".into();
    assert!(store.open_side(blank).is_err());
    assert!(store.is_side("claude#side-1") && !store.is_side("claude#a"));
    assert_eq!(
        store.of_parent("claude#a").unwrap().view().key,
        "claude#side-1"
    );
    drop(store);

    let mut store = ThreadSideStore::open(&file).unwrap();
    assert_eq!(store.sides(), [side("claude#a", "claude#side-1")]);
    // Closing by the thread or by the side thread removes it.
    assert_eq!(
        store.close("claude#side-1").unwrap(),
        Some(side("claude#a", "claude#side-1"))
    );
    assert_eq!(store.close("claude#a").unwrap(), None);
    store.open_side(side("claude#a", "claude#side-4")).unwrap();
    assert!(store.close("claude#a").unwrap().is_some());
    assert!(store.sides().is_empty());
}

#[test]
fn a_file_that_breaks_the_rules_is_set_aside() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join(THREAD_SIDES_FILE_NAME);
    std::fs::write(
        &file,
        r#"{"version":1,"sides":[{"parent":"a#1","side":"a#1","question":"q","askedAt":1}]}"#,
    )
    .unwrap();
    let store = ThreadSideStore::open(&file).unwrap();
    assert!(store.sides().is_empty());
    assert!(store.quarantined_artifact().is_some());
}
