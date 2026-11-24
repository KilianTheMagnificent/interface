//! Folder trust in Threads (protocol 72) with a fake decision: which
//! threads' views carry `folderTrust`, the two commands, and the restart of
//! a newly trusted folder's idle Claude threads.

use std::collections::HashMap;
use std::time::Duration;

use interface_contracts::threads::{FolderTrustState as WireState, ThreadFolderTrust};
use interface_desktop_services::{DesktopServiceError, FolderTrustDecision, FolderTrustState};
use serde_json::json;

use super::*;
use crate::threads::{
    THREADS_FOLDER_TRUST_REFUSED_CODE, THREADS_FOLDER_TRUST_UNAVAILABLE_CODE, ThreadFolderTrustPort,
};

/// Each folder is its own trust root; answers are kept in memory.
#[derive(Default)]
struct FakeTrust {
    states: Mutex<HashMap<PathBuf, FolderTrustState>>,
    /// Each answer recorded: folder, trust.
    records: Mutex<Vec<(PathBuf, bool)>>,
}

impl FakeTrust {
    fn set(&self, folder: &str, state: FolderTrustState) {
        lock(&self.states).insert(PathBuf::from(folder), state);
    }
}

impl ThreadFolderTrustPort for FakeTrust {
    fn decide(&self, cwd: &Path, _account: Option<&str>) -> FolderTrustDecision {
        if cwd.starts_with("/missing") {
            return FolderTrustDecision {
                root: None,
                state: FolderTrustState::Unasked,
            };
        }
        FolderTrustDecision {
            root: Some(cwd.to_path_buf()),
            state: lock(&self.states)
                .get(cwd)
                .copied()
                .unwrap_or(FolderTrustState::Unasked),
        }
    }

    fn decide_new(&self, folder: &Path) -> FolderTrustDecision {
        self.decide(folder, None)
    }

    fn record(&self, folder: &Path, trust: bool) -> Result<PathBuf, DesktopServiceError> {
        lock(&self.records).push((folder.to_path_buf(), trust));
        let mut states = lock(&self.states);
        let state = states
            .entry(folder.to_path_buf())
            .or_insert(FolderTrustState::Unasked);
        if trust {
            *state = FolderTrustState::Trusted;
        } else if *state != FolderTrustState::Trusted {
            *state = FolderTrustState::Declined;
        }
        Ok(folder.to_path_buf())
    }
}

fn trusting_harness() -> (Harness, Arc<FakeTrust>) {
    let harness = harness();
    let trust = Arc::new(FakeTrust::default());
    harness.host.attach_folder_trust(trust.clone());
    (harness, trust)
}

fn thread_in(harness: &Harness, key: &str, cwd: &str) {
    let mut summary = row(key, SessionKind::Code);
    summary.cwd = cwd.into();
    harness.sessions.upsert(summary);
}

/// Waits (bounded) until `closed` keys have been closed.
async fn closed(harness: &Harness, keys: &[&str]) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let closes = lock(&harness.sessions.closes).clone();
        if keys
            .iter()
            .all(|key| closes.iter().any(|closed| closed == key))
            || tokio::time::Instant::now() > deadline
        {
            return closes;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_thread_view_carries_folder_trust_only_for_an_untrusted_claude_thread() {
    let (harness, trust) = trusting_harness();
    trust.set("/work/declined", FolderTrustState::Declined);
    trust.set("/work/trusted", FolderTrustState::Trusted);
    thread_in(&harness, "claude#unasked", "/work/hostile");
    thread_in(&harness, "claude#declined", "/work/declined");
    thread_in(&harness, "claude#trusted", "/work/trusted");
    thread_in(&harness, "claude#missing", "/missing/folder");
    thread_in(&harness, "codex#other", "/work/hostile");
    let view = harness
        .command(json!({"kind": "pin", "key": "codex#other"}))
        .await
        .unwrap()
        .view;
    let trust_of = |key: &str| thread(&view, key).folder_trust.clone();
    assert_eq!(
        trust_of("claude#unasked"),
        Some(ThreadFolderTrust {
            folder: "/work/hostile".into(),
            asked: false
        })
    );
    assert_eq!(
        trust_of("claude#declined"),
        Some(ThreadFolderTrust {
            folder: "/work/declined".into(),
            asked: true
        })
    );
    assert_eq!(trust_of("claude#trusted"), None);
    assert_eq!(trust_of("claude#missing"), None, "nothing to trust");
    assert_eq!(trust_of("codex#other"), None, "other providers never ask");
    // Without folder trust attached, no view carries it.
    let plain = harness_with_sessions_like(&harness).await;
    assert!(
        plain
            .threads
            .iter()
            .all(|thread| thread.folder_trust.is_none())
    );
}

/// The same registry rows on a host with no folder trust attached.
async fn harness_with_sessions_like(trusting: &Harness) -> ThreadsView {
    let plain = harness();
    for summary in lock(&trusting.sessions.rows).clone() {
        plain.sessions.upsert(summary);
    }
    plain
        .command(json!({"kind": "pin", "key": "codex#other"}))
        .await
        .unwrap()
        .view
}

#[tokio::test]
async fn folder_trust_answers_where_a_new_threads_folder_stands_and_refuses_bad_paths() {
    let (harness, trust) = trusting_harness();
    let answer = |response: ThreadControlResponse| {
        let answer = response.folder_trust.unwrap();
        (answer.folder, answer.state)
    };
    let read = harness
        .command(json!({"kind": "folderTrust", "folder": "/work/hostile"}))
        .await
        .unwrap();
    assert_eq!(
        answer(read),
        ("/work/hostile".to_owned(), WireState::Unasked)
    );
    trust.set("/work/hostile", FolderTrustState::Declined);
    let read = harness
        .command(json!({"kind": "folderTrust", "folder": "/work/hostile"}))
        .await
        .unwrap();
    assert_eq!(answer(read).1, WireState::Declined);
    for folder in ["relative/path", "/work/a\u{7}b", "", "/missing/folder"] {
        for command in [
            json!({"kind": "folderTrust", "folder": folder}),
            json!({"kind": "trustFolder", "folder": folder, "trust": true}),
        ] {
            let Err(refused) = harness.command(command.clone()).await else {
                if folder == "/missing/folder" && command["kind"] == "trustFolder" {
                    // The fake records any folder; the desktop service
                    // refuses a missing one (its own test).
                    continue;
                }
                panic!("{command} was accepted");
            };
            assert_eq!(refused.code, THREADS_FOLDER_TRUST_REFUSED_CODE, "{command}");
            assert!(refused.replay_safe);
        }
    }
    let long = format!("/{}", "a".repeat(4096));
    assert!(
        harness
            .command(json!({"kind": "trustFolder", "folder": long, "trust": true}))
            .await
            .is_err()
    );
    assert!(
        !lock(&trust.records)
            .iter()
            .any(|(folder, _)| folder.as_os_str().len() > 4096 || folder.is_relative()),
        "nothing invalid was recorded"
    );
    // A host without folder trust says so, definitely unsent.
    let plain = super::harness();
    let refused = plain
        .command(json!({"kind": "folderTrust", "folder": "/work/hostile"}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, THREADS_FOLDER_TRUST_UNAVAILABLE_CODE);
    assert!(refused.replay_safe);
}

#[tokio::test]
async fn trusting_a_folder_restarts_its_idle_claude_threads_and_waits_for_one_mid_turn() {
    let (harness, trust) = trusting_harness();
    for (key, cwd) in [
        ("claude#idle", "/work/hostile"),
        ("claude#busy", "/work/hostile"),
        ("claude#elsewhere", "/work/other"),
        ("codex#beside", "/work/hostile"),
    ] {
        thread_in(&harness, key, cwd);
        lock(&harness.sessions.live).insert(key.into());
    }
    // A live Claude session in the folder that is no thread is left alone.
    let mut worker = row("claude#worker", SessionKind::Agent);
    worker.cwd = "/work/hostile".into();
    harness.sessions.upsert(worker);
    lock(&harness.sessions.live).insert("claude#worker".into());
    harness
        .sessions
        .edit("claude#busy", |row| row.status = SessionStatus::Working);
    // The threads are listed.
    harness
        .command(json!({"kind": "pin", "key": "codex#beside"}))
        .await
        .unwrap();

    // Not now changes nothing that runs.
    let declined = harness
        .command(json!({"kind": "trustFolder", "folder": "/work/hostile", "trust": false}))
        .await
        .unwrap();
    assert_eq!(declined.folder_trust.unwrap().state, WireState::Declined);
    assert!(lock(&harness.sessions.closes).is_empty());

    let trusted = harness
        .command(json!({"kind": "trustFolder", "folder": "/work/hostile", "trust": true}))
        .await
        .unwrap();
    assert_eq!(trusted.folder_trust.unwrap().state, WireState::Trusted);
    assert_eq!(
        trusted
            .view
            .threads
            .iter()
            .filter(|thread| thread.folder_trust.is_some())
            .map(|thread| thread.key.as_str())
            .collect::<Vec<_>>(),
        ["claude#elsewhere"],
        "the trusted folder's threads no longer carry it"
    );
    assert_eq!(closed(&harness, &["claude#idle"]).await, ["claude#idle"]);
    // The turn finishes first.
    tokio::task::yield_now().await;
    assert!(!lock(&harness.sessions.closes).contains(&"claude#busy".to_owned()));
    harness
        .sessions
        .edit("claude#busy", |row| row.status = SessionStatus::Idle);
    harness.host.team_changed();
    tokio::time::timeout(Duration::from_secs(10), settle_tasks(&harness.host))
        .await
        .expect("the restart ends once the turn has");
    assert_eq!(
        lock(&harness.sessions.closes).clone(),
        ["claude#idle", "claude#busy"]
    );
    assert_eq!(
        lock(&trust.records).clone(),
        [
            (PathBuf::from("/work/hostile"), false),
            (PathBuf::from("/work/hostile"), true)
        ]
    );
}
