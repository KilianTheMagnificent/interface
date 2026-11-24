use super::*;
use interface_contracts::agents::{
    AgentColor, AgentPastThread, AgentPendingApproval, AgentShape, AgentStatus, AgentThread,
    AgentThreadState, AgentThreadStatus,
};
use serde_json::json;
use std::{fs, os::unix::fs::OpenOptionsExt, path::PathBuf};

fn open_in(dir: &Path) -> Result<AgentStore, SettingsError> {
    AgentStore::open(&dir.join("agents.v2.json"), &dir.join("agents.v1.json"))
}
fn profile(id: &str) -> AgentProfile {
    AgentProfile {
        id: id.into(),
        name: id.into(),
        engine: "codex".into(),
        model: String::new(),
        effort: String::new(),
        instructions: "Be precise.".into(),
        color: AgentColor::Teal,
        shape: Some(AgentShape::Droplet),
        home: None,
        folders: Vec::new(),
        instructions_file_sha256: None,
    }
}
fn fixture() -> (tempfile::TempDir, AgentStore) {
    let root = tempfile::tempdir().unwrap();
    let mut store = open_in(root.path()).unwrap();
    store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: 0,
                profile: profile("one"),
            },
            1,
        )
        .unwrap();
    store
        .command(
            AgentCommand::SaveRoutine {
                expected_revision: 1,
                routine: AgentRoutine {
                    id: "r".into(),
                    agent_id: "one".into(),
                    name: "Report".into(),
                    prompt: "Write a report".into(),
                    cwd: "/fixture".into(),
                    enabled: true,
                    start_at_ms: 1000,
                    interval_minutes: Some(5),
                    next_run_at_ms: None,
                },
            },
            1,
        )
        .unwrap();
    (root, store)
}
#[test]
fn an_idle_claim_writes_nothing_but_still_resets_launch_observations() {
    let (root, mut store) = fixture();
    let file = root.path().join("agents.v2.json");
    let before = fs::read(&file).unwrap();
    store
        .pending_events
        .insert("codex#stray".into(), (SessionStatus::Idle, false));
    store.pending_overflow = true;
    // The routine first falls due at 1000; nothing is queued.
    assert!(store.claim(999).unwrap().is_none());
    assert!(store.pending_events.is_empty());
    assert!(!store.pending_overflow);
    assert_eq!(fs::read(&file).unwrap(), before);
    assert!(store.claim(1000).unwrap().is_some());
}
#[test]
fn only_status_bearing_events_reach_the_store_lock() {
    assert!(!AgentStore::observes(&event(
        json!({"kind":"message.delta","text":"x"})
    )));
    assert!(AgentStore::observes(&event(
        json!({"kind":"status.changed","status":"idle"})
    )));
}
#[test]
fn only_one_writer_and_stale_commands_never_overwrite() {
    let (root, mut store) = fixture();
    assert!(open_in(root.path()).is_err());
    let before = std::fs::read(root.path().join("agents.v2.json")).unwrap();
    assert!(
        store
            .command(
                AgentCommand::SaveAgent {
                    expected_revision: 0,
                    profile: profile("two")
                },
                2
            )
            .is_err()
    );
    assert_eq!(
        before,
        std::fs::read(root.path().join("agents.v2.json")).unwrap()
    );
}
#[test]
fn missed_intervals_coalesce_and_claims_survive_restart_without_replay() {
    let (root, mut store) = fixture();
    let run = store.claim(901_001).unwrap().unwrap();
    assert_eq!(run.scheduled_at_ms, 901_000);
    assert_eq!(store.snapshot().routines[0].next_run_at_ms, Some(1_201_000));
    assert!(store.claim(1_801_001).unwrap().is_none());
    drop(store);
    let mut reopened = open_in(root.path()).unwrap();
    assert_eq!(reopened.snapshot().runs[0].status, AgentRunStatus::Unknown);
    assert!(reopened.claim(1_801_001).unwrap().is_none());
    let revision = reopened.snapshot().revision;
    reopened
        .command(
            AgentCommand::AcknowledgeRun {
                expected_revision: revision,
                id: run.id,
            },
            1_801_001,
        )
        .unwrap();
    assert_eq!(
        reopened.claim(1_801_001).unwrap().unwrap().scheduled_at_ms,
        1_801_000
    );
}
#[test]
fn next_claim_at_names_the_earliest_due_routine_or_a_queued_run() {
    let (_root, mut store) = fixture();
    assert_eq!(store.next_claim_at(), Some(1000), "the routine's first run");
    let revision = store.revision();
    store
        .command(
            AgentCommand::RunNow {
                expected_revision: revision,
                request_id: "now".into(),
                routine_id: "r".into(),
            },
            500,
        )
        .unwrap();
    assert!(store.revision() > revision, "a command commits");
    assert_eq!(store.next_claim_at(), Some(0), "a queued run is due now");
    let run = store.claim(600).unwrap().unwrap();
    assert_eq!(run.status, AgentRunStatus::Starting);
    assert_eq!(store.next_claim_at(), Some(1000));
}

#[test]
fn manual_requests_are_idempotent_and_preserve_the_admitted_inputs() {
    let (_root, mut store) = fixture();
    let command = AgentCommand::RunNow {
        expected_revision: 2,
        routine_id: "r".into(),
        request_id: "manual-one".into(),
    };
    let first = store.command(command.clone(), 5).cloned().unwrap();
    assert_eq!(store.command(command, 6).unwrap(), &first);
    let mut updated = profile("one");
    updated.instructions = "Changed later".into();
    store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: first.revision,
                profile: updated,
            },
            6,
        )
        .unwrap();
    let run = store.claim(6).unwrap().unwrap();
    assert_eq!(run.profile.instructions, "Be precise.");
    store.bind(&run.id, "codex#task", 6).unwrap();
    let idle =
        serde_json::from_value::<EngineEvent>(json!({"kind":"status.changed","status":"idle"}))
            .unwrap();
    let working =
        serde_json::from_value::<EngineEvent>(json!({"kind":"status.changed","status":"working"}))
            .unwrap();
    store.observe("codex#task", &working, 8).unwrap();
    store.observe("codex#task", &idle, 9).unwrap();
    assert_eq!(store.snapshot().runs[0].status, AgentRunStatus::Succeeded);
}
#[test]
fn import_is_once_only_even_after_every_agent_is_deleted() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open_in(root.path()).unwrap();
    let import = AgentCommand::ImportLegacy {
        agents: vec![profile("one")],
        assignments: BTreeMap::default(),
    };
    store.command(import.clone(), 1).unwrap();
    store
        .command(
            AgentCommand::RemoveAgent {
                expected_revision: 1,
                id: "one".into(),
            },
            2,
        )
        .unwrap();
    drop(store);
    let mut store = open_in(root.path()).unwrap();
    assert!(store.command(import, 3).unwrap().agents.is_empty());
}
#[test]
fn fifty_agents_receive_distinct_shape_color_pairs() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open_in(root.path()).unwrap();
    let mut appearances = BTreeSet::new();
    for n in 0..50 {
        let state = store
            .command(
                AgentCommand::SaveAgent {
                    expected_revision: n,
                    profile: profile(&format!("agent-{n}")),
                },
                1,
            )
            .unwrap();
        let p = state.agents.last().unwrap();
        assert!(appearances.insert(format!("{:?}/{:?}", p.shape, p.color)));
    }
}
#[test]
fn drift_and_corrupt_storage_are_preserved_and_stop_claims() {
    let (root, mut store) = fixture();
    let path = root.path().join("agents.v2.json");
    std::fs::write(&path, b"{broken").unwrap();
    assert!(store.claim(1000).is_err());
    assert!(store.claim(1001).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    drop(store);
    assert!(open_in(root.path()).is_err());
}

#[test]
fn initial_idle_does_not_finish_a_run_before_the_provider_begins_work() {
    let (_root, mut store) = fixture();
    let run = store.claim(1000).unwrap().unwrap();
    let idle = serde_json::from_value(json!({"kind":"status.changed","status":"idle"})).unwrap();
    store.observe("codex#initializing", &idle, 1000).unwrap();
    store.bind(&run.id, "codex#initializing", 1001).unwrap();
    assert_eq!(store.snapshot().runs[0].status, AgentRunStatus::Running);
    assert!(!store.snapshot().runs[0].began);
    assert!(store.claim(301_001).unwrap().is_none());
}
#[test]
fn reused_request_id_cannot_silently_select_another_routine() {
    let (_root, mut store) = fixture();
    store
        .command(
            AgentCommand::RunNow {
                expected_revision: 2,
                routine_id: "r".into(),
                request_id: "one".into(),
            },
            3,
        )
        .unwrap();
    assert!(
        store
            .command(
                AgentCommand::RunNow {
                    expected_revision: 3,
                    routine_id: "other".into(),
                    request_id: "one".into()
                },
                4
            )
            .is_err()
    );
    assert_eq!(store.snapshot().runs.len(), 1);
}

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn event(value: serde_json::Value) -> EngineEvent {
    serde_json::from_value(value).unwrap()
}

fn one_agent() -> (tempfile::TempDir, AgentStore) {
    let root = tempfile::tempdir().unwrap();
    let mut store = open_in(root.path()).unwrap();
    store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: 0,
                profile: profile("one"),
            },
            1,
        )
        .unwrap();
    (root, store)
}

fn revision(store: &AgentStore) -> u64 {
    store.snapshot().revision
}

fn write_private(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
}

#[test]
fn migrates_v1_once_and_leaves_v1_bytes() {
    let root = tempfile::tempdir().unwrap();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../electron/src/fixtures/agent-workspace-v14.json"
    ))
    .unwrap();
    let v1 = root.path().join("agents.v1.json");
    let bytes = serde_json::to_vec_pretty(&fixture["state"]).unwrap();
    write_private(&v1, &bytes);

    let store = open_in(root.path()).unwrap();
    let migrated = store.snapshot();
    assert_eq!(migrated.version, 2);
    assert_eq!(migrated.agents[0].id, "research");
    assert_eq!(migrated.routines[0].id, "morning");
    let on_disk: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("agents.v2.json")).unwrap()).unwrap();
    assert_eq!(on_disk["version"], 2);
    assert_eq!(fs::read(&v1).unwrap(), bytes);
    let migrated_revision = migrated.revision;
    drop(store);

    let reopened = open_in(root.path()).unwrap();
    assert_eq!(reopened.snapshot().revision, migrated_revision);
    assert_eq!(fs::read(&v1).unwrap(), bytes);
}

#[test]
fn v2_file_requires_version_two_and_legacy_requires_one() {
    let root = tempfile::tempdir().unwrap();
    let mut state = AgentWorkspace {
        version: 1,
        revision: 1,
        ..AgentWorkspace::default()
    };
    write_private(
        &root.path().join("agents.v2.json"),
        &serde_json::to_vec(&state).unwrap(),
    );
    assert!(open_in(root.path()).is_err());

    let legacy = tempfile::tempdir().unwrap();
    state.version = 2;
    write_private(
        &legacy.path().join("agents.v1.json"),
        &serde_json::to_vec(&state).unwrap(),
    );
    assert!(open_in(legacy.path()).is_err());
    assert!(!legacy.path().join("agents.v2.json").exists());
}

#[test]
fn open_marks_non_idle_thread_interrupted() {
    let (root, mut store) = one_agent();
    store.bind_thread("one", "claude#thread", 10).unwrap();
    assert_eq!(
        store.snapshot().threads["one"]
            .current
            .as_ref()
            .unwrap()
            .status,
        AgentThreadStatus::Working
    );
    drop(store);
    let reopened = open_in(root.path()).unwrap();
    let current = reopened.snapshot().threads["one"].current.clone().unwrap();
    assert_eq!(current.status, AgentThreadStatus::Idle);
    assert!(current.interrupted);
    let revision = revision(&reopened);
    drop(reopened);
    // An already idle, interrupted thread needs no second recovery commit.
    assert_eq!(open_in(root.path()).unwrap().snapshot().revision, revision);
}

#[test]
fn save_agent_preserves_rust_owned_fields() {
    let (_root, mut store) = one_agent();
    store.set_home("one", "/fixture/agents/one", SHA_A).unwrap();
    store
        .command(
            AgentCommand::SaveAgentFolders {
                expected_revision: revision(&store),
                agent_id: "one".into(),
                folders: vec!["/fixture/project".into()],
            },
            3,
        )
        .unwrap();
    let mut incoming = profile("one");
    incoming.instructions = "Changed".into();
    incoming.home = Some("/elsewhere".into());
    incoming.folders = Vec::new();
    incoming.instructions_file_sha256 = Some("b".repeat(64));
    let state = store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: revision(&store),
                profile: incoming,
            },
            4,
        )
        .cloned()
        .unwrap();
    let saved = &state.agents[0];
    assert_eq!(saved.instructions, "Changed");
    assert_eq!(saved.home.as_deref(), Some("/fixture/agents/one"));
    assert_eq!(saved.folders, vec!["/fixture/project".to_owned()]);
    assert_eq!(saved.instructions_file_sha256.as_deref(), Some(SHA_A));

    let mut fresh = profile("two");
    fresh.home = Some("/fixture/agents/two".into());
    fresh.folders = vec!["/fixture/other".into()];
    fresh.instructions_file_sha256 = Some(SHA_A.into());
    let state = store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: state.revision,
                profile: fresh,
            },
            5,
        )
        .unwrap();
    let two = state.agents.iter().find(|p| p.id == "two").unwrap();
    assert!(two.home.is_none());
    assert!(two.folders.is_empty());
    assert!(two.instructions_file_sha256.is_none());
}

#[test]
fn engine_change_moves_thread_to_past() {
    let (_root, mut store) = one_agent();
    store.bind_thread("one", "codex#t1", 10).unwrap();
    let mut same = profile("one");
    same.instructions = "Same engine".into();
    let state = store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: revision(&store),
                profile: same,
            },
            15,
        )
        .cloned()
        .unwrap();
    assert!(state.threads["one"].current.is_some());

    let mut changed = profile("one");
    changed.engine = "claude".into();
    let state = store
        .command(
            AgentCommand::SaveAgent {
                expected_revision: state.revision,
                profile: changed,
            },
            20,
        )
        .unwrap();
    let threads = &state.threads["one"];
    assert!(threads.current.is_none());
    assert_eq!(
        threads.past,
        vec![AgentPastThread {
            session_key: "codex#t1".into(),
            started_at_ms: 10,
            ended_at_ms: 20,
        }]
    );
}

#[test]
fn new_thread_moves_current_to_past_capped_at_20() {
    let (_root, mut store) = one_agent();
    for n in 0..22_u64 {
        store
            .bind_thread("one", &format!("codex#t{n}"), n + 10)
            .unwrap();
        store
            .command(
                AgentCommand::NewThread {
                    expected_revision: revision(&store),
                    agent_id: "one".into(),
                },
                n + 100,
            )
            .unwrap();
    }
    let threads = store.snapshot().threads["one"].clone();
    assert!(threads.current.is_none());
    assert_eq!(threads.past.len(), 20);
    assert_eq!(threads.past[0].session_key, "codex#t2");
    assert_eq!(threads.past[19].session_key, "codex#t21");
    assert_eq!(threads.past[19].ended_at_ms, 121);
    assert!(
        store
            .command(
                AgentCommand::NewThread {
                    expected_revision: 0,
                    agent_id: "one".into(),
                },
                200,
            )
            .is_err()
    );
}

#[test]
fn mark_read_is_noop_when_already_read() {
    let (root, mut store) = one_agent();
    store.bind_thread("one", "codex#t", 10).unwrap();
    let before = revision(&store);
    let state = store
        .command(
            AgentCommand::MarkRead {
                agent_id: "one".into(),
            },
            50,
        )
        .unwrap();
    assert_eq!(state.revision, before + 1);
    assert_eq!(state.threads["one"].last_read_at_ms, 50);
    let bytes = fs::read(root.path().join("agents.v2.json")).unwrap();
    let again = store
        .command(
            AgentCommand::MarkRead {
                agent_id: "one".into(),
            },
            60,
        )
        .unwrap();
    assert_eq!(again.revision, before + 1);
    assert_eq!(again.threads["one"].last_read_at_ms, 50);
    assert_eq!(fs::read(root.path().join("agents.v2.json")).unwrap(), bytes);

    // Reopening marks the working thread interrupted; reading clears it even when nothing is unread.
    drop(store);
    let mut store = open_in(root.path()).unwrap();
    let state = store
        .command(
            AgentCommand::MarkRead {
                agent_id: "one".into(),
            },
            70,
        )
        .unwrap();
    assert!(!state.threads["one"].current.as_ref().unwrap().interrupted);
    assert_eq!(state.threads["one"].last_read_at_ms, 70);
}

#[test]
fn folders_validate_shape() {
    let (_root, mut store) = one_agent();
    let save = |store: &mut AgentStore, folders: Vec<String>| {
        let expected_revision = revision(store);
        store
            .command(
                AgentCommand::SaveAgentFolders {
                    expected_revision,
                    agent_id: "one".into(),
                    folders,
                },
                2,
            )
            .cloned()
    };
    assert!(save(&mut store, vec!["relative/path".into()]).is_err());
    assert!(
        save(
            &mut store,
            (0..17).map(|n| format!("/fixture/{n}")).collect()
        )
        .is_err()
    );
    assert!(save(&mut store, vec!["/fixture/a".into(), "/fixture/a".into()]).is_err());
    assert!(save(&mut store, vec![format!("/{}", "a".repeat(4096))]).is_err());
    assert_eq!(revision(&store), 1);
    let state = save(&mut store, vec!["/fixture/a".into(), "/fixture/b".into()]).unwrap();
    assert_eq!(state.agents[0].folders, vec!["/fixture/a", "/fixture/b"]);
    assert!(
        store
            .command(
                AgentCommand::SaveAgentFolders {
                    expected_revision: state.revision,
                    agent_id: "missing".into(),
                    folders: Vec::new(),
                },
                3,
            )
            .is_err()
    );
}

#[test]
fn observe_thread_status_and_activity() {
    let (_root, mut store) = one_agent();
    store.bind_thread("one", "claude#t", 10).unwrap();
    let thread = |store: &AgentStore| store.snapshot().threads["one"].clone();
    assert_eq!(thread(&store).last_activity_at_ms, 10);

    store
        .observe(
            "claude#t",
            &event(json!({"kind":"status.changed","status":"idle"})),
            20,
        )
        .unwrap();
    assert_eq!(
        thread(&store).current.unwrap().status,
        AgentThreadStatus::Idle
    );
    assert_eq!(thread(&store).last_activity_at_ms, 20);

    let idle_revision = revision(&store);
    store
        .observe(
            "claude#t",
            &event(json!({"kind":"status.changed","status":"idle"})),
            25,
        )
        .unwrap();
    assert_eq!(revision(&store), idle_revision);
    assert_eq!(thread(&store).last_activity_at_ms, 20);

    store
        .observe(
            "claude#t",
            &event(json!({"kind":"user.message","text":"hi"})),
            30,
        )
        .unwrap();
    assert_eq!(
        thread(&store).current.unwrap().status,
        AgentThreadStatus::Working
    );
    assert_eq!(thread(&store).last_activity_at_ms, 20);

    store
        .observe(
            "claude#t",
            &event(json!({"kind":"approval.requested","approvalId":"a1","tool":"Bash","summary":"ls","input":{}})),
            40,
        )
        .unwrap();
    assert_eq!(
        thread(&store).current.unwrap().status,
        AgentThreadStatus::Waiting
    );
    assert_eq!(thread(&store).last_activity_at_ms, 40);

    store
        .observe(
            "claude#t",
            &event(json!({"kind":"status.changed","status":"working"})),
            45,
        )
        .unwrap();
    store
        .observe("claude#t", &event(json!({"kind":"session.closed"})), 50)
        .unwrap();
    let closed = thread(&store);
    assert_eq!(closed.current.unwrap().status, AgentThreadStatus::Idle);
    assert_eq!(closed.last_activity_at_ms, 50);
    // Thread events never create run records or buffered run observations.
    assert!(store.snapshot().runs.is_empty());
    assert!(store.pending_events.is_empty());
}

fn run(agent: &str, status: AgentRunStatus) -> AgentRun {
    AgentRun {
        id: format!("run-{agent}"),
        routine_id: "r".into(),
        agent_id: agent.into(),
        name: "Run".into(),
        scheduled_at_ms: 1,
        updated_at_ms: 1,
        status,
        session_key: Some(format!("codex#{agent}")),
        detail: String::new(),
        profile: profile(agent),
        prompt: "Do it".into(),
        cwd: "/fixture/project".into(),
        began: true,
    }
}

fn thread_state(
    status: AgentThreadStatus,
    interrupted: bool,
    read: u64,
    activity: u64,
) -> AgentThreadState {
    AgentThreadState {
        current: Some(AgentThread {
            session_key: "claude#thread".into(),
            started_at_ms: 1,
            status,
            interrupted,
        }),
        past: Vec::new(),
        last_read_at_ms: read,
        last_activity_at_ms: activity,
    }
}

#[test]
fn statuses_precedence() {
    let (_root, mut store) = one_agent();
    let mut state = AgentWorkspace::default();
    for id in [
        "pending",
        "interrupted",
        "waiting-run",
        "unknown-run",
        "working",
        "running",
        "starting",
        "unread",
        "idle",
    ] {
        state.agents.push(profile(id));
    }
    let mut pending_thread = thread_state(AgentThreadStatus::Working, false, 1, 9);
    pending_thread.current.as_mut().unwrap().session_key = "claude#pending".into();
    state.threads.insert("pending".into(), pending_thread);
    state.threads.insert(
        "interrupted".into(),
        thread_state(AgentThreadStatus::Idle, true, 5, 5),
    );
    state.threads.insert(
        "working".into(),
        thread_state(AgentThreadStatus::Working, false, 1, 9),
    );
    state.threads.insert(
        "waiting-run".into(),
        thread_state(AgentThreadStatus::Working, false, 1, 9),
    );
    state.threads.insert(
        "unread".into(),
        thread_state(AgentThreadStatus::Idle, false, 1, 9),
    );
    state.threads.insert(
        "idle".into(),
        thread_state(AgentThreadStatus::Idle, false, 9, 9),
    );
    state.runs = vec![
        run("waiting-run", AgentRunStatus::Waiting),
        run("unknown-run", AgentRunStatus::Unknown),
        run("running", AgentRunStatus::Running),
        run("starting", AgentRunStatus::Starting),
        run("idle", AgentRunStatus::Failed),
        run("unread", AgentRunStatus::Succeeded),
    ];
    store.state = state;
    let pending = vec![AgentPendingApproval {
        agent_id: "pending".into(),
        session_key: "claude#pending".into(),
        approval_id: "approval-1".into(),
        tool: "Bash".into(),
        summary: "rm -rf build".into(),
        reason: "outsideScope".into(),
        asked_at_ms: 9,
    }];
    let statuses = store.statuses(&pending);
    assert_eq!(statuses["pending"], AgentStatus::NeedsAttention);
    assert_eq!(statuses["interrupted"], AgentStatus::NeedsAttention);
    assert_eq!(statuses["waiting-run"], AgentStatus::NeedsAttention);
    assert_eq!(statuses["unknown-run"], AgentStatus::NeedsAttention);
    assert_eq!(statuses["working"], AgentStatus::Working);
    assert_eq!(statuses["running"], AgentStatus::Working);
    assert_eq!(statuses["starting"], AgentStatus::Working);
    assert_eq!(statuses["unread"], AgentStatus::Unread);
    assert_eq!(statuses["idle"], AgentStatus::Idle);
    assert_eq!(statuses.len(), 9);

    let response = store.response(&pending);
    assert_eq!(response.statuses, statuses);
    assert_eq!(response.pending_approvals, pending);
    assert_eq!(response.workspace, store.snapshot());
    assert!(response.files.is_none() && response.decisions.is_none());
}

#[test]
fn binding_for_thread_and_run() {
    let (_root, mut store) = one_agent();
    store.set_home("one", "/fixture/agents/one", SHA_A).unwrap();
    store
        .command(
            AgentCommand::SaveAgentFolders {
                expected_revision: revision(&store),
                agent_id: "one".into(),
                folders: vec!["/fixture/project".into()],
            },
            2,
        )
        .unwrap();
    store.bind_thread("one", "claude#thread", 3).unwrap();
    assert_eq!(store.snapshot().assignments["claude#thread"], "one");
    assert_eq!(
        store.binding("claude#thread", |_| true),
        Some(AgentBinding {
            agent_id: "one".into(),
            roots: vec![
                PathBuf::from("/fixture/agents/one"),
                PathBuf::from("/fixture/project"),
            ],
            cwd: PathBuf::from("/fixture/agents/one"),
            thread: true,
            access: None,
        })
    );

    store
        .command(
            AgentCommand::SaveRoutine {
                expected_revision: revision(&store),
                routine: AgentRoutine {
                    id: "r".into(),
                    agent_id: "one".into(),
                    name: "Report".into(),
                    prompt: "Write a report".into(),
                    cwd: "/fixture/routine".into(),
                    enabled: false,
                    start_at_ms: 1000,
                    interval_minutes: None,
                    next_run_at_ms: None,
                },
            },
            4,
        )
        .unwrap();
    store
        .command(
            AgentCommand::RunNow {
                expected_revision: revision(&store),
                routine_id: "r".into(),
                request_id: "once".into(),
            },
            5,
        )
        .unwrap();
    let claimed = store.claim(6).unwrap().unwrap();
    store.bind(&claimed.id, "codex#run", 7).unwrap();
    assert_eq!(
        store.binding("codex#run", |_| true),
        Some(AgentBinding {
            agent_id: "one".into(),
            roots: vec![
                PathBuf::from("/fixture/agents/one"),
                PathBuf::from("/fixture/project"),
                PathBuf::from("/fixture/routine"),
            ],
            cwd: PathBuf::from("/fixture/routine"),
            thread: false,
            access: None,
        })
    );
    assert_eq!(store.binding("claude#unbound", |_| true), None);

    // A run cwd that fails the grant check is left out, so the policy asks instead.
    let refused = store
        .binding("codex#run", |path| path != Path::new("/fixture/routine"))
        .unwrap();
    assert_eq!(
        refused.roots,
        vec![
            PathBuf::from("/fixture/agents/one"),
            PathBuf::from("/fixture/project"),
        ]
    );
    assert_eq!(refused.cwd, PathBuf::from("/fixture/routine"));
    // The thread's own home and granted folders are not re-checked.
    assert_eq!(
        store
            .binding("claude#thread", |_| false)
            .unwrap()
            .roots
            .len(),
        2
    );
}

#[test]
fn remove_agent_drops_threads_entry() {
    let (_root, mut store) = one_agent();
    store.bind_thread("one", "claude#thread", 3).unwrap();
    assert!(store.snapshot().threads.contains_key("one"));
    let state = store
        .command(
            AgentCommand::RemoveAgent {
                expected_revision: revision(&store),
                id: "one".into(),
            },
            4,
        )
        .unwrap();
    assert!(state.threads.is_empty());
    assert!(state.assignments.is_empty());
    assert_eq!(store.binding("claude#thread", |_| true), None);
}

#[test]
fn past_threads_are_recognized_until_bound_again() {
    let (_root, mut store) = one_agent();
    store.bind_thread("one", "claude#old", 3).unwrap();
    assert!(!store.is_past_thread("claude#old"));
    store
        .command(
            AgentCommand::NewThread {
                expected_revision: revision(&store),
                agent_id: "one".into(),
            },
            4,
        )
        .unwrap();
    assert!(store.is_past_thread("claude#old"));
    assert!(!store.is_past_thread("claude#other"));
    store.bind_thread("one", "claude#old", 5).unwrap();
    assert!(!store.is_past_thread("claude#old"));
}

#[test]
fn live_agent_sessions_are_the_current_thread_and_unsettled_runs() {
    let (_root, mut store) = one_agent();
    store.bind_thread("one", "claude#thread", 3).unwrap();
    assert!(store.is_live_agent_session("claude#thread"));
    assert!(!store.is_live_agent_session("claude#unknown"));
    store
        .command(
            AgentCommand::SaveRoutine {
                expected_revision: revision(&store),
                routine: AgentRoutine {
                    id: "r".into(),
                    agent_id: "one".into(),
                    name: "Report".into(),
                    prompt: "Write a report".into(),
                    cwd: "/fixture/routine".into(),
                    enabled: false,
                    start_at_ms: 1000,
                    interval_minutes: None,
                    next_run_at_ms: None,
                },
            },
            4,
        )
        .unwrap();
    store
        .command(
            AgentCommand::RunNow {
                expected_revision: revision(&store),
                routine_id: "r".into(),
                request_id: "once".into(),
            },
            5,
        )
        .unwrap();
    let claimed = store.claim(6).unwrap().unwrap();
    store.bind(&claimed.id, "codex#run", 7).unwrap();
    assert!(store.is_live_agent_session("codex#run"));
    store.fail(&claimed.id, "done", true, 8).unwrap();
    assert!(!store.is_live_agent_session("codex#run"));
    store
        .command(
            AgentCommand::NewThread {
                expected_revision: revision(&store),
                agent_id: "one".into(),
            },
            9,
        )
        .unwrap();
    assert!(!store.is_live_agent_session("claude#thread"));
}

#[test]
fn desktop_service_commands_are_refused_by_the_store() {
    let (_root, mut store) = one_agent();
    for command in [
        AgentCommand::StartAgentThread {
            agent_id: "one".into(),
            text: "hi".into(),
            request_id: "r".into(),
        },
        AgentCommand::ReadAgentFiles {
            agent_id: "one".into(),
        },
        AgentCommand::RevealAgentHome {
            agent_id: "one".into(),
        },
    ] {
        let error = store.command(command, 2).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Handled by the desktop service.")
        );
    }
    assert_eq!(revision(&store), 1);
}

#[test]
fn closing_an_idle_thread_keeps_its_activity_and_status() {
    let (_root, mut store) = one_agent();
    store.bind_thread("one", "claude#t", 10).unwrap();
    store
        .observe(
            "claude#t",
            &event(json!({"kind":"status.changed","status":"idle"})),
            20,
        )
        .unwrap();
    // Unread and idle: a close does not move its activity.
    let unread = store.snapshot();
    store
        .observe("claude#t", &event(json!({"kind":"session.closed"})), 25)
        .unwrap();
    assert_eq!(store.snapshot(), unread);
    assert_eq!(store.statuses(&[])["one"], AgentStatus::Unread);

    store
        .command(
            AgentCommand::MarkRead {
                agent_id: "one".into(),
            },
            30,
        )
        .unwrap();
    let read = store.snapshot();
    assert_eq!(store.statuses(&[])["one"], AgentStatus::Idle);
    for (at, closing) in [
        (40, json!({"kind":"session.closed"})),
        (50, json!({"kind":"status.changed","status":"failed"})),
        (
            60,
            json!({"kind":"error","message":"gone","retryable":false}),
        ),
    ] {
        store.observe("claude#t", &event(closing), at).unwrap();
    }
    assert_eq!(store.snapshot(), read);
    assert_eq!(store.snapshot().threads["one"].last_activity_at_ms, 20);
    assert_eq!(store.statuses(&[])["one"], AgentStatus::Idle);
}

#[test]
fn import_legacy_drops_rust_owned_fields() {
    let root = tempfile::tempdir().unwrap();
    let mut store = open_in(root.path()).unwrap();
    let mut imported = profile("one");
    imported.home = Some("/".into());
    imported.folders = vec!["/".into()];
    imported.instructions_file_sha256 = Some(SHA_A.into());
    let state = store
        .command(
            AgentCommand::ImportLegacy {
                agents: vec![imported],
                assignments: BTreeMap::default(),
            },
            1,
        )
        .unwrap();
    let saved = &state.agents[0];
    assert!(saved.home.is_none());
    assert!(saved.folders.is_empty());
    assert!(saved.instructions_file_sha256.is_none());
}

#[test]
#[allow(clippy::too_many_lines)]
fn calendar_requires_a_matching_test_and_coalesces_or_skips_missed_dates() {
    use interface_contracts::agent_work::{AgentCalendarSchedule, AgentMissedRunPolicy};
    let (_root, mut store) = fixture();
    let schedule = AgentCalendarSchedule {
        timezone: "UTC".into(),
        weekdays: vec![],
        hour: 9,
        minute: 0,
        missed: AgentMissedRunPolicy::RunOnce,
        pause_on_failure: true,
    };
    store
        .configure_calendar("one", "r", store.revision(), Some(schedule.clone()), 1)
        .unwrap();
    let mut routine = store.snapshot().routines[0].clone();
    routine.enabled = true;
    assert!(
        store
            .command(
                AgentCommand::SaveRoutine {
                    expected_revision: store.revision(),
                    routine: routine.clone()
                },
                1
            )
            .is_err()
    );
    // A calendar may be tested while paused. The due scheduler must not bypass that check.
    let revision = store.revision();
    store
        .command(
            AgentCommand::RunNow {
                expected_revision: revision,
                routine_id: "r".into(),
                request_id: "test-calendar".into(),
            },
            2,
        )
        .unwrap();
    let run = store.claim(2).unwrap().unwrap();
    assert_eq!(run.routine_id, "r");
    store.bind(&run.id, "codex#calendar", 3).unwrap();
    for status in [SessionStatus::Working, SessionStatus::Idle] {
        store
            .observe(
                "codex#calendar",
                &EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged { status },
                },
                4,
            )
            .unwrap();
    }
    assert!(!store.routine_tested(&routine));
    // Exercise calendar catch-up independent of the output verifier, by recording the
    // completed host test and its exact routine/profile/spec snapshot.
    let mut task = super::super::agent_work::tests::calendar_task(
        &run.id,
        &run.agent_id,
        &run.cwd,
        &run.prompt,
    );
    task.execution = interface_contracts::agent_work::AgentExecutionState::Completed;
    store.work.admit(task).unwrap();
    assert!(store.routine_tested(&routine));
    store
        .command(
            AgentCommand::SaveRoutine {
                expected_revision: store.revision(),
                routine: routine.clone(),
            },
            4,
        )
        .unwrap();
    let due = store.snapshot().routines[0].next_run_at_ms.unwrap();
    let catch_up = store.claim(due + 2 * 86_400_000).unwrap().unwrap();
    assert_eq!(catch_up.scheduled_at_ms, due);
    assert!(store.snapshot().routines[0].next_run_at_ms.unwrap() > due + 2 * 86_400_000);
    store
        .fail(&catch_up.id, "done", true, due + 2 * 86_400_000)
        .unwrap();
    let mut skip = schedule;
    skip.missed = AgentMissedRunPolicy::Skip;
    store
        .configure_calendar(
            "one",
            "r",
            store.revision(),
            Some(skip),
            due + 2 * 86_400_000,
        )
        .unwrap();
    routine = store.snapshot().routines[0].clone();
    routine.enabled = true;
    store
        .command(
            AgentCommand::SaveRoutine {
                expected_revision: store.revision(),
                routine,
            },
            due + 2 * 86_400_000,
        )
        .unwrap();
    let due = store.snapshot().routines[0].next_run_at_ms.unwrap();
    assert!(store.claim(due + 120_000).unwrap().is_none());
    assert!(store.snapshot().routines[0].next_run_at_ms.unwrap() > due + 120_000);
}

#[test]
fn permission_modes_live_beside_the_roster_and_follow_the_agent() {
    use interface_contracts::{agents::AgentPermissionMode, engine::AgentAccess};
    let (root, mut store) = one_agent();
    let roster = fs::read(root.path().join("agents.v2.json")).unwrap();
    store
        .save_permission_mode("one", AgentPermissionMode::Bypass)
        .unwrap();
    // The roster file an older build reads is untouched.
    assert_eq!(
        fs::read(root.path().join("agents.v2.json")).unwrap(),
        roster
    );
    assert_eq!(store.permission_mode("one"), AgentPermissionMode::Bypass);
    assert_eq!(
        store.response(&[]).permission_modes,
        std::collections::BTreeMap::from([("one".to_owned(), AgentPermissionMode::Bypass)])
    );
    assert!(
        store
            .save_permission_mode("ghost", AgentPermissionMode::Auto)
            .is_err()
    );
    store.set_home("one", "/fixture/agents/one", SHA_A).unwrap();
    store.bind_thread("one", "claude#thread", 3).unwrap();
    assert_eq!(
        store.binding("claude#thread", |_| true).unwrap().access,
        Some(AgentAccess::Bypass)
    );
    drop(store);
    let mut store = open_in(root.path()).unwrap();
    assert_eq!(store.permission_mode("one"), AgentPermissionMode::Bypass);
    store
        .save_permission_mode("one", AgentPermissionMode::Auto)
        .unwrap();
    assert_eq!(
        store.binding("claude#thread", |_| true).unwrap().access,
        Some(AgentAccess::Auto)
    );
    store
        .command(
            AgentCommand::RemoveAgent {
                expected_revision: revision(&store),
                id: "one".into(),
            },
            9,
        )
        .unwrap();
    assert!(store.response(&[]).permission_modes.is_empty());
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("agent-permissions.v1.json")).unwrap())
            .unwrap();
    assert_eq!(written["modes"], json!({}));
}

#[test]
fn written_bytes_match_the_pinned_format() {
    let (root, store) = fixture();
    let file = root.path().join("agents.v2.json");
    crate::golden::assert_golden("agents.saved.json", root.path(), &file);
    drop(store);
    let mut store = open_in(root.path()).unwrap();
    assert!(store.claim(1000).unwrap().is_some());
    crate::golden::assert_golden("agents.claimed.json", root.path(), &file);
    store
        .save_permission_mode("one", AgentPermissionMode::Auto)
        .unwrap();
    crate::golden::assert_golden(
        "agents.permissions.json",
        root.path(),
        &root.path().join("agent-permissions.v1.json"),
    );
    assert_eq!(
        store
            .reserve_start("request", "one", "first message")
            .unwrap(),
        crate::AgentStartAdmission::Admitted
    );
    crate::golden::assert_golden(
        "agents.starts.json",
        root.path(),
        &root.path().join("agent-starts.v1.json"),
    );
    for lock in [
        ".agents.lock",
        ".agent-work.lock",
        ".agent-starts.lock",
        ".agent-permissions.lock",
    ] {
        assert!(root.path().join(lock).exists(), "{lock}");
    }
}

fn task<'a>(request_id: &'a str, agent_id: &'a str, prompt: &'a str) -> AgentTaskRequest<'a> {
    AgentTaskRequest {
        request_id,
        agent_id,
        cwd: "/fixture/project",
        title: "Summarize the open issues",
        prompt,
        model: Some("gpt-6"),
        effort: Some(" "),
    }
}

#[test]
fn a_one_off_task_is_admitted_once_as_a_starting_gated_run() {
    let (root, mut store) = fixture();
    let long = "x".repeat(9_000);
    let AgentTaskAdmission::Admitted(run) =
        store.admit_task(&task("req-1", "one", &long), 50).unwrap()
    else {
        panic!("a new request is admitted");
    };
    assert_eq!(run.id, "task:req-1");
    assert!(run.routine_id.is_empty());
    assert_eq!(run.status, AgentRunStatus::Starting);
    assert_eq!(run.name, "Summarize the open issues");
    assert_eq!(run.cwd, "/fixture/project");
    // A model picked at launch applies to this task; a blank effort keeps the agent's.
    assert_eq!(run.profile.model, "gpt-6");
    assert_eq!(run.profile.effort, "");
    assert_eq!(run.profile.instructions, "Be precise.");
    // The run keeps a bounded copy; the launch sends the whole prompt.
    assert_eq!(run.prompt.chars().count(), 8_000);
    // The same request never admits twice.
    assert_eq!(
        store
            .admit_task(&task("req-1", "one", "again"), 60)
            .unwrap(),
        AgentTaskAdmission::Existing(run.clone())
    );
    assert_eq!(
        store
            .workspace()
            .runs
            .iter()
            .filter(|r| r.id == "task:req-1")
            .count(),
        1
    );
    // The scheduler never claims it: it is launched by the request that admitted it.
    assert!(
        store
            .claim(50)
            .unwrap()
            .is_none_or(|claimed| claimed.id != run.id)
    );
    // A restart before it was bound leaves it uncertain, never retried.
    drop(store);
    let reopened = open_in(root.path()).unwrap();
    let after = reopened
        .workspace()
        .runs
        .iter()
        .find(|r| r.id == "task:req-1")
        .unwrap();
    assert_eq!(after.status, AgentRunStatus::Unknown);
}

#[test]
fn a_one_off_task_refuses_bad_input_and_another_agents_request_id() {
    let (_root, mut store) = fixture();
    store.admit_task(&task("req-1", "one", "Go."), 50).unwrap();
    assert!(store.admit_task(&task("req-1", "two", "Go."), 50).is_err());
    assert!(
        store
            .admit_task(&task("req-2", "missing", "Go."), 50)
            .is_err()
    );
    assert!(store.admit_task(&task("req-3", "one", "   "), 50).is_err());
    assert!(store.admit_task(&task("", "one", "Go."), 50).is_err());
    let relative = AgentTaskRequest {
        cwd: "project",
        ..task("req-4", "one", "Go.")
    };
    assert!(store.admit_task(&relative, 50).is_err());
    let untitled = AgentTaskRequest {
        title: "  ",
        ..task("req-5", "one", "Go.")
    };
    let AgentTaskAdmission::Admitted(run) = store.admit_task(&untitled, 50).unwrap() else {
        panic!("admitted");
    };
    assert_eq!(run.name, "Task");
}
