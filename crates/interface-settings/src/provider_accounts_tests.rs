use std::os::unix::fs::PermissionsExt;

use super::*;

fn root() -> tempfile::TempDir {
    tempfile::tempdir().expect("temporary root")
}

#[test]
fn provider_accounts_add_rename_default_and_reopen() {
    let root = root();
    let mut store = ProviderAccountsStore::open(root.path()).unwrap();
    assert_eq!(store.default_account(AccountProvider::Claude), "default");
    let work = store.add(AccountProvider::Claude, "  Work ", 7).unwrap();
    assert_eq!(work.label, "Work");
    assert!(valid_account_id(&work.id) && work.id.starts_with("acct-"));
    let directory = store.directory(AccountProvider::Claude, &work.id);
    assert_eq!(
        directory,
        root.path().join("accounts").join("claude").join(&work.id)
    );
    let mode = std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "an account directory is owner-private");
    store
        .rename(AccountProvider::Claude, &work.id, "Work laptop")
        .unwrap();
    store
        .set_default(AccountProvider::Claude, &work.id)
        .unwrap();
    assert!(
        store.set_default(AccountProvider::Codex, &work.id).is_err(),
        "an account belongs to one provider"
    );
    drop(store);

    let store = ProviderAccountsStore::open(root.path()).unwrap();
    assert_eq!(store.quarantined(), None);
    assert_eq!(store.default_account(AccountProvider::Claude), work.id);
    assert_eq!(store.default_account(AccountProvider::Codex), "default");
    let kept = store.accounts(AccountProvider::Claude);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].label, "Work laptop");
    assert_eq!(kept[0].created_at, 7);
    let file: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join(PROVIDER_ACCOUNTS_FILE)).unwrap())
            .unwrap();
    assert_eq!(file["version"], 1);
    assert_eq!(file["defaults"]["claude"], work.id.as_str());
    assert_eq!(file["accounts"][0]["provider"], "claude");
    assert_eq!(file["accounts"][0]["createdAt"], 7);
}

#[test]
fn provider_accounts_are_bounded_per_provider() {
    let root = root();
    let mut store = ProviderAccountsStore::open(root.path()).unwrap();
    for index in 0..MAX_ACCOUNTS_PER_PROVIDER {
        store
            .add(AccountProvider::Codex, &format!("Codex {index}"), 1)
            .unwrap();
    }
    let refused = store
        .add(AccountProvider::Codex, "One more", 1)
        .unwrap_err();
    assert!(
        refused.to_string().contains("remove one first"),
        "{refused}"
    );
    // Another provider has its own bound.
    store.add(AccountProvider::Grok, "Grok work", 1).unwrap();
    assert!(store.add(AccountProvider::Claude, "   ", 1).is_err());
    assert!(
        store
            .add(AccountProvider::Claude, &"x".repeat(41), 1)
            .is_err()
    );
}

#[test]
fn the_default_account_cannot_be_removed_or_renamed() {
    let root = root();
    let mut store = ProviderAccountsStore::open(root.path()).unwrap();
    assert!(
        store
            .remove(AccountProvider::Claude, "default", true)
            .is_err()
    );
    assert!(
        store
            .rename(AccountProvider::Claude, "default", "Mine")
            .is_err()
    );
    assert!(
        store
            .remove(AccountProvider::Claude, "acct-000000000000", false)
            .is_err()
    );
}

#[test]
fn removing_an_account_keeps_its_directory_unless_asked() {
    let root = root();
    let mut store = ProviderAccountsStore::open(root.path()).unwrap();
    let kept = store.add(AccountProvider::Grok, "Kept", 1).unwrap();
    let gone = store.add(AccountProvider::Grok, "Gone", 1).unwrap();
    store.set_default(AccountProvider::Grok, &kept.id).unwrap();
    let kept_directory = store.directory(AccountProvider::Grok, &kept.id);
    std::fs::write(kept_directory.join("auth.json"), b"{}").unwrap();

    let removed = store
        .remove(AccountProvider::Grok, &kept.id, false)
        .unwrap();
    assert!(!removed.directory_deleted);
    assert!(kept_directory.join("auth.json").exists());
    assert_eq!(
        store.default_account(AccountProvider::Grok),
        "default",
        "removing the default account makes Default the default again"
    );
    let removed = store.remove(AccountProvider::Grok, &gone.id, true).unwrap();
    assert!(removed.directory_deleted);
    assert!(!removed.directory.exists());
    assert!(store.accounts(AccountProvider::Grok).is_empty());
}

#[test]
fn an_unreadable_accounts_file_is_set_aside() {
    let root = root();
    std::fs::write(
        root.path().join(PROVIDER_ACCOUNTS_FILE),
        br#"{"version":1,"accounts":[],"defaults":{"claude":"acct-missing"}}"#,
    )
    .unwrap();
    let store = ProviderAccountsStore::open(root.path()).unwrap();
    assert!(store.quarantined().is_some());
    assert_eq!(store.default_account(AccountProvider::Claude), "default");
    std::fs::write(root.path().join(SESSION_ACCOUNTS_FILE), b"not json").unwrap();
    drop(store);
    let sessions = SessionAccountsStore::open(root.path()).unwrap();
    assert!(sessions.quarantined().is_some());
    assert!(sessions.is_empty());
}

#[test]
fn session_bindings_keep_the_first_account_and_carry_over() {
    let root = root();
    let mut store = SessionAccountsStore::open(root.path()).unwrap();
    assert_eq!(store.account_of("claude#one"), None);
    let changed = store
        .bind_all(
            [
                ("claude#one".to_owned(), "acct-aaaaaaaaaaaa".to_owned()),
                ("claude#two".to_owned(), "default".to_owned()),
            ],
            false,
        )
        .unwrap();
    assert_eq!(changed, 1, "Default is never written");
    // A listing that finds the same session again does not move it.
    store
        .bind_all(
            [("claude#one".to_owned(), "acct-bbbbbbbbbbbb".to_owned())],
            false,
        )
        .unwrap();
    assert_eq!(store.account_of("claude#one"), Some("acct-aaaaaaaaaaaa"));
    assert!(
        store
            .bind_all([("no-separator".to_owned(), "acct-a".to_owned())], false)
            .is_err()
    );
    drop(store);
    let mut store = SessionAccountsStore::open(root.path()).unwrap();
    assert_eq!(store.account_of("claude#one"), Some("acct-aaaaaaaaaaaa"));
    assert_eq!(store.account_of("claude#two"), None);
    // A launch on an explicit account replaces.
    store
        .bind_all([("claude#one".to_owned(), "default".to_owned())], true)
        .unwrap();
    assert_eq!(store.account_of("claude#one"), None);
}

#[test]
fn session_bindings_are_bounded_and_pruned_against_the_registry() {
    let root = root();
    let mut store = SessionAccountsStore::open(root.path()).unwrap();
    // One write fills the file to its bound.
    store
        .bind_all(
            (0..MAX_SESSION_BINDINGS)
                .map(|index| (format!("codex#{index}"), "acct-aaaaaaaaaaaa".to_owned())),
            false,
        )
        .unwrap();
    assert_eq!(store.len(), MAX_SESSION_BINDINGS);
    assert!(
        store
            .bind_all(
                [("codex#extra".to_owned(), "acct-aaaaaaaaaaaa".to_owned())],
                false
            )
            .is_err()
    );
    let dropped = store
        .retain(|key| key.ends_with("#1") || key.ends_with("#2"))
        .unwrap();
    assert_eq!(dropped, MAX_SESSION_BINDINGS - 2);
    store
        .bind_all(
            [("codex#extra".to_owned(), "acct-aaaaaaaaaaaa".to_owned())],
            false,
        )
        .unwrap();
    drop(store);
    let store = SessionAccountsStore::open(root.path()).unwrap();
    assert_eq!(store.len(), 3);
}

#[test]
fn account_environment_names_each_cli_directory_variable() {
    let directory = Path::new("/data/accounts/codex/acct-1");
    assert_eq!(
        account_environment(AccountProvider::Codex, directory),
        vec![(
            OsString::from("CODEX_HOME"),
            directory.as_os_str().to_owned()
        )]
    );
    assert_eq!(
        account_environment(AccountProvider::Claude, directory)[0].0,
        "CLAUDE_CONFIG_DIR"
    );
    assert_eq!(
        account_environment(AccountProvider::Grok, directory)[0].0,
        "GROK_HOME"
    );
}

#[test]
fn a_started_thread_takes_its_callers_account_only_on_the_callers_provider() {
    assert_eq!(
        started_thread_account("claude", Some("acct-work"), "claude").as_deref(),
        Some("acct-work")
    );
    assert_eq!(
        started_thread_account("claude", None, "claude").as_deref(),
        Some("default"),
        "a caller on Default starts on Default, not on a newer default"
    );
    assert_eq!(
        started_thread_account("claude", Some("acct-work"), "codex"),
        None
    );
}
