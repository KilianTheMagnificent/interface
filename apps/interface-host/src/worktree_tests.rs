use std::{
    collections::VecDeque,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use interface_registry::{GitResult, SPAWN_FAILED};

use super::*;

struct ScriptedGit {
    answers: Mutex<VecDeque<GitResult>>,
}

impl GitRunner for ScriptedGit {
    fn run(&self, _args: &[String], _cwd: &Path) -> GitResult {
        self.answers
            .lock()
            .expect("scripted git lock")
            .pop_front()
            .unwrap_or(GitResult {
                code: SPAWN_FAILED,
                stdout: String::new(),
                stderr: "unexpected git call".into(),
            })
    }
}

struct BlockingRemovalGit {
    remove_started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    allow_remove: Mutex<mpsc::Receiver<()>>,
}

impl GitRunner for BlockingRemovalGit {
    fn run(&self, args: &[String], _cwd: &Path) -> GitResult {
        if matches!(
            args.first().map(String::as_str),
            Some("status" | "ls-files")
        ) {
            return GitResult {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            };
        }
        if args.first().map(String::as_str) == Some("worktree")
            && args.get(1).map(String::as_str) == Some("remove")
        {
            if let Some(started) = self
                .remove_started
                .lock()
                .expect("remove-start lock")
                .take()
            {
                let _ = started.send(());
            }
            self.allow_remove
                .lock()
                .expect("remove-release lock")
                .recv()
                .expect("release blocked removal");
            return GitResult {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            };
        }
        GitResult {
            code: SPAWN_FAILED,
            stdout: String::new(),
            stderr: format!("unexpected git arguments: {args:?}"),
        }
    }
}

fn test_worktree(root: &Path, name: &str) -> Worktree {
    Worktree {
        path: root.join(format!("repo/.interface-worktrees/{name}")),
        session_cwd: root.join(format!("repo/.interface-worktrees/{name}/src")),
        branch: format!("interface/{name}"),
        repo_root: root.join("repo"),
        source_cwd: root.join("repo/src"),
        dirty_source: false,
    }
}

#[tokio::test]
async fn creation_failure_remains_before_provider_contact() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let git = Arc::new(ScriptedGit {
        answers: Mutex::new(VecDeque::from([GitResult {
            code: SPAWN_FAILED,
            stdout: String::new(),
            stderr: "git unavailable".into(),
        }])),
    });
    let service = SystemWorktreeService::with_git(store, git);

    let error = service
        .create("/missing".into(), None)
        .await
        .expect_err("git refusal");

    assert!(error.message.contains("Git isn't available"));
}

#[tokio::test]
async fn session_creation_and_explicit_management_share_one_ledger() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let service = SystemWorktreeService::with_git(
        store,
        Arc::new(ScriptedGit {
            answers: Mutex::new(VecDeque::new()),
        }),
    );
    let worktree = Worktree {
        path: root.path().join("repo/.interface-worktrees/task"),
        session_cwd: root.path().join("repo/.interface-worktrees/task/src"),
        branch: "interface/task".into(),
        repo_root: root.path().join("repo"),
        source_cwd: root.path().join("repo/src"),
        dirty_source: false,
    };

    WorktreeService::persist(&service, "codex#task", &worktree)
        .await
        .expect("session creation persisted worktree");
    let records =
        ManagedWorktreeInventory::records(&service).expect("explicit management inventory");

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key, "codex#task");
    assert_eq!(records[0].worktree(), worktree);
}

#[tokio::test]
async fn stale_registration_cannot_admit_a_replacement_directory() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let worktree = test_worktree(root.path(), "replaced");
    std::fs::create_dir_all(&worktree.session_cwd)
        .expect("replacement directory at recorded checkout path");
    std::fs::create_dir_all(&worktree.source_cwd).expect("source repository directory");
    let registration = format!(
        "worktree {}\nHEAD {}\nbranch refs/heads/{}\n\n",
        worktree.path.display(),
        "1".repeat(40),
        worktree.branch
    );
    let source_root = std::fs::canonicalize(&worktree.repo_root)
        .expect("canonical source repository")
        .to_string_lossy()
        .into_owned();
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let service = SystemWorktreeService::with_git(
        store,
        Arc::new(ScriptedGit {
            answers: Mutex::new(VecDeque::from([
                GitResult {
                    code: 0,
                    stdout: registration.clone(),
                    stderr: String::new(),
                },
                GitResult {
                    code: 0,
                    stdout: registration,
                    stderr: String::new(),
                },
                // The recorded path was replaced by an ordinary directory
                // belonging to the source checkout. Registration alone is
                // stale; its actual Git toplevel is not the worktree path.
                GitResult {
                    code: 0,
                    stdout: format!("{source_root}\n"),
                    stderr: String::new(),
                },
            ])),
        }),
    );
    let record = ManagedWorktreeRecord {
        key: "claude#replaced".to_owned(),
        path: worktree.path.clone(),
        session_cwd: worktree.session_cwd.clone(),
        branch: worktree.branch.clone(),
        repo_root: worktree.repo_root.clone(),
        source_cwd: worktree.source_cwd.clone(),
        dirty_source: worktree.dirty_source,
    };

    assert_eq!(
        ManagedWorktreeInventory::verify(&service, record)
            .await
            .expect("bounded verification task"),
        ManagedWorktreeRegistration::Unknown,
        "a stale path+branch registration must not admit replacement bytes"
    );
}

#[tokio::test]
async fn degraded_ledger_never_claims_an_empty_recovered_inventory() {
    let service = SystemWorktreeService::degraded();
    assert!(
        !ManagedWorktreeInventory::records(&service)
            .expect_err("degraded inventory must remain unavailable")
            .replay_safe
    );
    let worktree = Worktree {
        path: PathBuf::from("/repo/.interface-worktrees/task"),
        session_cwd: PathBuf::from("/repo/.interface-worktrees/task/src"),
        branch: "interface/task".into(),
        repo_root: PathBuf::from("/repo"),
        source_cwd: PathBuf::from("/repo/src"),
        dirty_source: false,
    };
    assert!(
        WorktreeService::persist(&service, "codex#task", &worktree)
            .await
            .is_err()
    );
    assert!(
        !ManagedWorktreeInventory::records(&service)
            .expect_err("failed persistence cannot establish inventory authority")
            .replay_safe
    );
}

#[tokio::test]
async fn concurrent_session_persists_commit_one_serial_ledger_history() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let service = Arc::new(SystemWorktreeService::with_git(
        store,
        Arc::new(ScriptedGit {
            answers: Mutex::new(VecDeque::new()),
        }),
    ));
    let worktrees = [
        Worktree {
            path: root.path().join("repo/.interface-worktrees/first"),
            session_cwd: root.path().join("repo/.interface-worktrees/first/src"),
            branch: "interface/first".into(),
            repo_root: root.path().join("repo"),
            source_cwd: root.path().join("repo/src"),
            dirty_source: false,
        },
        Worktree {
            path: root.path().join("repo/.interface-worktrees/second"),
            session_cwd: root.path().join("repo/.interface-worktrees/second/src"),
            branch: "interface/second".into(),
            repo_root: root.path().join("repo"),
            source_cwd: root.path().join("repo/src"),
            dirty_source: false,
        },
    ];
    let mutation = service.mutation_gate.lock().await;
    let start = Arc::new(tokio::sync::Barrier::new(3));
    let writers = ["claude#first", "claude#second"]
        .into_iter()
        .zip(worktrees)
        .map(|(key, worktree)| {
            let service = Arc::clone(&service);
            let start = Arc::clone(&start);
            tokio::spawn(async move {
                start.wait().await;
                WorktreeService::persist(service.as_ref(), key, &worktree).await
            })
        })
        .collect::<Vec<_>>();
    start.wait().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        writers.iter().all(|writer| !writer.is_finished()),
        "both full-snapshot writers must wait behind the shared mutation gate"
    );
    drop(mutation);
    for writer in writers {
        tokio::time::timeout(Duration::from_secs(2), writer)
            .await
            .expect("persist completed")
            .expect("persist task joined")
            .expect("persist succeeded");
    }

    let mut keys = ManagedWorktreeInventory::records(service.as_ref())
        .expect("in-memory records")
        .into_iter()
        .map(|record| record.key)
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["claude#first", "claude#second"]);
    drop(service);

    let reopened = ManagedWorktreeStore::open(root.path()).expect("reopen managed ledger");
    let mut durable_keys = reopened
        .records()
        .iter()
        .map(|record| record.key.as_str())
        .collect::<Vec<_>>();
    durable_keys.sort_unstable();
    assert_eq!(durable_keys, ["claude#first", "claude#second"]);
}

#[tokio::test]
async fn a_ledger_write_keeps_readers_and_outlives_a_caller_that_stops_waiting() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let service = Arc::new(SystemWorktreeService::with_git(
        store,
        Arc::new(ScriptedGit {
            answers: Mutex::new(VecDeque::new()),
        }),
    ));
    WorktreeService::persist(
        service.as_ref(),
        "claude#first",
        &test_worktree(root.path(), "first"),
    )
    .await
    .expect("persist first record");

    let (entered, reached) = tokio::sync::oneshot::channel();
    let (release, released) = mpsc::channel::<()>();
    *service
        .ledger
        .replace_hook
        .lock()
        .expect("replace hook injection") = Some(Box::new(move || {
        let _ = entered.send(());
        let _ = released.recv_timeout(Duration::from_secs(10));
    }));
    let writer = Arc::clone(&service);
    let second = test_worktree(root.path(), "second");
    let persist = tokio::spawn(async move {
        WorktreeService::persist(writer.as_ref(), "claude#second", &second).await
    });
    reached.await.expect("the durable write started");

    // Readers keep the committed records while the write is in flight.
    let records = ManagedWorktreeInventory::records(service.as_ref()).expect("readable records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key, "claude#first");

    // The caller stops waiting. The write still commits to disk and memory
    // before the next writer may start from the records.
    persist.abort();
    let _ = persist.await;
    let writer = Arc::clone(&service);
    let third = test_worktree(root.path(), "third");
    let next = tokio::spawn(async move {
        WorktreeService::persist(writer.as_ref(), "claude#third", &third).await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !next.is_finished(),
        "the next writer waits for the write in flight"
    );
    release.send(()).expect("release the durable write");
    tokio::time::timeout(Duration::from_secs(2), next)
        .await
        .expect("next persist completed")
        .expect("next persist joined")
        .expect("next persist succeeded");

    let mut keys = ManagedWorktreeInventory::records(service.as_ref())
        .expect("records")
        .into_iter()
        .map(|record| record.key)
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["claude#first", "claude#second", "claude#third"]);
    drop(service);
    let reopened = ManagedWorktreeStore::open(root.path()).expect("reopen managed ledger");
    assert_eq!(reopened.records().len(), 3);
}

#[tokio::test]
async fn same_key_persist_waits_for_git_removal_and_then_replaces_the_record() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let (remove_started_tx, remove_started_rx) = tokio::sync::oneshot::channel();
    let (allow_remove_tx, allow_remove_rx) = mpsc::channel();
    let service = Arc::new(SystemWorktreeService::with_git(
        store,
        Arc::new(BlockingRemovalGit {
            remove_started: Mutex::new(Some(remove_started_tx)),
            allow_remove: Mutex::new(allow_remove_rx),
        }),
    ));
    let prior = test_worktree(root.path(), "prior");
    std::fs::create_dir_all(&prior.session_cwd).expect("prior checkout");
    WorktreeService::persist(service.as_ref(), "claude#same", &prior)
        .await
        .expect("persist prior record");
    let prior_record = ManagedWorktreeInventory::records(service.as_ref())
        .expect("prior inventory")
        .remove(0);

    let removal_service = Arc::clone(&service);
    let removal = tokio::spawn(async move {
        ManagedWorktreeInventory::remove(removal_service.as_ref(), prior_record).await
    });
    remove_started_rx.await.expect("Git removal started");

    let replacement = test_worktree(root.path(), "replacement");
    let persisted_replacement = replacement.clone();
    let persist_service = Arc::clone(&service);
    let persist = tokio::spawn(async move {
        WorktreeService::persist(
            persist_service.as_ref(),
            "claude#same",
            &persisted_replacement,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !persist.is_finished(),
        "same-key persistence must remain behind the in-flight Git removal"
    );

    allow_remove_tx.send(()).expect("allow Git removal");
    let removed = tokio::time::timeout(Duration::from_secs(2), removal)
        .await
        .expect("removal completed")
        .expect("removal task joined")
        .expect("removal succeeded");
    assert!(removed.removed);
    tokio::time::timeout(Duration::from_secs(2), persist)
        .await
        .expect("replacement persist completed")
        .expect("persist task joined")
        .expect("replacement persist succeeded");

    let records =
        ManagedWorktreeInventory::records(service.as_ref()).expect("replacement inventory");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key, "claude#same");
    assert_eq!(records[0].worktree(), replacement);
    drop(service);

    let reopened = ManagedWorktreeStore::open(root.path()).expect("reopen managed ledger");
    assert_eq!(reopened.records().len(), 1);
    assert_eq!(reopened.records()[0].key, "claude#same");
    assert_eq!(reopened.records()[0].worktree(), replacement);
}

#[tokio::test]
async fn definitely_unsent_replace_failure_keeps_prior_state_and_can_retry() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let service = SystemWorktreeService::with_git(
        store,
        Arc::new(ScriptedGit {
            answers: Mutex::new(VecDeque::new()),
        }),
    );
    let first = test_worktree(root.path(), "first");
    let second = test_worktree(root.path(), "second");
    WorktreeService::persist(&service, "claude#first", &first)
        .await
        .expect("persist first record");
    *service
        .ledger
        .replace_failure
        .lock()
        .expect("replace failure injection") = Some(StorageError::WriteFailed {
        path: root.path().join("worktrees.json"),
        reason: "injected pre-rename failure".into(),
    });

    let error = WorktreeService::persist(&service, "claude#second", &second)
        .await
        .expect_err("injected replace failure");
    assert!(error.replay_safe);
    assert!(!service.ledger.uncertain.load(Ordering::Acquire));
    let records = ManagedWorktreeInventory::records(&service).expect("prior state remains ready");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].key, "claude#first");

    WorktreeService::persist(&service, "claude#second", &second)
        .await
        .expect("retry after definitely-unsent failure");
    assert_eq!(
        ManagedWorktreeInventory::records(&service)
            .expect("retried inventory")
            .len(),
        2
    );
}

#[tokio::test]
async fn uncertain_replace_failure_poisoned_inventory_is_never_served() {
    let root = tempfile::tempdir().expect("temporary ledger");
    let mut permissions = std::fs::metadata(root.path())
        .expect("temporary directory metadata")
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(root.path(), permissions).expect("private temporary directory");
    let store = ManagedWorktreeStore::open(root.path()).expect("managed ledger");
    let service = SystemWorktreeService::with_git(
        store,
        Arc::new(ScriptedGit {
            answers: Mutex::new(VecDeque::new()),
        }),
    );
    let first = test_worktree(root.path(), "first");
    let second = test_worktree(root.path(), "second");
    WorktreeService::persist(&service, "claude#first", &first)
        .await
        .expect("persist first record");
    *service
        .ledger
        .replace_failure
        .lock()
        .expect("replace failure injection") = Some(StorageError::WriteUncertain {
        path: root.path().join("worktrees.json"),
        reason: "injected post-rename failure".into(),
    });

    let error = WorktreeService::persist(&service, "claude#second", &second)
        .await
        .expect_err("injected uncertain replace");
    assert!(!error.replay_safe);
    assert!(service.ledger.uncertain.load(Ordering::Acquire));
    assert!(ManagedWorktreeInventory::records(&service).is_err());
    assert!(
        WorktreeService::persist(&service, "claude#second", &second)
            .await
            .is_err(),
        "an uncertain store must stay unavailable for this process"
    );
}
