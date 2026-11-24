use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    time::{Duration, SystemTime},
};

use super::*;

const KEY: &str = "claude#thread-a";
const VALUE: &str = "sk_test_hunter2_value";

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn age(path: &Path, by: Duration) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - by).unwrap();
}

fn now() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[test]
fn a_secret_is_one_owner_only_file_holding_exactly_the_value() {
    let root = tempfile::tempdir().unwrap();
    let path = write_secret(root.path(), KEY, "STRIPE_TEST_KEY", VALUE).unwrap();
    assert_eq!(
        path,
        thread_folder(root.path(), KEY).join("STRIPE_TEST_KEY")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), VALUE);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert_eq!(mode(&root.path().join(THREAD_SECRETS_DIR)), 0o700);
    // The folder names the thread by a digest, never by its key.
    let folder = path
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy();
    assert_eq!(folder.len(), 32);
    assert!(!folder.contains("claude"));
    // Giving it again replaces it, still owner-only, and leaves no temporary file.
    write_secret(root.path(), KEY, "STRIPE_TEST_KEY", "second").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "second");
    assert_eq!(mode(&path), 0o600);
    let names = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        [".thread".to_owned(), "STRIPE_TEST_KEY".to_owned()].into()
    );
}

#[test]
fn names_values_and_links_outside_the_rules_write_nothing() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "",
        ".",
        "..",
        ".thread",
        "a/b",
        "../escape",
        &"n".repeat(65),
    ] {
        assert!(
            write_secret(root.path(), KEY, name, VALUE).is_err(),
            "{name:?}"
        );
    }
    assert!(write_secret(root.path(), KEY, "EMPTY", "").is_err());
    let long = "x".repeat(THREAD_SECRET_VALUE_BYTES + 1);
    let error = write_secret(root.path(), KEY, "LONG", &long).unwrap_err();
    assert!(!error.0.contains('x'), "an error never carries the value");
    assert!(!root.path().join(THREAD_SECRETS_DIR).exists());
    // A folder that is a link elsewhere is refused.
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), root.path().join(THREAD_SECRETS_DIR)).unwrap();
    assert!(write_secret(root.path(), KEY, "OK", VALUE).is_err());
    assert_eq!(fs::read_dir(elsewhere.path()).unwrap().count(), 0);
}

#[test]
fn secrets_list_by_thread_and_name_and_go_after_a_day_or_with_their_thread() {
    let root = tempfile::tempdir().unwrap();
    let old = write_secret(root.path(), KEY, "OLD", VALUE).unwrap();
    write_secret(root.path(), KEY, "NEW", VALUE).unwrap();
    write_secret(root.path(), "codex#other", "TOKEN", VALUE).unwrap();
    age(&old, Duration::from_secs(25 * 60 * 60));
    let held = list(root.path());
    assert_eq!(
        held.iter()
            .map(|secret| (secret.key.as_str(), secret.name.as_str()))
            .collect::<Vec<_>>()[0],
        (KEY, "OLD"),
        "oldest expiry first"
    );
    assert_eq!(held.len(), 3);
    let soon = held.iter().find(|secret| secret.name == "NEW").unwrap();
    assert!(soon.expires_at > now() + THREAD_SECRET_TTL_MS - 60_000);

    assert_eq!(sweep(root.path(), now()), 1);
    assert!(!old.exists());
    assert_eq!(list(root.path()).len(), 2);

    assert!(forget(root.path(), KEY, "NEW").unwrap());
    assert!(!forget(root.path(), KEY, "NEW").unwrap());
    assert!(
        !thread_folder(root.path(), KEY).exists(),
        "an empty folder goes"
    );
    forget_thread(root.path(), "codex#other").unwrap();
    assert!(list(root.path()).is_empty());
    forget_thread(root.path(), "codex#never").unwrap();
}
