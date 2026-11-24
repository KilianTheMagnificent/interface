use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use interface_contracts::{
    agents::{
        AgentDecision, AgentDecisionKind, AgentDeliveryOutcome, AgentPendingApproval, ApprovalFacts,
    },
    ipc::CHANNELS,
    terminal::{TerminalId, TerminalLaunchSessionV1, TerminalWorkspaceTerminal},
};
use interface_review::{
    REVIEW_GIT_UNAVAILABLE, ReviewCheckInvocation, ReviewCheckRunner, ReviewCheckRunnerResult,
    ReviewGitResult, ReviewGitRunner,
};
use interface_settings::{
    TerminalReopenAuthority, TerminalReopenAuthoritySlot, TerminalWorkspaceStore,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use super::{
    COVERED_OPERATIONS, DesktopService, DesktopServiceConfig, DesktopServiceReviewPorts,
    INTERNAL_DESKTOP_OPERATIONS, NATIVE_DIRECTORY_PICKER_START_PATH,
    RecoverableTerminalReopenAuthorityStore, SavedTerminalProviderSessionPersistence,
    TerminalReopenAuthorityState, TerminalReopenAuthorityStore,
};

#[derive(Default)]
struct FakeChecks {
    invocations: Mutex<Vec<ReviewCheckInvocation>>,
}

impl ReviewCheckRunner for FakeChecks {
    fn run(&self, invocation: &ReviewCheckInvocation) -> ReviewCheckRunnerResult {
        self.invocations
            .lock()
            .expect("invocation lock")
            .push(invocation.clone());
        ReviewCheckRunnerResult {
            exit_code: Some(0),
            stdout: "green\n".to_owned(),
            ..ReviewCheckRunnerResult::default()
        }
    }
}

#[derive(Default)]
struct MissingGit;

impl ReviewGitRunner for MissingGit {
    fn run(&self, _args: &[String], _cwd: &Path, _maximum: usize) -> ReviewGitResult {
        ReviewGitResult {
            code: REVIEW_GIT_UNAVAILABLE,
            ..ReviewGitResult::default()
        }
    }
}

struct ScriptedGit {
    results: Mutex<VecDeque<ReviewGitResult>>,
}

impl ScriptedGit {
    fn for_untracked(path: &str) -> Self {
        Self {
            results: Mutex::new(
                [
                    git_result("true\n"),
                    git_result("main\n"),
                    git_result("0123456789abcdef\n"),
                    git_result(&format!("?? {path}\0")),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }
}

impl ReviewGitRunner for ScriptedGit {
    fn run(&self, _args: &[String], _cwd: &Path, _maximum: usize) -> ReviewGitResult {
        self.results
            .lock()
            .expect("git results")
            .pop_front()
            .expect("one scripted result per git call")
    }
}

fn git_result(stdout: &str) -> ReviewGitResult {
    ReviewGitResult {
        code: 0,
        stdout: stdout.to_owned(),
        ..ReviewGitResult::default()
    }
}

#[cfg(unix)]
struct SwapBeforeRestore {
    target: PathBuf,
    parked: PathBuf,
    outside: PathBuf,
    fired: AtomicBool,
}

#[cfg(unix)]
impl crate::CheckpointHooks for SwapBeforeRestore {
    fn before_restore_rename(&self, relative: &str, _absolute: &Path) {
        use std::os::unix::fs::symlink;

        if relative == "note.txt" && !self.fired.swap(true, Ordering::SeqCst) {
            fs::rename(&self.target, &self.parked).expect("park raced target");
            symlink(&self.outside, &self.target).expect("replace raced target with link");
        }
    }
}

struct Fixture {
    _temp: TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
    user_customization: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp");
        let home = temp.path().join("home");
        let project = home.join("Desktop/Projects");
        let state = temp.path().join("state");
        let user_customization = temp.path().join("claude-user");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&user_customization).expect("user customization");
        Self {
            _temp: temp,
            home,
            project,
            state,
            user_customization,
        }
    }

    fn config(&self) -> DesktopServiceConfig {
        DesktopServiceConfig {
            appearance_file: self.state.join("appearance.json"),
            acp_profiles_file: self.state.join("acp-profiles.json"),
            terminal_workspaces_file: self.state.join("terminal-workspaces.json"),
            home_directory: self.home.clone(),
            customization_user_root: Some(self.user_customization.clone()),
            review_checkpoint_store_path: Some(self.state.join("review-checkpoints")),
        }
    }

    fn service(&self, review: DesktopServiceReviewPorts) -> DesktopService {
        DesktopService::open(self.config(), review).expect("open service")
    }
}

#[test]
fn terminal_roots_include_the_cold_default_without_granting_editor_authority() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let canonical_default = fs::canonicalize(&fixture.project).expect("canonical default");

    assert!(service.workspace_roots().is_empty());
    assert_eq!(
        service.terminal_workspace_roots(),
        vec![canonical_default.clone()]
    );

    let explicitly_granted = fixture.home.join("picked");
    fs::create_dir(&explicitly_granted).expect("picked root");
    let explicitly_granted = service
        .grant_workspace_root(&explicitly_granted)
        .expect("explicit grant");
    assert_eq!(
        service.terminal_workspace_roots(),
        std::collections::BTreeSet::from([canonical_default, explicitly_granted])
            .into_iter()
            .collect::<Vec<_>>()
    );
}

#[test]
fn create_project_creates_and_grants_the_exact_named_folder_from_the_shared_fixture() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let payload: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/project-creation-v26.json"
    ))
    .expect("shared project creation fixture");
    let result = service
        .dispatch_blocking(CHANNELS.create_project, payload)
        .expect("create");
    // New projects land in ~/Developer/Projects even while the old Desktop
    // folder (here the default directory) still exists.
    let project = fs::canonicalize(fixture.home.join("Developer/Projects/My new project"))
        .expect("created folder");
    assert_eq!(result, json!(project));
    assert!(service.workspace_roots().contains(&project));
    assert_eq!(fs::read_dir(project).expect("empty folder").count(), 0);
    assert_eq!(
        fs::read_dir(&fixture.project)
            .expect("Desktop folder")
            .count(),
        0
    );
}

#[test]
fn create_project_rejects_invalid_names_and_payloads_before_creating_projects() {
    let fixture = Fixture::new();
    let projects = fixture.home.join("Developer/Projects");
    let service = fixture.service(DesktopServiceReviewPorts::default());
    for name in [
        "",
        " ",
        ".",
        "..",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        "a:b",
        "line\nbreak",
        "nul\0byte",
        " spaced ",
        &"x".repeat(256),
    ] {
        assert!(
            service
                .dispatch_blocking(CHANNELS.create_project, json!({"name":name}))
                .is_err(),
            "{name:?}"
        );
        assert!(!projects.exists());
    }
    for payload in [
        json!({}),
        json!({"name":1}),
        json!({"name":"safe", "parent":"/tmp"}),
    ] {
        let error = service
            .dispatch_blocking(CHANNELS.create_project, payload)
            .expect_err("invalid payload");
        assert_eq!(error.code, "invalid-payload");
    }
    assert!(!projects.exists());
}

#[test]
fn create_project_preserves_existing_files_directories_and_symlinks() {
    let fixture = Fixture::new();
    let projects = fixture.home.join("Developer/Projects");
    fs::create_dir_all(&projects).expect("projects");
    let service = fixture.service(DesktopServiceReviewPorts::default());
    fs::create_dir(projects.join("Existing")).expect("existing directory");
    fs::write(projects.join("Existing/keep.txt"), "keep").expect("contents");
    fs::write(projects.join("File"), "keep").expect("existing file");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&fixture.home, projects.join("Link")).expect("link");
    for name in ["Existing", "File"] {
        let error = service
            .dispatch_blocking(CHANNELS.create_project, json!({"name":name}))
            .expect_err("collision");
        assert!(error.message.contains("already in use"));
    }
    #[cfg(unix)]
    assert!(
        service
            .dispatch_blocking(CHANNELS.create_project, json!({"name":"Link"}))
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(projects.join("Existing/keep.txt")).expect("contents"),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(projects.join("File")).expect("contents"),
        "keep"
    );
    assert!(service.workspace_roots().is_empty());
}

#[test]
fn create_project_builds_missing_projects_and_refuses_duplicate_submissions() {
    let fixture = Fixture::new();
    let projects = fixture.home.join("Developer/Projects");
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let first = service.dispatch_blocking(CHANNELS.create_project, json!({"name":"Café project"}));
    let second = service.dispatch_blocking(CHANNELS.create_project, json!({"name":"Café project"}));
    assert_ne!(first.is_ok(), second.is_ok());
    assert!(projects.join("Café project").is_dir());
    assert_eq!(fs::read_dir(&projects).expect("projects").count(), 1);
}

#[test]
fn new_project_creates_in_the_chosen_projects_folder_and_reads_it_back() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let read = |service: &DesktopService| {
        service
            .dispatch_blocking(CHANNELS.create_project, json!({"projectsFolder":"read"}))
            .expect("read the Projects folder")
    };
    // Nothing chosen, no Developer/Projects and nothing picked yet: the
    // default, which New project creates.
    let first = read(&service);
    assert_eq!(first["source"], "default");
    assert_eq!(first["display"], "~/Developer/Projects");
    assert_eq!(first["version"], 1);

    let code = fixture.home.join("code");
    fs::create_dir(&code).expect("code folder");
    let chosen = service
        .dispatch_blocking(
            CHANNELS.create_project,
            json!({"projectsFolder":"choose","selectedPath":code}),
        )
        .expect("choose");
    let canonical_code = fs::canonicalize(&code).expect("canonical code");
    assert_eq!(chosen["source"], "chosen");
    assert_eq!(chosen["path"], json!(canonical_code));
    assert_eq!(chosen["display"], "~/code");

    let created = service
        .dispatch_blocking(CHANNELS.create_project, json!({"name":"web"}))
        .expect("create");
    assert_eq!(created, json!(canonical_code.join("web")));
    assert!(!fixture.home.join("Developer/Projects").exists());

    // A cancelled picker changes nothing; a missing folder is refused.
    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.create_project,
                json!({"projectsFolder":"choose","selectedPath":null}),
            )
            .expect("cancelled choice")["source"],
        "chosen"
    );
    let missing = service
        .dispatch_blocking(
            CHANNELS.create_project,
            json!({"projectsFolder":"choose","selectedPath":fixture.home.join("gone")}),
        )
        .expect_err("missing folder");
    assert_eq!(missing.code, "request-refused");
    assert_eq!(read(&service)["path"], json!(canonical_code));

    // The choice survives a restart.
    drop(service);
    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(read(&reopened)["source"], "chosen");
}

#[test]
fn projects_folder_defaults_to_developer_projects_then_the_last_picked_parent() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let src = fixture.home.join("src");
    fs::create_dir_all(src.join("app")).expect("picked project");
    service
        .dispatch_blocking(
            CHANNELS.pick_directory,
            json!({"selectedPath": src.join("app")}),
        )
        .expect("pick");
    let view = service
        .dispatch_blocking(CHANNELS.create_project, json!({"projectsFolder":"read"}))
        .expect("read");
    assert_eq!(view["source"], "lastPicked");
    assert_eq!(
        view["path"],
        json!(fs::canonicalize(&src).expect("canonical src"))
    );

    // Once ~/Developer/Projects exists it wins over the last pick.
    fs::create_dir_all(fixture.home.join("Developer/Projects")).expect("developer projects");
    let view = service
        .dispatch_blocking(CHANNELS.create_project, json!({"projectsFolder":"read"}))
        .expect("read");
    assert_eq!(view["source"], "default");
    assert_eq!(view["display"], "~/Developer/Projects");

    for payload in [
        json!({"projectsFolder":"choose","selectedPath":"relative"}),
        json!({"projectsFolder":"write"}),
        json!({"projectsFolder":"read","name":"web"}),
    ] {
        let error = service
            .dispatch_blocking(CHANNELS.create_project, payload)
            .expect_err("invalid folder request");
        assert_eq!(error.code, "invalid-payload");
    }
}

#[test]
fn repair_terminal_titles_rewrites_the_roster_only_when_a_name_changes() {
    use std::os::unix::fs::MetadataExt;

    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                TerminalId::random(),
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("seed roster");
    let roster = fixture.state.join("terminal-workspaces.json");
    let inode = |path: &Path| fs::metadata(path).expect("roster metadata").ino();
    let seeded = inode(&roster);

    service
        .repair_terminal_titles(|terminal| Ok(terminal.title.clone()))
        .expect("unchanged repair");
    assert_eq!(inode(&roster), seeded, "an unchanged roster was rewritten");

    service
        .repair_terminal_titles(|_| Ok("Claude 2".to_owned()))
        .expect("renaming repair");
    assert_ne!(inode(&roster), seeded, "a repaired name was not saved");
    let saved = service
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
        .expect("saved roster");
    assert_eq!(saved[0]["terminals"][0]["title"], "Claude 2");
}

/// The saved Claude terminal [`saved_terminal_workspace_payload`] describes,
/// as the host's provider-reopen seam names it.
fn saved_claude_terminal(
    project: &Path,
    terminal_id: TerminalId,
    launch_session: TerminalLaunchSessionV1,
) -> TerminalWorkspaceTerminal {
    TerminalWorkspaceTerminal {
        id: terminal_id,
        title: "Claude".to_owned(),
        cwd: project.to_string_lossy().into_owned(),
        session_key: None,
        launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
            version: 1,
            profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
        }),
        launch_session: Some(launch_session),
    }
}

fn saved_terminal_workspace_payload(
    project: &Path,
    terminal_id: TerminalId,
    launch_session: &Value,
) -> Value {
    json!({"workspaces":[{
        "id":"workspace-1",
        "name":"Agents",
        "createdAt":"2026-09-02T00:00:00Z",
        "updatedAt":"2026-09-02T00:00:00Z",
        "terminals":[{
            "id":terminal_id,
            "title":"Claude",
            "cwd":project,
            "launch":{"version":1,"profileId":"claude"},
            "launchSession":launch_session
        }]
    }]})
}

#[test]
fn unavailable_authority_sidecar_never_classifies_an_unknown_id_as_unsaved() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces":[]}))
        .expect("persist an empty but valid roster");
    drop(service);

    fs::write(
        fixture
            .state
            .join(".terminal-workspaces.json.host-authority"),
        b"{malformed",
    )
    .expect("corrupt authority fixture");
    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        reopened.terminal_reopen_authority_state(TerminalId::random()),
        TerminalReopenAuthorityState::Unavailable
    );
}

#[test]
fn unsaved_provider_binding_commits_to_authority_before_returning_not_saved() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let root = fixture.project.to_string_lossy().into_owned();
    let spawned = saved_claude_terminal(
        &fixture.project,
        terminal_id,
        TerminalLaunchSessionV1 {
            version: 1,
            id: None,
            resume: true,
        },
    );
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: root.clone(),
            managed_worktree_key: Some("claude#unsaved".to_owned()),
            terminal: spawned,
        })
        .expect("seed live spawn authority");
    let confirmed = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
        resume: true,
    };

    assert_eq!(
        service
            .persist_terminal_provider_reopen_identity(
                &root,
                &saved_claude_terminal(&fixture.project, terminal_id, confirmed.clone()),
            )
            .expect("authority update is durable even before roster save"),
        SavedTerminalProviderSessionPersistence::NotSaved
    );
    assert_eq!(
        service
            .live_terminal_reopen_authority(terminal_id)
            .expect("authority store remains available")
            .expect("authority remains present")
            .terminal
            .launch_session,
        Some(confirmed.clone())
    );

    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("renderer later persists roster membership");
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    let TerminalReopenAuthorityState::Authority(authority) =
        reopened.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("cold restart must retain the host proof");
    };
    assert_eq!(authority.terminal.launch_session, Some(confirmed));
}

#[test]
fn recovered_manual_claude_repairs_an_authority_ahead_shell_roster() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let root = fixture.project.to_string_lossy().into_owned();
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces":[{
                "id":"workspace-1",
                "name":"Agents",
                "createdAt":"2026-09-03T00:00:00Z",
                "updatedAt":"2026-09-03T00:00:00Z",
                "terminals":[{
                    "id":terminal_id,
                    "title":"Shell",
                    "cwd":root
                }]
            }]}),
        )
        .expect("seed shell roster");
    let shell = TerminalWorkspaceTerminal {
        id: terminal_id,
        title: "Shell".to_owned(),
        cwd: root.clone(),
        session_key: None,
        launch: None,
        launch_session: None,
    };
    let shell_authority = TerminalReopenAuthority {
        root: root.clone(),
        managed_worktree_key: None,
        terminal: shell,
    };
    service
        .record_terminal_reopen_authority_value(&shell_authority)
        .expect("seed shell authority");

    let confirmed = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("43434343-4343-4343-8343-434343434343".to_owned()),
        resume: true,
    };
    let mut recovered = shell_authority.terminal;
    recovered.launch = Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
        version: 1,
        profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
    });
    recovered.launch_session = Some(confirmed.clone());
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: root.clone(),
            managed_worktree_key: None,
            terminal: recovered.clone(),
        })
        .expect("simulate authority-first crash winner");

    assert_eq!(
        service
            .persist_terminal_provider_reopen_identity(&root, &recovered)
            .expect("repair saved roster from authority-ahead state"),
        SavedTerminalProviderSessionPersistence::Durable
    );
    let saved = service
        .terminal_state()
        .expect("workspace state")
        .workspaces
        .get()
        .into_iter()
        .flat_map(|workspace| workspace.terminals)
        .find(|terminal| terminal.id == terminal_id)
        .expect("saved terminal");
    assert_eq!(saved.launch, recovered.launch);
    assert_eq!(saved.launch_session, Some(confirmed));
}

#[test]
#[allow(clippy::too_many_lines)] // One rollback fixture exercises both durable CAS transitions.
fn authority_rollback_reloads_the_exact_attempt_and_restores_prior() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let prior_session = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
        resume: true,
    };
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &serde_json::to_value(&prior_session).expect("prior session JSON"),
            ),
        )
        .expect("seed saved terminal");
    let prior = TerminalReopenAuthority {
        root: fixture.project.to_string_lossy().into_owned(),
        managed_worktree_key: Some("claude#prior".to_owned()),
        terminal: TerminalWorkspaceTerminal {
            id: terminal_id,
            title: "Claude".to_owned(),
            cwd: fixture.project.to_string_lossy().into_owned(),
            session_key: None,
            launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
                version: 1,
                profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
            }),
            launch_session: Some(prior_session),
        },
    };
    service
        .record_terminal_reopen_authority_value(&prior)
        .expect("seed exact prior proof");

    // Inject the observable state of a post-rename failure: durable bytes
    // contain the attempted commit, while the service discarded its old
    // in-memory writer because the call outcome was uncertain.
    let authority_file = fixture
        .state
        .join(".terminal-workspaces.json.host-authority");
    let mut disk =
        TerminalReopenAuthorityStore::open(&authority_file).expect("open authority disk state");
    disk.load().expect("load prior authority");
    let mut uncertain_winner = prior.clone();
    uncertain_winner.managed_worktree_key = Some("claude#failed".to_owned());
    uncertain_winner.terminal.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("22222222-2222-4222-8222-222222222222".to_owned()),
        resume: true,
    });
    disk.upsert(&uncertain_winner)
        .expect("install post-rename winner");
    drop(disk);
    service
        .terminal_state
        .lock()
        .expect("authority state")
        .authorities = RecoverableTerminalReopenAuthorityStore::Unavailable;

    assert!(
        service
            .rollback_terminal_reopen_authority(
                terminal_id,
                &TerminalReopenAuthoritySlot::Authority(prior.clone()),
                &TerminalReopenAuthoritySlot::Authority(uncertain_winner),
            )
            .expect("restore prior authority with readback")
    );
    assert_eq!(
        service
            .live_terminal_reopen_authority(terminal_id)
            .expect("restored store is ready"),
        Some(prior.clone())
    );

    let transient_id = TerminalId::random();
    let mut transient = prior.clone();
    transient.terminal.id = transient_id;
    service
        .record_terminal_reopen_authority_value(&transient)
        .expect("seed uncommitted new-terminal proof");
    service
        .terminal_state
        .lock()
        .expect("authority state")
        .authorities = RecoverableTerminalReopenAuthorityStore::Unavailable;
    assert!(
        service
            .rollback_terminal_reopen_authority(
                transient_id,
                &TerminalReopenAuthoritySlot::Absent,
                &TerminalReopenAuthoritySlot::Authority(transient),
            )
            .expect("remove proof for a transaction with no prior value")
    );
    assert_eq!(
        service
            .live_terminal_reopen_authority(transient_id)
            .expect("store remains ready"),
        None
    );
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        reopened.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Authority(prior)
    );
    assert_eq!(
        reopened.terminal_reopen_authority_state(transient_id),
        TerminalReopenAuthorityState::Unsaved
    );
}

#[test]
fn authority_rollback_preserves_and_reloads_a_third_winner() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("seed saved terminal");
    let terminal = TerminalWorkspaceTerminal {
        id: terminal_id,
        title: "Claude".to_owned(),
        cwd: fixture.project.to_string_lossy().into_owned(),
        session_key: None,
        launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
            version: 1,
            profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
        }),
        launch_session: Some(TerminalLaunchSessionV1 {
            version: 1,
            id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
            resume: true,
        }),
    };
    let prior = TerminalReopenAuthority {
        root: fixture.project.to_string_lossy().into_owned(),
        managed_worktree_key: Some("claude#prior".to_owned()),
        terminal: terminal.clone(),
    };
    service
        .record_terminal_reopen_authority_value(&prior)
        .expect("seed prior authority");
    let mut attempted = prior.clone();
    attempted.managed_worktree_key = Some("claude#attempted".to_owned());
    let mut winner = prior.clone();
    winner.managed_worktree_key = Some("claude#winner".to_owned());
    winner.terminal.launch_session = Some(TerminalLaunchSessionV1 {
        version: 1,
        id: Some("33333333-3333-4333-8333-333333333333".to_owned()),
        resume: true,
    });

    let authority_file = fixture
        .state
        .join(".terminal-workspaces.json.host-authority");
    let mut disk =
        TerminalReopenAuthorityStore::open(&authority_file).expect("open authority disk");
    disk.load().expect("load prior authority");
    disk.upsert(&winner).expect("install third winner");
    drop(disk);
    service
        .terminal_state
        .lock()
        .expect("authority state")
        .authorities = RecoverableTerminalReopenAuthorityStore::Unavailable;

    let error = service
        .rollback_terminal_reopen_authority(
            terminal_id,
            &TerminalReopenAuthoritySlot::Authority(prior),
            &TerminalReopenAuthoritySlot::Authority(attempted),
        )
        .expect_err("rollback must refuse a third winner");
    assert_eq!(error.code, "terminal-reopen-authority-rollback-conflict");
    assert_eq!(
        service
            .live_terminal_reopen_authority(terminal_id)
            .expect("preserved winner remains readable"),
        Some(winner.clone())
    );

    let mut reopened =
        TerminalReopenAuthorityStore::open(&authority_file).expect("reopen authority disk");
    reopened.load().expect("load preserved winner");
    assert_eq!(
        reopened.slot(terminal_id),
        TerminalReopenAuthoritySlot::Authority(winner)
    );
}

#[test]
fn poisoned_roster_reloads_disk_before_an_explicit_repair_save() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let provisional = json!({"version":1,"resume":true});
    let stale_payload =
        saved_terminal_workspace_payload(&fixture.project, terminal_id, &provisional);
    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, stale_payload.clone())
        .expect("seed provisional roster");

    {
        let mut state = service
            .terminal_state
            .lock()
            .expect("terminal workspace store");
        let workspaces = &mut state.workspaces;
        let mut invalid = workspaces.get();
        invalid[0].name.clear();
        assert!(
            workspaces
                .commit_prepared(&service.terminal_workspaces_file, &invalid)
                .is_err(),
            "a failed write must poison the possibly stale in-memory store"
        );
        assert!(!workspaces.is_available());
    }

    // Model the only safe assumption after a post-rename error: disk may
    // already contain the provider-confirmed winner even though the
    // discarded in-memory store did not advance.
    let confirmed = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
        resume: true,
    };
    let winner = saved_claude_terminal(&fixture.project, terminal_id, confirmed.clone());
    let mut disk =
        TerminalWorkspaceStore::open(&service.terminal_workspaces_file).expect("open disk winner");
    disk.load().expect("load disk winner");
    assert!(
        disk.reconcile_terminal_provider_reopen(
            terminal_id,
            &winner.cwd,
            winner.launch,
            winner.launch_session.as_ref(),
        )
        .expect("advance disk beyond discarded memory")
    );

    let repaired = service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, stale_payload)
        .expect("explicit save reloads and repairs poisoned roster");
    assert_eq!(
        repaired[0]["terminals"][0]["launchSession"],
        json!({
            "version":1,
            "id":"11111111-1111-4111-8111-111111111111",
            "resume":true
        })
    );
    assert_eq!(
        service
            .terminal_state()
            .expect("workspace state")
            .workspaces
            .saved_terminal(terminal_id),
        Some(winner)
    );
}

#[test]
fn failed_migration_resolution_poison_is_reloaded_only_by_explicit_roster_save() {
    let fixture = Fixture::new();
    let terminal_id = TerminalId::random();
    let payload = saved_terminal_workspace_payload(
        &fixture.project,
        terminal_id,
        &json!({"version":1,"resume":true}),
    );
    let records = serde_json::from_value::<
        Vec<interface_contracts::terminal::TerminalWorkspaceRecord>,
    >(payload["workspaces"].clone())
    .expect("decode roster fixture");
    let mut roster = TerminalWorkspaceStore::open(&fixture.state.join("terminal-workspaces.json"))
        .expect("open roster fixture");
    roster.save(&records).expect("save legacy roster fixture");
    drop(roster);

    let service = fixture.service(DesktopServiceReviewPorts::default());
    let TerminalReopenAuthorityState::PendingMigration(pending) =
        service.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("legacy saved terminal must begin as a migration candidate");
    };
    let invalid = TerminalReopenAuthority {
        root: "relative-root".to_owned(),
        managed_worktree_key: Some("claude#invalid".to_owned()),
        terminal: pending,
    };
    service
        .resolve_terminal_reopen_authority_migration(terminal_id, Some(&invalid))
        .expect_err("invalid resolution must fail");
    assert_eq!(
        service.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Unavailable,
        "a failed resolution must not keep serving its old in-memory candidate"
    );

    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, payload)
        .expect("explicit roster save reloads the unchanged sidecar");
    assert!(matches!(
        service.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::PendingMigration(_)
    ));
}

#[test]
fn terminal_provider_reopen_is_host_owned_and_durable() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let root = fixture.project.to_string_lossy().into_owned();
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("seed saved terminal");
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: root.clone(),
            managed_worktree_key: None,
            terminal: saved_claude_terminal(
                &fixture.project,
                terminal_id,
                TerminalLaunchSessionV1 {
                    version: 1,
                    id: None,
                    resume: true,
                },
            ),
        })
        .expect("seed spawn authority");

    let confirmed = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
        resume: true,
    };
    let confirmed_terminal = saved_claude_terminal(&fixture.project, terminal_id, confirmed);
    for expectation in [
        "persist provider-confirmed binding",
        "same binding is already durable",
    ] {
        assert_eq!(
            service
                .persist_terminal_provider_reopen_identity(&root, &confirmed_terminal)
                .expect(expectation),
            SavedTerminalProviderSessionPersistence::Durable
        );
    }
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    let workspaces = reopened
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
        .expect("read reconciled roster");
    assert_eq!(
        workspaces[0]["terminals"][0]["launchSession"],
        json!({
            "version":1,
            "id":"11111111-1111-4111-8111-111111111111",
            "resume":true
        })
    );
    let TerminalReopenAuthorityState::Authority(authority) =
        reopened.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("the host proof must survive service reopen");
    };
    assert_eq!(authority.terminal, confirmed_terminal);
}

#[test]
fn stale_full_roster_cannot_roll_back_a_host_confirmed_provider_session() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let root = fixture.project.to_string_lossy().into_owned();
    let stale_confirmed = json!({
        "version":1,
        "id":"00000000-0000-4000-8000-000000000000",
        "resume":true
    });
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(&fixture.project, terminal_id, &stale_confirmed),
        )
        .expect("seed stale saved terminal");
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: root.clone(),
            managed_worktree_key: None,
            terminal: saved_claude_terminal(
                &fixture.project,
                terminal_id,
                serde_json::from_value(stale_confirmed.clone()).expect("stale session"),
            ),
        })
        .expect("seed stale authority");

    let confirmed = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("11111111-1111-4111-8111-111111111111".to_owned()),
        resume: true,
    };
    assert_eq!(
        service
            .persist_terminal_provider_reopen_identity(
                &root,
                &saved_claude_terminal(&fixture.project, terminal_id, confirmed),
            )
            .expect("host advances provider binding"),
        SavedTerminalProviderSessionPersistence::Durable
    );

    for stale_binding in [stale_confirmed, json!({"version":1,"resume":true})] {
        let stale_save = service
            .dispatch_blocking(
                CHANNELS.set_terminal_workspaces,
                saved_terminal_workspace_payload(&fixture.project, terminal_id, &stale_binding),
            )
            .expect("stale renderer save");
        assert_eq!(
            stale_save[0]["terminals"][0]["launchSession"],
            json!({
                "version":1,
                "id":"11111111-1111-4111-8111-111111111111",
                "resume":true
            })
        );
    }

    let removed = service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces":[{
                "id":"workspace-1",
                "name":"Agents",
                "createdAt":"2026-09-02T00:00:00Z",
                "updatedAt":"2026-09-02T00:00:00Z",
                "terminals":[]
            }]}),
        )
        .expect("renderer terminal removal");
    assert_eq!(removed[0]["terminals"], json!([]));
    assert_eq!(
        service
            .terminal_state()
            .expect("workspace state")
            .workspaces
            .saved_terminal(terminal_id),
        None
    );
}

#[test]
fn stale_full_roster_cannot_undo_a_host_persisted_shell_demotion() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    let root = fixture.project.to_string_lossy().into_owned();
    let confirmed = TerminalLaunchSessionV1 {
        version: 1,
        id: Some("12121212-1212-4212-8212-121212121212".to_owned()),
        resume: true,
    };
    let stale_claude = TerminalWorkspaceTerminal {
        id: terminal_id,
        title: "Claude".to_owned(),
        cwd: root.clone(),
        session_key: None,
        launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
            version: 1,
            profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
        }),
        launch_session: Some(confirmed.clone()),
    };
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &serde_json::to_value(&confirmed).expect("confirmed session JSON"),
            ),
        )
        .expect("seed confirmed Claude roster");
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: root.clone(),
            managed_worktree_key: None,
            terminal: stale_claude.clone(),
        })
        .expect("seed confirmed Claude authority");

    let shell = TerminalWorkspaceTerminal {
        launch: None,
        launch_session: None,
        ..stale_claude
    };
    assert_eq!(
        service
            .persist_terminal_provider_reopen_identity(&root, &shell)
            .expect("host persists manual-Claude shell demotion"),
        SavedTerminalProviderSessionPersistence::Durable
    );

    let stale_save = service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &serde_json::to_value(&confirmed).expect("stale confirmed session JSON"),
            ),
        )
        .expect("stale renderer roster is reconciled");
    assert_eq!(stale_save[0]["terminals"][0]["launch"], Value::Null);
    assert_eq!(stale_save[0]["terminals"][0]["launchSession"], Value::Null);
    let TerminalReopenAuthorityState::Authority(authority) =
        service.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("host shell authority must survive the stale renderer save");
    };
    assert_eq!(authority.terminal.launch, None);
    assert_eq!(authority.terminal.launch_session, None);
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    let saved = reopened
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
        .expect("reopen reconciled roster");
    assert_eq!(saved[0]["terminals"][0]["launch"], Value::Null);
    assert_eq!(saved[0]["terminals"][0]["launchSession"], Value::Null);
    let TerminalReopenAuthorityState::Authority(authority) =
        reopened.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("durable shell authority must survive service reopen");
    };
    assert_eq!(authority.terminal.launch, None);
    assert_eq!(authority.terminal.launch_session, None);
}

#[test]
fn malformed_terminal_roster_preserves_authority_and_blocks_worktree_removal() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let terminal_id = TerminalId::random();
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("seed saved terminal");
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: fixture.project.to_string_lossy().into_owned(),
            managed_worktree_key: Some("claude#saved".to_owned()),
            terminal: TerminalWorkspaceTerminal {
                id: terminal_id,
                title: "Claude".to_owned(),
                cwd: fixture.project.to_string_lossy().into_owned(),
                session_key: None,
                launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
                    version: 1,
                    profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
                }),
                launch_session: Some(TerminalLaunchSessionV1 {
                    version: 1,
                    id: None,
                    resume: true,
                }),
            },
        })
        .expect("seed host authority");
    drop(service);

    let authority_file = fixture
        .state
        .join(".terminal-workspaces.json.host-authority");
    let authority_before = fs::read(&authority_file).expect("authority bytes");
    fs::write(
        fixture.state.join("terminal-workspaces.json"),
        b"{malformed",
    )
    .expect("corrupt roster fixture");

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        reopened.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Unavailable
    );
    assert!(reopened.saved_terminal_cwds().is_none());
    let Err(removal_error) = reopened.reserve_workspace_removal(&fixture.project) else {
        panic!("unavailable roster must fail closed");
    };
    assert!(removal_error.retryable);
    assert_eq!(
        fs::read(&authority_file).expect("preserved authority bytes"),
        authority_before
    );

    reopened
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("explicit validated roster save repairs the transaction");
    assert!(matches!(
        reopened.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Authority(_)
    ));
    drop(reopened);

    let repaired = fixture.service(DesktopServiceReviewPorts::default());
    assert!(matches!(
        repaired.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Authority(_)
    ));
}

#[test]
fn explicit_roster_save_repairs_malformed_pair_without_minting_migration_authority() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, json!({"workspaces":[]}))
        .expect("seed valid stores");
    drop(service);

    fs::write(
        fixture.state.join("terminal-workspaces.json"),
        b"{malformed",
    )
    .expect("corrupt roster fixture");
    fs::write(
        fixture
            .state
            .join(".terminal-workspaces.json.host-authority"),
        b"{malformed",
    )
    .expect("corrupt authority fixture");

    let terminal_id = TerminalId::random();
    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        reopened.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Unavailable
    );
    reopened
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(
                &fixture.project,
                terminal_id,
                &json!({"version":1,"resume":true}),
            ),
        )
        .expect("explicit validated save repairs both stores");
    assert_eq!(
        reopened.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Unproven,
        "renderer repair must not mint host proof or pending migration"
    );
    drop(reopened);

    let repaired = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        repaired.terminal_reopen_authority_state(terminal_id),
        TerminalReopenAuthorityState::Unproven,
        "the repaired sidecar must remain initialized across restart"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn settings_wire_shapes_and_restart_semantics_match_electron() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());

    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, json!({"systemDark":true}))
            .expect("appearance"),
        json!({"preference":"system","resolved":"dark"})
    );
    let invalid_empty = service
        .dispatch_blocking(CHANNELS.get_appearance, json!({}))
        .expect_err("arrays are not empty payloads");
    assert_eq!(invalid_empty.code, "invalid-payload");

    let changed = service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"preference":"sauteed-paper-light","systemDark":true}),
        )
        .expect("set appearance");
    // A retired palette is still accepted and lands on the theme that replaced it.
    assert_eq!(
        changed,
        json!({"preference":"anthropic-light","resolved":"anthropic-light"})
    );
    let strict = service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"preference":"light","systemDark":false,"extra":true}),
        )
        .expect_err("unknown native context fields are refused");
    assert_eq!(strict.code, "invalid-payload");

    let profile = json!({
        "slug":"local-agent",
        "displayName":"  Local Agent  ",
        "executablePath":"/opt/local/bin/agent",
        "args":["acp","stdio"]
    });
    let first = service
        .dispatch_blocking(
            CHANNELS.set_acp_profiles,
            json!({"profiles":[profile.clone()]}),
        )
        .expect("save profiles");
    assert_eq!(first["restartRequired"], true);
    assert_eq!(first["profiles"][0]["displayName"], "Local Agent");
    assert!(service.active_acp_profiles().is_empty());
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(reopened.active_acp_profiles().len(), 1);
    assert_eq!(reopened.active_acp_profiles()[0].slug, "local-agent");
    let second = reopened
        .dispatch_blocking(CHANNELS.set_acp_profiles, json!({"profiles":[profile]}))
        .expect("same startup roster");
    assert_eq!(second["restartRequired"], false);

    let invalid = reopened
        .dispatch_blocking(
            CHANNELS.set_acp_profiles,
            json!({"profiles":[{
                "slug":"grok",
                "displayName":"Grok",
                "executablePath":"/opt/grok",
                "args":[]
            }]}),
        )
        .expect_err("reserved provider");
    assert_eq!(invalid.code, "invalid-acp-profiles");
    assert_eq!(
        invalid.message,
        "Invalid ACP profile configuration: slug \"grok\" is reserved for Interface's built-in ACP provider"
    );
    let wrong_roster = reopened
        .dispatch_blocking(
            CHANNELS.set_acp_profiles,
            json!({"profiles":{"slug":"not-an-array"}}),
        )
        .expect_err("roster type");
    assert_eq!(
        wrong_roster.message,
        "Invalid ACP profile configuration: profiles must be an array"
    );
    let wrong_arg = reopened
        .dispatch_blocking(
            CHANNELS.set_acp_profiles,
            json!({"profiles":[{
                "slug":"local-two",
                "displayName":"Local Two",
                "executablePath":"/opt/local/bin/agent",
                "args":[7]
            }]}),
        )
        .expect_err("argv type");
    assert_eq!(
        wrong_arg.message,
        "Invalid ACP profile configuration: each argv entry must be a string of at most 1024 characters without NUL"
    );
}

#[test]
fn internal_directory_picker_policy_uses_configured_home_and_strict_payload() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let preferred = fixture.project.join("picker-start");
    fs::create_dir(&preferred).expect("preferred picker directory");

    assert_eq!(
        service
            .dispatch_blocking(
                NATIVE_DIRECTORY_PICKER_START_PATH,
                json!({"preferredPath": preferred.to_string_lossy()}),
            )
            .expect("directory picker policy"),
        json!(
            std::path::absolute(&preferred)
                .expect("absolute preferred path")
                .to_string_lossy()
        )
    );
    for payload in [
        Value::Null,
        json!({}),
        json!({"preferredPath": preferred.to_string_lossy(), "extra": true}),
        json!({"preferredPath": ""}),
    ] {
        assert_eq!(
            service
                .dispatch_blocking(NATIVE_DIRECTORY_PICKER_START_PATH, payload)
                .expect_err("strict internal picker payload")
                .code,
            "invalid-payload"
        );
    }
    assert_eq!(
        INTERNAL_DESKTOP_OPERATIONS,
        [NATIVE_DIRECTORY_PICKER_START_PATH]
    );
    assert!(!COVERED_OPERATIONS.contains(&NATIVE_DIRECTORY_PICKER_START_PATH));
}

#[cfg(unix)]
#[test]
fn unsafe_acp_profile_target_degrades_to_builtins_and_can_be_repaired() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.state).expect("state directory");
    let outside = fixture.home.join("outside-profiles.json");
    fs::write(&outside, b"{\"version\":1,\"profiles\":[]}\n").expect("outside profile file");
    let linked = fixture.state.join("acp-profiles.json");
    symlink(&outside, &linked).expect("unsafe profile link");

    let service = fixture.service(DesktopServiceReviewPorts::default());
    assert!(service.active_acp_profiles().is_empty());
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_acp_profiles, Value::Null)
            .expect("built-in-only roster"),
        json!([])
    );
    assert!(
        service
            .dispatch_blocking(CHANNELS.set_acp_profiles, json!({"profiles": []}))
            .is_err(),
        "the unsafe target stays untouched"
    );

    fs::remove_file(&linked).expect("remove unsafe profile link");
    let repaired = service
        .dispatch_blocking(CHANNELS.set_acp_profiles, json!({"profiles": []}))
        .expect("repair profile store");
    assert_eq!(repaired, json!({"profiles": [], "restartRequired": false}));
    assert!(
        !fs::symlink_metadata(&linked)
            .expect("repaired profile file")
            .file_type()
            .is_symlink()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn default_and_picker_grants_bound_every_workspace_operation() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.project.join("folder")).expect("folder");
    fs::write(fixture.project.join("a.txt"), "Alpha needle\n").expect("a");
    fs::write(fixture.project.join("z.txt"), "Zulu\n").expect("z");
    fs::create_dir(fixture.project.join("node_modules")).expect("excluded");
    fs::write(fixture.project.join("node_modules/hidden.txt"), "needle").expect("hidden");

    let service = fixture.service(DesktopServiceReviewPorts::default());
    let default = service
        .dispatch_blocking(CHANNELS.get_default_directory, json!({}))
        .expect("default");
    let canonical_project = fs::canonicalize(&fixture.project).expect("canonical project");
    assert_eq!(default, json!(canonical_project.to_string_lossy()));

    let listing = service
        .dispatch_blocking(
            CHANNELS.list_dir,
            json!({"path": fixture.project.to_string_lossy()}),
        )
        .expect("list directory");
    assert_eq!(listing[0], json!({"name":"folder","dir":true}));
    assert_eq!(listing[1], json!({"name":"a.txt","dir":false}));
    assert_eq!(listing[2], json!({"name":"z.txt","dir":false}));

    let snapshot = service
        .dispatch_blocking(
            CHANNELS.read_file_snapshot,
            json!({"path": fixture.project.join("a.txt").to_string_lossy()}),
        )
        .expect("snapshot");
    assert_eq!(snapshot["text"], "Alpha needle\n");
    assert_eq!(snapshot["revision"].as_str().map(str::len), Some(64));

    let saved = service
        .dispatch_blocking(
            CHANNELS.save_file,
            json!({
                "path":fixture.project.join("a.txt").to_string_lossy(),
                "text":"Changed\n",
                "expectedRevision":snapshot["revision"]
            }),
        )
        .expect("save");
    assert_eq!(saved["text"], "Changed\n");
    let stale = service
        .dispatch_blocking(
            CHANNELS.save_file,
            json!({
                "path":fixture.project.join("a.txt").to_string_lossy(),
                "text":"Must not land\n",
                "expectedRevision":snapshot["revision"]
            }),
        )
        .expect_err("stale save");
    assert!(stale.message.starts_with("File changed on disk.\n"));
    assert_eq!(
        fs::read_to_string(fixture.project.join("a.txt")).expect("read fixture"),
        "Changed\n"
    );

    let indexed = service
        .dispatch_blocking(
            CHANNELS.list_project_files,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("index");
    assert_eq!(indexed["files"].as_array().map(Vec::len), Some(2));
    assert_eq!(indexed["truncated"], false);

    let searched = service
        .dispatch_blocking(
            CHANNELS.search_project,
            json!({"root":fixture.project.to_string_lossy(),"query":"zulu"}),
        )
        .expect("search");
    assert_eq!(searched["matches"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        searched["matches"][0]["path"],
        fs::canonicalize(fixture.project.join("z.txt"))
            .expect("canonical result")
            .to_string_lossy()
            .as_ref()
    );

    let outside = fixture.home.join("outside");
    fs::create_dir(&outside).expect("outside");
    fs::write(outside.join("ordinary.txt"), "not authorized").expect("outside file");
    let refused = service
        .dispatch_blocking(
            CHANNELS.read_file,
            json!({"path":outside.join("ordinary.txt").to_string_lossy()}),
        )
        .expect_err("outside refusal");
    assert!(refused.message.starts_with("Outside your open folders.\n"));

    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.pick_directory,
                json!({"selectedPath":outside.to_string_lossy()}),
            )
            .expect("picked"),
        json!(
            fs::canonicalize(&outside)
                .expect("canonical outside")
                .to_string_lossy()
        )
    );
    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.read_file,
                json!({"path":outside.join("ordinary.txt").to_string_lossy()}),
            )
            .expect("picked root authorizes file"),
        "not authorized"
    );
}

#[cfg(unix)]
#[test]
fn linked_roots_and_exact_root_escalation_fail_closed() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    service
        .dispatch_blocking(CHANNELS.get_default_directory, Value::Null)
        .expect("default grant");
    let child = fixture.project.join("child");
    fs::create_dir(&child).expect("child");
    let exact = service
        .dispatch_blocking(
            CHANNELS.list_customization,
            json!({"root":child.to_string_lossy()}),
        )
        .expect_err("child is not exact root");
    assert_eq!(exact.code, "request-refused");
    assert!(
        exact
            .message
            .contains("exact current session or picked workspace root")
    );

    let outside = fixture.home.join("outside");
    fs::create_dir(&outside).expect("outside");
    let linked = fixture.project.join("linked");
    symlink(&outside, &linked).expect("link");
    let escaped = service
        .dispatch_blocking(
            CHANNELS.list_project_files,
            json!({"root":linked.to_string_lossy()}),
        )
        .expect_err("link escape");
    assert!(escaped.message.starts_with("Outside your open folders.\n"));
    assert!(service.grant_workspace_root(&linked).is_err());
}

#[test]
fn customization_and_review_are_exact_bounded_adapters() {
    let fixture = Fixture::new();
    fs::write(
        fixture.project.join("AGENTS.md"),
        "# Project rules\nKeep parity.\n",
    )
    .expect("rules");
    fs::write(
        fixture.project.join("package.json"),
        r#"{"scripts":{"check":"npm test","postinstall":"never"}}"#,
    )
    .expect("package");
    let checks = Arc::new(FakeChecks::default());
    let service = fixture.service(DesktopServiceReviewPorts {
        git_runner: Some(Arc::new(MissingGit)),
        check_runner: Some(checks.clone()),
        checkpoint_hooks: None,
    });
    service
        .dispatch_blocking(CHANNELS.get_default_directory, Value::Null)
        .expect("grant");

    let customization = service
        .dispatch_blocking(
            CHANNELS.list_customization,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("customization");
    assert_eq!(
        customization["root"],
        fs::canonicalize(&fixture.project)
            .expect("canonical project")
            .to_string_lossy()
            .as_ref()
    );
    assert!(
        customization["entries"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
    );

    let discovered = service
        .dispatch_blocking(
            CHANNELS.discover_review_checks,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("discover");
    assert_eq!(
        discovered,
        json!({
            "root":fs::canonicalize(&fixture.project)
                .expect("canonical project")
                .to_string_lossy(),
            "packagePresent":true,
            "checks":["check"]
        })
    );

    let ran = service
        .dispatch_blocking(
            CHANNELS.run_review_checks,
            json!({"root":fixture.project.to_string_lossy(),"selected":["check"]}),
        )
        .expect("run");
    assert_eq!(ran[0]["name"], "check");
    assert_eq!(ran[0]["status"], "passed");
    assert_eq!(ran[0]["stdout"], "green\n");
    assert!(ran[0].get("command").is_none());
    {
        let invocation = checks.invocations.lock().expect("invocations");
        assert_eq!(invocation.len(), 1);
        assert_eq!(invocation[0].executable, "npm");
        assert_eq!(invocation[0].args, ["run", "--silent", "check"]);
    }

    let review = service
        .dispatch_blocking(
            CHANNELS.read_repository_review,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("review is a normal unavailable result");
    assert_eq!(review["repository"], false);
    assert_eq!(review["error"]["code"], "git-unavailable");
}

#[test]
fn electron_context_and_workspace_ownership_are_strict_named_payloads() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());

    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.pick_directory, json!({"selectedPath":null}))
            .expect("picker cancellation"),
        Value::Null
    );
    let missing_picker_field = service
        .dispatch_blocking(CHANNELS.pick_directory, json!({}))
        .expect_err("selectedPath is required even when null");
    assert_eq!(missing_picker_field.code, "invalid-payload");

    let lexical = fixture.project.join("nested/../draft-root");
    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.set_workspace_open,
                json!({"root":lexical.to_string_lossy(),"open":true}),
            )
            .expect("protect draft"),
        Value::Null
    );
    let normalized = fixture.project.join("draft-root");
    assert!(service.workspace_is_open(&normalized));
    service
        .acquire_workspace_lease(&normalized, "terminal:one")
        .expect("first terminal lease");
    service
        .acquire_workspace_lease(&normalized, "terminal:two")
        .expect("second terminal lease");
    service
        .dispatch_blocking(
            CHANNELS.set_workspace_open,
            json!({"root":lexical.to_string_lossy(),"open":false}),
        )
        .expect("release draft");
    assert!(service.workspace_is_open(&normalized));
    service
        .release_workspace_lease(&normalized, "terminal:one")
        .expect("release only one terminal");
    assert!(service.workspace_is_open(&normalized));
    service
        .release_workspace_lease(&normalized, "terminal:two")
        .expect("release final terminal");
    assert!(!service.workspace_is_open(&normalized));

    let invalid = service
        .dispatch_blocking(
            CHANNELS.set_workspace_open,
            json!({"root":fixture.project.to_string_lossy(),"open":"yes"}),
        )
        .expect_err("invalid workspace state");
    assert_eq!(
        invalid.message,
        "Could not protect that draft.\nThe editor sent invalid workspace state; no worktree was changed."
    );
    assert_eq!(COVERED_OPERATIONS.len(), 29);
    assert_eq!(
        service
            .dispatch_blocking("iface:listSessions", Value::Null)
            .expect_err("another slice")
            .code,
        "operation-not-owned"
    );
}

#[test]
fn checkpoint_create_list_and_restore_match_review_wire_shapes() {
    let fixture = Fixture::new();
    fs::write(fixture.project.join("note.txt"), "checkpoint\n").expect("checkpoint source");
    let service = fixture.service(DesktopServiceReviewPorts {
        git_runner: Some(Arc::new(ScriptedGit::for_untracked("note.txt"))),
        check_runner: None,
        checkpoint_hooks: None,
    });
    service
        .dispatch_blocking(CHANNELS.get_default_directory, Value::Null)
        .expect("grant root");

    let created = service
        .dispatch_blocking(
            CHANNELS.create_review_checkpoint,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("create checkpoint");
    assert_eq!(created["kind"], "review");
    assert_eq!(created["fileCount"], 1);
    assert_eq!(created["files"], json!([{"path":"note.txt","bytes":11}]));
    assert!(created["files"][0].get("blob").is_none());
    assert!(created["files"][0].get("sha256").is_none());

    let listed = service
        .dispatch_blocking(
            CHANNELS.list_review_checkpoints,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("list checkpoints");
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert_eq!(listed[0]["id"], created["id"]);
    assert_eq!(listed[0]["fileCount"], 1);

    fs::write(fixture.project.join("note.txt"), "changed\n").expect("changed source");
    let restored = service
        .dispatch_blocking(
            CHANNELS.restore_review_checkpoint,
            json!({
                "root":fixture.project.to_string_lossy(),
                "id":created["id"],
                "paths":["../escape.txt","unknown.txt","note.txt"]
            }),
        )
        .expect("restore checkpoint");
    assert_eq!(restored["checkpointId"], created["id"]);
    assert_eq!(restored["restored"], json!(["note.txt"]));
    assert_eq!(
        restored["skipped"],
        json!([
            {"path":"../escape.txt","reason":"unsafe-path"},
            {"path":"unknown.txt","reason":"not-in-checkpoint"}
        ])
    );
    assert_eq!(restored["conflicts"], json!([]));
    assert!(restored["recoveryCheckpointId"].is_string());
    assert_eq!(
        fs::read_to_string(fixture.project.join("note.txt")).expect("restored source"),
        "checkpoint\n"
    );
}

#[cfg(unix)]
#[test]
fn checkpoint_restore_refuses_a_last_moment_symlink_swap() {
    let fixture = Fixture::new();
    let target = fixture.project.join("note.txt");
    let parked = fixture.project.join("note.parked");
    let outside = fixture.home.join("outside.txt");
    fs::write(&target, "checkpoint\n").expect("checkpoint source");
    fs::write(&outside, "outside\n").expect("outside source");
    let hook = Arc::new(SwapBeforeRestore {
        target: target.clone(),
        parked: parked.clone(),
        outside: outside.clone(),
        fired: AtomicBool::new(false),
    });
    let service = fixture.service(DesktopServiceReviewPorts {
        git_runner: Some(Arc::new(ScriptedGit::for_untracked("note.txt"))),
        check_runner: None,
        checkpoint_hooks: Some(hook),
    });
    service
        .dispatch_blocking(CHANNELS.get_default_directory, Value::Null)
        .expect("grant root");
    let created = service
        .dispatch_blocking(
            CHANNELS.create_review_checkpoint,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect("create checkpoint");
    fs::write(&target, "current\n").expect("current source");

    let restored = service
        .dispatch_blocking(
            CHANNELS.restore_review_checkpoint,
            json!({
                "root":fixture.project.to_string_lossy(),
                "id":created["id"],
                "paths":["note.txt"]
            }),
        )
        .expect("structured race result");

    assert_eq!(restored["restored"], json!([]));
    assert_eq!(
        restored["conflicts"],
        json!([{"path":"note.txt","reason":"changed-during-restore"}])
    );
    assert!(restored["recoveryCheckpointId"].is_string());
    assert_eq!(
        fs::read_to_string(&outside).expect("outside intact"),
        "outside\n"
    );
    assert_eq!(
        fs::read_to_string(&parked).expect("current parked"),
        "current\n"
    );
}

#[test]
fn checkpoint_storage_absence_is_the_original_explicit_refusal() {
    let fixture = Fixture::new();
    let mut config = fixture.config();
    config.review_checkpoint_store_path = None;
    let service = DesktopService::open(config, DesktopServiceReviewPorts::default())
        .expect("service without optional checkpoint store");
    service
        .dispatch_blocking(CHANNELS.get_default_directory, Value::Null)
        .expect("grant root");
    let error = service
        .dispatch_blocking(
            CHANNELS.list_review_checkpoints,
            json!({"root":fixture.project.to_string_lossy()}),
        )
        .expect_err("explicit unavailable checkpoint store");
    assert_eq!(
        error.message,
        "Review checkpoints are unavailable.\nThis Interface build has no private checkpoint storage configured."
    );
}

#[cfg(unix)]
fn assert_unavailable_appearance_does_not_write(service: &DesktopService, appearance: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let before = fs::symlink_metadata(appearance).expect("appearance before set");
    let get = service
        .dispatch_blocking(CHANNELS.get_appearance, json!({"systemDark": true}))
        .expect("get appearance while degraded");
    assert_eq!(get, json!({"preference":"system","resolved":"dark"}));

    let error = service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"preference":"dark","systemDark":false}),
        )
        .expect_err("set appearance while unavailable");
    assert_eq!(error.code, "appearance-unavailable");
    assert_eq!(
        error.message,
        "The appearance file is unavailable; the theme was not saved."
    );
    assert!(!error.retryable);

    let after = fs::symlink_metadata(appearance).expect("appearance after set");
    assert_eq!(before.file_type().is_dir(), after.file_type().is_dir());
    assert_eq!(
        before.file_type().is_symlink(),
        after.file_type().is_symlink()
    );
    assert_eq!(before.file_type().is_file(), after.file_type().is_file());
    assert_eq!(before.len(), after.len());
    assert_eq!(
        before.permissions().mode() & 0o777,
        after.permissions().mode() & 0o777
    );
    assert_eq!(
        before.modified().expect("mtime before"),
        after.modified().expect("mtime after")
    );
}

#[cfg(unix)]
#[test]
fn unreadable_appearance_file_does_not_fail_host_open() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let appearance = fixture.config().appearance_file;
    fs::create_dir_all(appearance.parent().expect("parent")).expect("state");
    fs::set_permissions(
        appearance.parent().expect("parent"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("state mode");
    fs::write(&appearance, br#"{"theme":"dark"}"#).expect("write appearance");
    fs::set_permissions(&appearance, fs::Permissions::from_mode(0o000)).expect("unreadable");

    let service = DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default())
        .expect("open with unreadable appearance");
    assert_unavailable_appearance_does_not_write(&service, &appearance);
    assert_eq!(
        fs::symlink_metadata(&appearance)
            .expect("still present")
            .permissions()
            .mode()
            & 0o777,
        0
    );
    fs::set_permissions(&appearance, fs::Permissions::from_mode(0o600)).expect("restore");
}

#[cfg(unix)]
#[test]
fn appearance_directory_does_not_fail_host_open() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let appearance = fixture.config().appearance_file;
    fs::create_dir_all(&appearance).expect("appearance as directory");
    fs::set_permissions(&appearance, fs::Permissions::from_mode(0o700)).expect("dir mode");

    let service = DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default())
        .expect("open with appearance directory");
    assert_unavailable_appearance_does_not_write(&service, &appearance);
    assert!(
        fs::symlink_metadata(&appearance)
            .expect("still a directory")
            .file_type()
            .is_dir()
    );
}

#[cfg(unix)]
#[test]
fn appearance_symlink_does_not_fail_host_open() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let fixture = Fixture::new();
    let appearance = fixture.config().appearance_file;
    fs::create_dir_all(&fixture.state).expect("state");
    fs::set_permissions(&fixture.state, fs::Permissions::from_mode(0o700)).expect("state mode");
    let target = fixture.state.join("real-appearance.json");
    let original = br#"{"theme":"dark"}"#;
    fs::write(&target, original).expect("link target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("target mode");
    symlink(&target, &appearance).expect("appearance symlink");

    let service = DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default())
        .expect("open with appearance symlink");
    assert_unavailable_appearance_does_not_write(&service, &appearance);
    assert!(
        fs::symlink_metadata(&appearance)
            .expect("still a symlink")
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(&target).expect("target bytes"), original);
}

#[test]
fn appearance_surface_v1_dispatch_preserves_legacy_calls_and_persists_separately() {
    let fixture = Fixture::new();
    let service =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let get = |version| json!({"systemDark":true,"surfaceVersion":version});
    let before = service
        .dispatch_blocking(CHANNELS.get_appearance, get(1))
        .unwrap();
    assert_eq!(
        before["translucency"],
        json!({"version":1,"terminal":0,"sidebar":0})
    );
    let value = json!({"version":1,"terminal":75,"sidebar":30});
    let change = json!({"systemDark":true,"surfaceVersion":1,"preference":value});
    let result = service
        .dispatch_blocking(CHANNELS.set_appearance, change)
        .unwrap();
    assert_eq!(result["translucency"], value);
    assert_eq!(result["preference"], "system");
    service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"systemDark":true,"preference":"light"}),
        )
        .unwrap();
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, json!({"systemDark":true}))
            .unwrap(),
        json!({"preference":"light","resolved":"light"})
    );
    assert!(
        service
            .dispatch_blocking(
                CHANNELS.set_appearance,
                json!({"systemDark":true,"surfaceVersion":7,"preference":"dark"})
            )
            .is_err()
    );
    assert!(
        service
            .dispatch_blocking(
                CHANNELS.set_appearance,
                json!({"systemDark":true,"preference":value})
            )
            .is_err()
    );
    assert!(service.dispatch_blocking(CHANNELS.set_appearance,
        json!({"systemDark":true,"surfaceVersion":1,"preference":{"version":1,"terminal":101,"sidebar":30}})).is_err());
    drop(service);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let restored = reopened
        .dispatch_blocking(CHANNELS.get_appearance, get(1))
        .unwrap();
    assert_eq!(restored["translucency"], value);
    assert_eq!(restored["preference"], "light");
}

#[test]
fn appearance_surface_v2_persists_pane_style_and_preserves_legacy_responses() {
    let fixture = Fixture::new();
    let service =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let get = |version| json!({"systemDark":true,"surfaceVersion":version});
    let value = json!({"version":1,"paneStyle":"clean"});
    let default = service
        .dispatch_blocking(CHANNELS.get_appearance, get(2))
        .unwrap();
    assert_eq!(
        default["panePresentation"],
        json!({"version":1,"paneStyle":"terminal"})
    );
    assert!(
        service
            .dispatch_blocking(
                CHANNELS.set_appearance,
                json!({"systemDark":true,"surfaceVersion":1,"preference":value})
            )
            .is_err()
    );
    let changed = service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"systemDark":true,"surfaceVersion":2,"preference":value}),
        )
        .unwrap();
    assert_eq!(changed["panePresentation"], value);
    assert_eq!(changed["translucency"], default["translucency"]);
    assert!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, get(1))
            .unwrap()
            .get("panePresentation")
            .is_none()
    );
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, json!({"systemDark":true}))
            .unwrap(),
        json!({"preference":"system","resolved":"dark"})
    );
    drop(service);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    assert_eq!(
        reopened
            .dispatch_blocking(CHANNELS.get_appearance, get(2))
            .unwrap()["panePresentation"],
        value
    );
}

#[test]
fn workspace_presentation_v3_restores_pins_and_reading_size_without_changing_legacy_responses() {
    let fixture = Fixture::new();
    let service =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let get = |version| json!({"systemDark":true,"surfaceVersion":version});
    let legacy = service
        .dispatch_blocking(CHANNELS.get_appearance, get(2))
        .unwrap();
    let default = service
        .dispatch_blocking(CHANNELS.get_appearance, get(3))
        .unwrap();
    assert_eq!(
        default["workspacePresentation"],
        json!({"version":1,"terminalFontSize":12,"pinnedWorkspaces":[]})
    );
    let value =
        json!({"version":1,"terminalFontSize":24,"pinnedWorkspaces":["workspace-a","workspace-b"]});
    for version in [1, 2, 6] {
        assert!(
            service
                .dispatch_blocking(
                    CHANNELS.set_appearance,
                    json!({"systemDark":true,"surfaceVersion":version,"preference":value})
                )
                .is_err()
        );
    }
    service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"systemDark":true,"surfaceVersion":3,"preference":value}),
        )
        .unwrap();
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, get(2))
            .unwrap(),
        legacy
    );
    drop(service);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    assert_eq!(
        reopened
            .dispatch_blocking(CHANNELS.get_appearance, get(3))
            .unwrap()["workspacePresentation"],
        value
    );
}

#[test]
fn workspace_presentation_v6_keeps_half_sizes_and_older_surfaces_read_whole_ones() {
    let fixture = Fixture::new();
    let service =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let get = |version| json!({"systemDark":true,"surfaceVersion":version});
    let set = |version, value: &Value| json!({"systemDark":true,"surfaceVersion":version,"preference":value});
    // Nothing saved: the owner's 11.5px, which v3-v5 read as the next whole size.
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, get(6))
            .unwrap()["workspacePresentation"],
        json!({"version":2,"terminalFontSize":11.5,"pinnedWorkspaces":[]})
    );
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, get(5))
            .unwrap()["workspacePresentation"],
        json!({"version":1,"terminalFontSize":12,"pinnedWorkspaces":[]})
    );
    let half = json!({"version":2,"terminalFontSize":10.5,"pinnedWorkspaces":["workspace-a"]});
    // v2 is v6's alone, and v6 no longer takes v1.
    for version in 1..=5 {
        assert!(
            service
                .dispatch_blocking(CHANNELS.set_appearance, set(version, &half))
                .is_err(),
            "{version}"
        );
    }
    let whole = json!({"version":1,"terminalFontSize":24,"pinnedWorkspaces":[]});
    assert!(
        service
            .dispatch_blocking(CHANNELS.set_appearance, set(6, &whole))
            .is_err()
    );
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.set_appearance, set(6, &half))
            .unwrap()["workspacePresentation"],
        half
    );
    drop(service);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    assert_eq!(
        reopened
            .dispatch_blocking(CHANNELS.get_appearance, get(6))
            .unwrap()["workspacePresentation"],
        half
    );
    assert_eq!(
        reopened
            .dispatch_blocking(CHANNELS.get_appearance, get(4))
            .unwrap()["workspacePresentation"],
        json!({"version":1,"terminalFontSize":12,"pinnedWorkspaces":["workspace-a"]})
    );
    // An older surface's save is kept, and v6 reads it as v2.
    reopened
        .dispatch_blocking(CHANNELS.set_appearance, set(3, &whole))
        .unwrap();
    assert_eq!(
        reopened
            .dispatch_blocking(CHANNELS.get_appearance, get(6))
            .unwrap()["workspacePresentation"],
        json!({"version":2,"terminalFontSize":24,"pinnedWorkspaces":[]})
    );
}

#[test]
fn project_pins_v4_survive_reopen_and_leave_legacy_settings_unchanged() {
    let fixture = Fixture::new();
    let service =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let get = |version| json!({"systemDark":true,"surfaceVersion":version});
    let legacy: Vec<_> = (1..=3)
        .map(|version| {
            service
                .dispatch_blocking(CHANNELS.get_appearance, get(version))
                .unwrap()
        })
        .collect();
    let value =
        json!({"version":1,"pinnedProjects":["/projects/first/App","/projects/second/App"]});
    for version in 1..=3 {
        assert!(
            service
                .dispatch_blocking(
                    CHANNELS.set_appearance,
                    json!({"systemDark":true,"surfaceVersion":version,"preference":value})
                )
                .is_err()
        );
    }
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, get(4))
            .unwrap()["projectPresentation"],
        json!({"version":1,"pinnedProjects":[]})
    );
    service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"systemDark":true,"surfaceVersion":4,"preference":value}),
        )
        .unwrap();
    for version in 1..=3 {
        assert_eq!(
            service
                .dispatch_blocking(CHANNELS.get_appearance, get(version))
                .unwrap(),
            legacy[version - 1]
        );
    }
    let workspace = json!({"version":1,"terminalFontSize":24,"pinnedWorkspaces":["workspace-a"]});
    service
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"systemDark":true,"surfaceVersion":4,"preference":workspace}),
        )
        .unwrap();
    drop(service);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let restored = reopened
        .dispatch_blocking(CHANNELS.get_appearance, get(4))
        .unwrap();
    assert_eq!(restored["projectPresentation"], value);
    assert_eq!(restored["workspacePresentation"], workspace);
    let empty = json!({"version":1,"pinnedProjects":[]});
    reopened
        .dispatch_blocking(
            CHANNELS.set_appearance,
            json!({"systemDark":true,"surfaceVersion":4,"preference":empty}),
        )
        .unwrap();
    drop(reopened);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let restored = reopened
        .dispatch_blocking(CHANNELS.get_appearance, get(4))
        .unwrap();
    assert_eq!(restored["projectPresentation"], empty);
    assert_eq!(restored["workspacePresentation"], workspace);
}

/// A service whose home holds two theme files: Moss (dark) and Paper (light).
fn theme_file_service() -> (Fixture, std::path::PathBuf, DesktopService) {
    let fixture = Fixture::new();
    let themes = fixture.home.join(".config/interface/themes");
    fs::create_dir_all(&themes).unwrap();
    fs::write(
        themes.join("Moss.css"),
        ":root { --theme-kind: dark; --theme-background: #0f1310; --theme-accent: #9fd4a8; }",
    )
    .unwrap();
    fs::write(themes.join("Paper.css"), ":root { --editor-bg: #f3efe6; }").unwrap();
    let service =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    (fixture, themes, service)
}

fn theme_get(version: u8, dark: bool) -> Value {
    json!({"systemDark":dark,"surfaceVersion":version})
}

fn theme_set(version: u8, value: &Value) -> Value {
    json!({"systemDark":true,"surfaceVersion":version,"preference":value})
}

#[test]
fn theme_files_v5_list_and_paint() {
    let (_fixture, _themes, service) = theme_file_service();
    let initial = service
        .dispatch_blocking(CHANNELS.get_appearance, theme_get(5, true))
        .unwrap();
    assert_eq!(initial["preference"], "system");
    assert_eq!(initial["resolved"], "dark");
    assert_eq!(
        initial["systemThemes"],
        json!({"version":1,"light":"light","dark":"dark"})
    );
    assert!(initial.get("themeCss").is_none());
    let listed = initial["customThemes"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["id"], "custom:Moss");
    assert_eq!(listed[0]["kind"], "dark");
    assert_eq!(listed[0]["background"], "#0f1310");
    assert_eq!(listed[0]["preview"]["--theme-accent"], "#9fd4a8");
    assert_eq!(listed[1]["id"], "custom:Paper");
    assert_eq!(listed[1]["kind"], "light");

    // Theme files need surface 5.
    for version in [1, 4] {
        assert!(
            service
                .dispatch_blocking(
                    CHANNELS.set_appearance,
                    theme_set(version, &json!("custom:Moss"))
                )
                .is_err()
        );
    }
    let picked = service
        .dispatch_blocking(CHANNELS.set_appearance, theme_set(5, &json!("custom:Moss")))
        .unwrap();
    assert_eq!(picked["preference"], "custom:Moss");
    assert_eq!(picked["resolved"], "custom:Moss");
    assert!(
        picked["themeCss"]
            .as_str()
            .unwrap()
            .contains("--theme-accent")
    );
    // Older surfaces see Follow macOS over the built-in pair.
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, json!({"systemDark":false}))
            .unwrap(),
        json!({"preference":"system","resolved":"light"})
    );
    assert!(
        service
            .dispatch_blocking(
                CHANNELS.set_appearance,
                theme_set(5, &json!("custom:../escape"))
            )
            .is_err()
    );
}

#[test]
fn theme_files_v5_pair_with_macos_and_survive_a_missing_file() {
    let (fixture, themes, service) = theme_file_service();
    let pairing = json!({"version":1,"light":"custom:Paper","dark":"custom:Moss"});
    assert!(
        service
            .dispatch_blocking(CHANNELS.set_appearance, theme_set(4, &pairing))
            .is_err()
    );
    service
        .dispatch_blocking(CHANNELS.set_appearance, theme_set(5, &pairing))
        .unwrap();
    service
        .dispatch_blocking(CHANNELS.set_appearance, theme_set(5, &json!("system")))
        .unwrap();
    let light = service
        .dispatch_blocking(CHANNELS.get_appearance, theme_get(5, false))
        .unwrap();
    assert_eq!(light["resolved"], "custom:Paper");
    assert!(light["themeCss"].as_str().unwrap().contains("#f3efe6"));
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.get_appearance, theme_get(5, true))
            .unwrap()["resolved"],
        "custom:Moss"
    );

    // A deleted file keeps the choice and paints the built-in for the slot.
    service
        .dispatch_blocking(CHANNELS.set_appearance, theme_set(5, &json!("custom:Moss")))
        .unwrap();
    fs::remove_file(themes.join("Moss.css")).unwrap();
    let gone = service
        .dispatch_blocking(CHANNELS.get_appearance, theme_get(5, true))
        .unwrap();
    assert_eq!(gone["preference"], "custom:Moss");
    assert_eq!(gone["resolved"], "dark");
    assert!(gone.get("themeCss").is_none());
    assert_eq!(gone["customThemes"].as_array().unwrap().len(), 1);

    drop(service);
    let reopened =
        DesktopService::open(fixture.config(), DesktopServiceReviewPorts::default()).unwrap();
    let restored = reopened
        .dispatch_blocking(CHANNELS.get_appearance, theme_get(5, true))
        .unwrap();
    assert_eq!(restored["preference"], "custom:Moss");
    assert_eq!(restored["systemThemes"], pairing);
}

#[test]
fn harness_results_bind_full_candidate_bytes_and_refuse_stale_checks() {
    let fixture = Fixture::new();
    let project = fs::canonicalize(&fixture.project).unwrap();
    let git = |args: &[&str]| {
        assert!(
            std::process::Command::new("git")
                .args(args)
                .current_dir(&project)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    git(&["init", "-q"]);
    fs::write(project.join("app.txt"), "baseline\n").unwrap();
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"test":"node -e \"console.log('checked')\""}}"#,
    )
    .unwrap();
    git(&["add", "app.txt", "package.json"]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "commit",
        "-qm",
        "baseline",
    ]);
    fs::write(project.join("app.txt"), "candidate\n").unwrap();
    let checks = Arc::new(FakeChecks::default());
    let service = fixture.service(DesktopServiceReviewPorts {
        check_runner: Some(checks.clone()),
        ..Default::default()
    });
    let roots = [project.clone()];
    let captured = service.harness_command(json!({"kind":"captureResult","id":"r","objective":"Inspect fixture","root":project,"target":{"kind":"session","key":"codex#fixture"}}), &roots).unwrap();
    assert!(
        checks.invocations.lock().unwrap().is_empty(),
        "inspection ran a check"
    );
    assert_eq!(captured["result"]["changedPaths"], json!(["app.txt"]));
    let checked = service
        .harness_command(
            json!({"kind":"runChecks","id":"r","selected":["test"]}),
            &roots,
        )
        .unwrap();
    assert_eq!(
        checked["result"]["checkCandidate"],
        captured["result"]["candidate"]
    );
    assert_eq!(checked["result"]["checks"][0]["stdout"], "green\n");
    assert_eq!(
        checked["state"]["tasks"].as_array().unwrap().len(),
        2,
        "check evidence replaced the original"
    );
    fs::write(project.join("app.txt"), "concurrent edit\n").unwrap();
    let inspected = service
        .harness_command(json!({"kind":"inspectResult","id":"r"}), &roots)
        .unwrap();
    assert_eq!(inspected["stale"], true);
    assert!(
        service
            .harness_command(
                json!({"kind":"runChecks","id":"r","selected":["test"]}),
                &roots
            )
            .is_err()
    );
    assert_eq!(
        checks.invocations.lock().unwrap().len(),
        1,
        "stale candidate ran a check"
    );
    assert_eq!(
        fs::read_to_string(project.join("app.txt")).unwrap(),
        "concurrent edit\n"
    );
}

#[test]
fn harness_context_suggests_the_first_twenty_unique_readable_entrypoints_in_order() {
    let fixture = Fixture::new();
    let project = fs::canonicalize(&fixture.project).unwrap();
    fs::create_dir(project.join("docs")).unwrap();
    let mut first = Vec::new();
    let mut second = Vec::new();
    for index in 0..30 {
        let name = format!("file{index:02}.rs");
        fs::write(project.join(&name), "fn f() {}\n").unwrap();
        if index < 12 {
            first.push(name);
        } else {
            second.push(name);
        }
    }
    // Duplicates, a credential path and a missing file never take a slot.
    first.insert(3, "file00.rs".to_owned());
    first.insert(5, ".env".to_owned());
    fs::write(project.join(".env"), "SECRET=1\n").unwrap();
    second.insert(0, "missing.rs".to_owned());
    let map = json!({"routes":[
        {"id":"a","entrypoints":first},
        {"id":"b","entrypoints":second}
    ]});
    fs::write(project.join("docs/agent-map.json"), map.to_string()).unwrap();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let result = service
        .harness_command(
            json!({"kind":"context","root":project,"query":""}),
            &[project.clone()],
        )
        .unwrap();
    let expected = (0..20)
        .map(|index| format!("file{index:02}.rs"))
        .collect::<Vec<_>>();
    assert_eq!(result["context"]["suggestions"], json!(expected));
}

#[test]
fn harness_context_rejects_traversal_and_never_grants_a_new_root() {
    let fixture = Fixture::new();
    let project = fs::canonicalize(&fixture.project).unwrap();
    fs::create_dir(project.join("docs")).unwrap();
    fs::write(project.join("safe.rs"), "fn safe() {}\n").unwrap();
    fs::write(project.join("docs/agent-map.json"), r#"{"routes":[{"id":"safe","keywords":["task"],"entrypoints":["../outside.txt","safe.rs","/tmp/elsewhere"]}]}"#).unwrap();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let result = service
        .harness_command(
            json!({"kind":"context","root":project,"query":"task"}),
            &[project.clone()],
        )
        .unwrap();
    assert_eq!(result["context"]["suggestions"], json!(["safe.rs"]));
    assert!(
        service
            .harness_command(
                json!({"kind":"context","root":fixture.home,"query":""}),
                &[project]
            )
            .is_err()
    );
    assert!(
        service
            .harness_command(json!({"kind":"catalog","executeShell":"anything"}), &[])
            .is_err()
    );
}

fn agent_command(service: &DesktopService, payload: Value) -> Value {
    service
        .dispatch_blocking(CHANNELS.agent_control, payload)
        .expect("agent command")
}

fn save_research_agent(service: &DesktopService) -> Value {
    agent_command(
        service,
        json!({"kind":"saveAgent","expectedRevision":0,"profile":{
            "id":"research","name":"Research","engine":"claude","model":"","effort":"",
            "instructions":"Cite primary sources.","color":"sky","shape":"star"
        }}),
    )
}

fn pending_approval(session_key: &str, approval_id: &str) -> AgentPendingApproval {
    AgentPendingApproval {
        agent_id: "research".into(),
        session_key: session_key.into(),
        approval_id: approval_id.into(),
        tool: "Bash".into(),
        summary: "git push".into(),
        reason: "risky:git-push".into(),
        asked_at_ms: 1,
    }
}

#[test]
fn agent_read_files_without_a_home_reports_it_missing() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let saved = save_research_agent(&service);
    assert_eq!(saved["version"], 2);
    assert_eq!(saved["statuses"], json!({"research":"idle"}));
    assert_eq!(saved["pendingApprovals"], json!([]));
    let response = agent_command(
        &service,
        json!({"kind":"readAgentFiles","agentId":"research"}),
    );
    assert_eq!(
        response["files"],
        json!({"home":null,"exists":false,"markerOk":false,"instructions":null,"instructionsConflict":false,"memory":null})
    );
    let refused = service
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"startAgentThread","agentId":"research","text":"hi","requestId":"r1"}),
        )
        .unwrap_err();
    assert_eq!(refused.code, "invalid-payload");
    assert_eq!(
        refused.message,
        "Starting a conversation is handled by the host."
    );
    let refused = service
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"startAgentTask","agentId":"research","text":"hi","cwd":"/fixture","requestId":"r2"}),
        )
        .unwrap_err();
    assert_eq!(refused.code, "invalid-payload");
    assert_eq!(refused.message, "Starting a task is handled by the host.");
}

#[test]
fn agent_reveal_creates_the_home_under_the_agents_root_and_memory_is_compare_and_save() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let root = fixture.home.join("Interface Dev Agents");
    service.set_agents_root(root.clone());
    save_research_agent(&service);
    let revealed = agent_command(
        &service,
        json!({"kind":"revealAgentHome","agentId":"research"}),
    );
    let home = root.join("Research");
    assert!(home.join(".interface-agent.json").is_file());
    assert!(home.join("CLAUDE.md").is_file() && home.join("AGENTS.md").is_file());
    assert_eq!(revealed["agents"][0]["home"], home.to_str().unwrap());
    assert_eq!(
        revealed["agents"][0]["instructionsFileSha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert!(!fixture.home.join("Interface Agents").exists());

    let files = agent_command(
        &service,
        json!({"kind":"readAgentFiles","agentId":"research"}),
    )["files"]
        .clone();
    assert_eq!(files["exists"], true);
    assert_eq!(files["markerOk"], true);
    assert_eq!(files["instructionsConflict"], false);
    assert!(
        files["instructions"]["text"]
            .as_str()
            .unwrap()
            .contains("Cite primary sources.")
    );
    let memory_sha = files["memory"]["sha256"].as_str().unwrap().to_owned();

    let conflict = service
        .dispatch_blocking(
            CHANNELS.agent_control,
            json!({"kind":"saveAgentMemory","agentId":"research","expectedSha256":"0".repeat(64),"text":"lost"}),
        )
        .unwrap_err();
    assert_eq!(conflict.code, "agent-file-conflict");
    let saved = agent_command(
        &service,
        json!({"kind":"saveAgentMemory","agentId":"research","expectedSha256":memory_sha,"text":"# Memory\nkept"}),
    );
    assert_eq!(saved["files"]["memory"]["text"], "# Memory\nkept");
    assert_eq!(
        fs::read_to_string(home.join("MEMORY.md")).unwrap(),
        "# Memory\nkept"
    );

    // Editing the job regenerates both files while Interface still owns them.
    let revision = saved["revision"].as_u64().unwrap();
    agent_command(
        &service,
        json!({"kind":"saveAgent","expectedRevision":revision,"profile":{
            "id":"research","name":"Research","engine":"claude","model":"","effort":"",
            "instructions":"Cite secondary sources too.","color":"sky","shape":"star"
        }}),
    );
    assert!(
        fs::read_to_string(home.join("AGENTS.md"))
            .unwrap()
            .contains("Cite secondary sources too.")
    );
    let files = agent_command(
        &service,
        json!({"kind":"readAgentFiles","agentId":"research"}),
    );
    assert_eq!(files["files"]["instructionsConflict"], false);
}

#[test]
fn pending_agent_approvals_flow_into_statuses_and_are_taken_exactly_once() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    save_research_agent(&service);
    service.add_pending_agent_approval(pending_approval("claude#t", "a1"));
    service.add_pending_agent_approval(pending_approval("claude#t", "a2"));
    let response = service.agent_control_response().unwrap();
    assert_eq!(response["statuses"]["research"], "needsAttention");
    assert_eq!(response["pendingApprovals"].as_array().unwrap().len(), 2);

    assert_eq!(
        service.take_pending_agent_approval("claude#t", "missing"),
        None
    );
    assert_eq!(service.take_pending_agent_approval("other#t", "a1"), None);
    let taken = service
        .take_pending_agent_approval("claude#t", "a1")
        .unwrap();
    assert_eq!(taken.approval_id, "a1");
    assert_eq!(service.take_pending_agent_approval("claude#t", "a1"), None);
    let response = service.agent_control_response().unwrap();
    assert_eq!(response["pendingApprovals"][0]["approvalId"], "a2");
    assert_eq!(response["statuses"]["research"], "needsAttention");

    service.clear_pending_agent_approvals("claude#t");
    let response = service.agent_control_response().unwrap();
    assert_eq!(response["statuses"]["research"], "idle");

    for n in 0..70 {
        service.add_pending_agent_approval(pending_approval("claude#t", &format!("n{n}")));
    }
    let response = service.agent_control_response().unwrap();
    let pending = response["pendingApprovals"].as_array().unwrap();
    assert_eq!(pending.len(), 70);
    assert_eq!(pending[0]["approvalId"], "n0");
}

/// A service whose home lives under `/tmp`, outside the system roots every temp dir in
/// `/var/folders` falls under, so each grant rule is exercised on its own.
fn tmp_service() -> (TempDir, PathBuf, PathBuf, DesktopService) {
    let temp = tempfile::tempdir_in("/tmp").expect("temp outside refused roots");
    let home = temp.path().join("home");
    let state = temp.path().join("state");
    fs::create_dir_all(home.join("Desktop/Projects")).unwrap();
    let work = temp.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let service = DesktopService::open(
        DesktopServiceConfig {
            appearance_file: state.join("appearance.json"),
            acp_profiles_file: state.join("acp-profiles.json"),
            terminal_workspaces_file: state.join("terminal-workspaces.json"),
            home_directory: home.clone(),
            customization_user_root: None,
            review_checkpoint_store_path: None,
        },
        DesktopServiceReviewPorts::default(),
    )
    .expect("open service");
    service.set_agents_root(home.join("Interface Agents"));
    (temp, home, work, service)
}

#[test]
fn grantable_roots_refuse_ancestors_and_the_agents_root() {
    let (temp, home, work, service) = tmp_service();
    let agents = home.join("Interface Agents");
    assert!(!service.is_grantable_root(&agents), "missing agents root");
    fs::create_dir_all(agents.join("Research")).unwrap();
    for refused in [
        Path::new("/"),
        Path::new("/private"),
        Path::new("/tmp"),
        temp.path(),
        home.as_path(),
        agents.as_path(),
        agents.join("Research").as_path(),
        work.join("missing").as_path(),
    ] {
        assert!(!service.is_grantable_root(refused), "{}", refused.display());
    }
    assert!(service.is_grantable_root(&work));
    assert!(service.is_grantable_root(&home.join("Desktop/Projects")));

    // An agents root outside the home directory is refused with its ancestors too.
    let elsewhere = work.join("Interface Dev Agents");
    service.set_agents_root(elsewhere.clone());
    assert!(!service.is_grantable_root(&work));
    assert!(service.is_grantable_root(&home.join("Desktop/Projects")));
}

#[test]
fn routine_run_in_the_home_directory_gets_no_root_for_it() {
    let (_temp, home, work, service) = tmp_service();
    save_research_agent(&service);
    for (routine, cwd) in [("home", home.as_path()), ("work", work.as_path())] {
        let read = service.agent_control_response().unwrap();
        agent_command(
            &service,
            json!({"kind":"saveRoutine","expectedRevision":read["revision"],"routine":{
                "id":routine,"agentId":"research","name":routine,"prompt":"Summarize.",
                "cwd":cwd.to_str().unwrap(),"enabled":false,"startAtMs":1000,
                "intervalMinutes":null,"nextRunAtMs":null
            }}),
        );
    }
    let run_in = |routine: &str, key: &str| {
        let read = service.agent_control_response().unwrap();
        agent_command(
            &service,
            json!({"kind":"runNow","expectedRevision":read["revision"],"routineId":routine,"requestId":routine}),
        );
        let run = service.claim_agent_run().unwrap().unwrap();
        service.bind_agent_run(&run.id, key).unwrap();
        // Finish it so the agent's next routine can be admitted; the binding stays readable.
        service.fail_agent_run(&run.id, "done", true).unwrap();
        service.agent_binding(key).unwrap()
    };
    let in_home = run_in("home", "codex#home");
    assert_eq!(in_home.cwd, home);
    let agent_home = PathBuf::from(service.prepare_agent_thread("research").unwrap().home);
    assert_eq!(in_home.roots, vec![agent_home.clone()]);
    assert!(
        !in_home.roots.contains(&home),
        "The owner's entire home is never granted"
    );
    let in_work = run_in("work", "codex#work");
    assert_eq!(in_work.roots, vec![agent_home, work.clone()]);
}

#[test]
fn save_agent_folders_refuses_root_system_and_home_directories() {
    let (temp, home, work, service) = tmp_service();
    save_research_agent(&service);
    let revealed = agent_command(
        &service,
        json!({"kind":"revealAgentHome","agentId":"research"}),
    );
    let revision = revealed["revision"].as_u64().unwrap();
    let agent_home = revealed["agents"][0]["home"].as_str().unwrap().to_owned();
    for folder in [
        "/".to_owned(),
        home.to_str().unwrap().to_owned(),
        temp.path().to_str().unwrap().to_owned(),
        home.join("Interface Agents").to_str().unwrap().to_owned(),
        "/usr/bin".to_owned(),
        "/private/var".to_owned(),
        agent_home.clone(),
        work.join("missing").to_str().unwrap().to_owned(),
        "relative".to_owned(),
    ] {
        let error = service
            .dispatch_blocking(
                CHANNELS.agent_control,
                json!({"kind":"saveAgentFolders","expectedRevision":revision,"agentId":"research","folders":[folder]}),
            )
            .unwrap_err();
        assert_eq!(error.code, "agent-state-refused", "{folder}");
    }
    let unchanged = service.agent_control_response().unwrap();
    assert_eq!(unchanged["revision"].as_u64().unwrap(), revision);

    let granted = agent_command(
        &service,
        json!({"kind":"saveAgentFolders","expectedRevision":revision,"agentId":"research","folders":[work.to_str().unwrap()]}),
    );
    let canonical = fs::canonicalize(&work).unwrap();
    assert_eq!(
        granted["agents"][0]["folders"],
        json!([canonical.to_str().unwrap()])
    );
    assert!(
        fs::read_to_string(Path::new(&agent_home).join("CLAUDE.md"))
            .unwrap()
            .contains(&format!(
                "Other folders you may work in: {}.",
                canonical.display()
            ))
    );
}

#[test]
fn save_agent_folders_refuses_a_credential_folder() {
    let (_temp, _home, work, service) = tmp_service();
    save_research_agent(&service);
    let read = service.agent_control_response().unwrap();
    let revision = read["revision"].as_u64().unwrap();
    for name in [".ssh", "secrets", ".aws"] {
        let folder = work.join(name);
        fs::create_dir_all(&folder).unwrap();
        let error = service
            .dispatch_blocking(
                CHANNELS.agent_control,
                json!({"kind":"saveAgentFolders","expectedRevision":revision,"agentId":"research","folders":[folder.to_str().unwrap()]}),
            )
            .unwrap_err();
        assert_eq!(error.code, "agent-state-refused", "{name}");
        assert!(error.message.contains("credentials"), "{}", error.message);
    }
    let unchanged = service.agent_control_response().unwrap();
    assert_eq!(unchanged["revision"].as_u64().unwrap(), revision);
    assert_eq!(unchanged["agents"][0].get("folders"), None);
}

#[test]
fn agent_home_sessions_are_read_only_unless_live() {
    let (temp, home, work, service) = tmp_service();
    save_research_agent(&service);
    let agent_home = PathBuf::from(service.prepare_agent_thread("research").unwrap().home);
    service
        .bind_agent_thread("research", "claude#thread")
        .unwrap();
    assert!(!service.is_read_only_agent_session("claude#thread", Some(&agent_home)));
    // Any other session recorded in an agent home, even one the store never saw.
    assert!(service.is_read_only_agent_session("claude#fork", Some(&agent_home)));
    assert!(service.is_read_only_agent_session("claude#fork", Some(&agent_home.join("sub"))));
    assert!(!service.is_read_only_agent_session("claude#plain", Some(&work)));
    assert!(!service.is_read_only_agent_session("claude#plain", None));
    // `/tmp/…` and `/private/tmp/…` name the same agent home.
    let canonical_home = fs::canonicalize(&agent_home).unwrap();
    assert!(service.is_read_only_agent_session("claude#fork", Some(&canonical_home)));

    let read = service.agent_control_response().unwrap();
    agent_command(
        &service,
        json!({"kind":"newThread","expectedRevision":read["revision"],"agentId":"research"}),
    );
    assert!(service.is_read_only_agent_session("claude#thread", Some(&agent_home)));
    // A recorded past thread is read-only wherever its registry folder is.
    assert!(service.is_read_only_agent_session("claude#thread", Some(&work)));
    assert!(service.is_read_only_agent_session("claude#thread", None));
    drop(service);

    // With the store unreadable, agent-home sessions are read-only; others are not.
    fs::write(temp.path().join("state/agents.v2.json"), b"{").unwrap();
    let state = temp.path().join("state");
    let unavailable = DesktopService::open(
        DesktopServiceConfig {
            appearance_file: state.join("appearance.json"),
            acp_profiles_file: state.join("acp-profiles.json"),
            terminal_workspaces_file: state.join("terminal-workspaces.json"),
            home_directory: home.clone(),
            customization_user_root: None,
            review_checkpoint_store_path: None,
        },
        DesktopServiceReviewPorts::default(),
    )
    .unwrap();
    unavailable.set_agents_root(home.join("Interface Agents"));
    assert!(unavailable.agent_control_response().is_err());
    assert!(unavailable.is_read_only_agent_session("claude#thread", Some(&agent_home)));
    assert!(!unavailable.is_read_only_agent_session("claude#plain", Some(&work)));
}

#[test]
fn agent_threads_are_prepared_bound_and_decisions_are_logged() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let root = fixture.home.join("Interface Agents");
    service.set_agents_root(root.clone());
    assert_eq!(service.home_directory(), fixture.home.as_path());
    save_research_agent(&service);

    let launch = service.prepare_agent_thread("research").unwrap();
    assert_eq!(
        launch,
        crate::AgentThreadLaunch {
            engine: "claude".into(),
            model: String::new(),
            effort: String::new(),
            name: "Research".into(),
            home: root.join("Research").to_str().unwrap().into(),
            folders: Vec::new(),
            access: None,
        }
    );
    service
        .bind_agent_thread("research", "claude#thread")
        .unwrap();
    let binding = service.agent_binding("claude#thread").unwrap();
    assert!(binding.thread);
    assert_eq!(binding.cwd, root.join("Research"));
    assert_eq!(service.agent_binding("claude#other"), None);
    assert_eq!(
        service
            .prepare_agent_thread("research")
            .unwrap_err()
            .message,
        "This agent already has a conversation; send to it instead."
    );

    let decision = AgentDecision {
        at_ms: 5,
        session_key: "claude#thread".into(),
        approval_id: "a1".into(),
        tool: "Bash".into(),
        summary: "ls".into(),
        facts: ApprovalFacts::Shell {
            command: "ls".into(),
            cwd: None,
        },
        decision: AgentDecisionKind::Allowed,
        reason: "inScopeRead".into(),
        outcome: AgentDeliveryOutcome::Sent,
    };
    service
        .record_agent_decision("research", &decision)
        .unwrap();
    let decisions = agent_command(
        &service,
        json!({"kind":"readAgentDecisions","agentId":"research","limit":20}),
    );
    assert_eq!(decisions["decisions"][0]["approvalId"], "a1");

    // A deleted home is refused with a recreate prompt, never silently replaced.
    let read = service.agent_control_response().unwrap();
    agent_command(
        &service,
        json!({"kind":"newThread","expectedRevision":read["revision"],"agentId":"research"}),
    );
    fs::remove_dir_all(root.join("Research")).unwrap();
    assert_eq!(
        service
            .prepare_agent_thread("research")
            .unwrap_err()
            .message,
        "This agent's folder is missing. Recreate it to continue."
    );
    let recreated = agent_command(
        &service,
        json!({"kind":"recreateAgentHome","agentId":"research"}),
    );
    assert_eq!(
        recreated["agents"][0]["home"],
        root.join("Research").to_str().unwrap()
    );
    assert!(service.prepare_agent_thread("research").is_ok());
}

#[allow(clippy::needless_pass_by_value)]
fn configure_agent_work(
    service: &DesktopService,
    spec: interface_contracts::agent_work::AgentWorkSpec,
) {
    let read = agent_command(
        service,
        json!({"kind":"work","agentId":"research","command":{"action":"read"}}),
    );
    agent_command(
        service,
        json!({"kind":"work","agentId":"research","command":{"action":"configure","expectedRevision":read["work"]["revision"],"spec":spec}}),
    );
}
fn task_view(service: &DesktopService) -> Value {
    agent_command(
        service,
        json!({"kind":"work","agentId":"research","command":{"action":"read"}}),
    )["work"]
        .clone()
}
#[test]
fn agent_artifact_checks_reject_stale_invalid_missing_and_changed_bytes() {
    use interface_contracts::agent_work::{
        AgentArtifactCheck, AgentExecutionState, AgentWorkSpec, ArtifactFormat,
    };
    let (_temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    let home = service.prepare_agent_thread("research").unwrap().home;
    configure_agent_work(
        &service,
        AgentWorkSpec {
            artifacts: vec![AgentArtifactCheck {
                path: "report.json".into(),
                format: ArtifactFormat::Json,
                contains: Some("checked".into()),
            }],
            ..Default::default()
        },
    );
    service
        .begin_agent_work("research", "proof", "Produce checked JSON", &home)
        .unwrap();
    service.bind_agent_work("proof", "claude#proof").unwrap();
    assert!(
        service
            .agent_work_command(
                "research",
                interface_contracts::agent_work::AgentWorkCommand::Verify {
                    task_id: "proof".into()
                }
            )
            .is_err()
    );
    service
        .settle_agent_work(
            "research",
            "proof",
            AgentExecutionState::Completed,
            "Turn ended",
        )
        .unwrap();
    let verify = || {
        agent_command(
            &service,
            json!({"kind":"work","agentId":"research","command":{"action":"verify","taskId":"proof"}}),
        )
    };
    assert_eq!(verify()["work"]["tasks"][0]["verification"], "failed");
    let output = Path::new(&home).join("report.json");
    fs::write(&output, "not JSON checked").unwrap();
    assert_eq!(verify()["work"]["tasks"][0]["verification"], "failed");
    fs::write(&output, "{\"checked\":true}").unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    fs::File::options()
        .write(true)
        .open(&output)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(old))
        .unwrap();
    assert_eq!(verify()["work"]["tasks"][0]["verification"], "failed");
    fs::write(&output, "{\"checked\":true}").unwrap();
    assert_eq!(verify()["work"]["tasks"][0]["verification"], "verified");
    fs::write(&output, "{\"checked\":false}").unwrap();
    assert_eq!(task_view(&service)["tasks"][0]["verification"], "stale");
    // Polling cannot upgrade stale evidence, even if the new bytes satisfy the same shape.
    assert_eq!(task_view(&service)["tasks"][0]["verification"], "stale");
}
#[test]
#[allow(clippy::too_many_lines)]
fn agent_selected_consultations_require_exact_grant_review_and_source_bytes() {
    use interface_contracts::agent_work::{AgentConsultationGrant, AgentWorkSpec};
    let (_temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    let home = service.prepare_agent_thread("research").unwrap().home;
    fs::write(Path::new(&home).join("selected.txt"), "before").unwrap();
    configure_agent_work(
        &service,
        AgentWorkSpec {
            consultation: Some(AgentConsultationGrant {
                model: "codex/test-model".into(),
                files: vec!["selected.txt".into()],
                writable_files: vec!["selected.txt".into()],
                max_children: 2,
            }),
            ..Default::default()
        },
    );
    service
        .begin_agent_work("research", "parent", "Review selected text", &home)
        .unwrap();
    service.bind_agent_work("parent", "claude#parent").unwrap();
    let (child, packet, _) = service
        .prepare_agent_consultation("research", "parent", "Improve the selected text")
        .unwrap();
    assert!(packet.contains("before"));
    assert!(!packet.contains("MEMORY.md"));
    assert!(
        service
            .finish_agent_consultation(
                "research",
                "parent",
                &child.id,
                "bad",
                vec![("outside.txt".into(), Some("oops".into()))],
                None,
                1
            )
            .is_err()
    );
    let result: Value = serde_json::from_str(
        &service
            .finish_agent_consultation(
                "research",
                "parent",
                &child.id,
                "Source selected.txt, inspected exact captured text",
                vec![("selected.txt".into(), Some("after".into()))],
                Some(10),
                5,
            )
            .unwrap(),
    )
    .unwrap();
    let apply = |request: &str| json!({"requestId":request,"childId":child.id,"resultSha256":result["resultSha256"],"path":child.inputs[0].path});
    let rejected: Value = serde_json::from_str(
        &service
            .agent_tool_mutation(
                "research",
                "parent",
                "apply-before-review",
                "consult_apply",
                &apply("apply-before-review"),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(rejected["error"].as_str().unwrap().contains("review"));
    service.agent_tool_mutation("research","parent","review","consult_review",&json!({"requestId":"review","childId":child.id,"resultSha256":result["resultSha256"],"verdict":"accepted"})).unwrap();
    fs::write(Path::new(&home).join("selected.txt"), "concurrent edit").unwrap();
    let stale: Value = serde_json::from_str(
        &service
            .agent_tool_mutation(
                "research",
                "parent",
                "apply-stale",
                "consult_apply",
                &apply("apply-stale"),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(stale["error"].as_str().unwrap().contains("changed"));
    assert_eq!(
        fs::read_to_string(Path::new(&home).join("selected.txt")).unwrap(),
        "concurrent edit"
    );
    fs::write(Path::new(&home).join("selected.txt"), "before").unwrap();
    let applied: Value = serde_json::from_str(
        &service
            .agent_tool_mutation(
                "research",
                "parent",
                "apply-reviewed",
                "consult_apply",
                &apply("apply-reviewed"),
            )
            .unwrap(),
    )
    .unwrap();
    assert!(applied.get("error").is_none(), "{applied}");
    assert_eq!(
        fs::read_to_string(Path::new(&home).join("selected.txt")).unwrap(),
        "after"
    );
    assert_eq!(task_view(&service)["tasks"][0]["execution"], "running");
    assert_eq!(
        task_view(&service)["tasks"][0]["verification"],
        "unverified"
    );
}
#[test]
fn agent_tool_connection_tracks_same_thread_only_and_stop_fences_mutations() {
    use interface_contracts::agent_work::AgentExecutionState;
    let (_temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    let home = service.prepare_agent_thread("research").unwrap().home;
    service
        .begin_agent_work("research", "first", "one", &home)
        .unwrap();
    service.bind_agent_work("first", "claude#same").unwrap();
    service
        .settle_agent_work("research", "first", AgentExecutionState::Completed, "Done")
        .unwrap();
    assert!(service.agent_tool_task("research", "first").is_err());
    service
        .begin_agent_work("research", "second", "two", &home)
        .unwrap();
    service.bind_agent_work("second", "claude#same").unwrap();
    assert_eq!(
        service.agent_tool_task("research", "first").unwrap().id,
        "second"
    );
    service
        .stop_agent_work("research", "second", "Stop")
        .unwrap();
    assert!(
        service
            .agent_tool_mutation(
                "research",
                "first",
                "note",
                "notification_request",
                &json!({"requestId":"note","text":"late"})
            )
            .is_err()
    );
    assert!(
        service
            .begin_agent_work("research", "third", "three", &home)
            .is_err()
    );
}
#[test]
fn agent_fresh_inputs_and_memory_writers_fail_closed() {
    use interface_contracts::agent_work::{AgentInputRequirement, AgentWorkSpec};
    let (_temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    let home = service.prepare_agent_thread("research").unwrap().home;
    configure_agent_work(
        &service,
        AgentWorkSpec {
            inputs: vec![AgentInputRequirement {
                path: "input.txt".into(),
                max_age_seconds: 30,
            }],
            ..Default::default()
        },
    );
    assert!(
        service
            .begin_agent_work("research", "missing", "work", &home)
            .is_err()
    );
    fs::write(Path::new(&home).join("input.txt"), "fresh").unwrap();
    service
        .begin_agent_work("research", "active", "work", &home)
        .unwrap();
    let files = agent_command(
        &service,
        json!({"kind":"readAgentFiles","agentId":"research"}),
    );
    let expected = &files["files"]["memory"]["sha256"];
    assert!(service.dispatch_blocking(CHANNELS.agent_control,json!({"kind":"saveAgentMemory","agentId":"research","expectedSha256":expected,"text":"owner racing provider"})).is_err());
    assert!(
        service
            .prepare_agent_consultation("research", "active", "share unapproved context")
            .is_err()
    );
}

/// A saved project folder that was moved and replaced by a link to its new
/// home, with one saved Claude pane whose host proof still names the link.
struct MovedFolder {
    link: PathBuf,
    target: PathBuf,
    terminal_id: TerminalId,
}

fn moved_folder_with_saved_pane(fixture: &Fixture, service: &DesktopService) -> MovedFolder {
    let moved = fixture.home.join("Developer/app");
    fs::create_dir_all(&moved).expect("new home");
    let link = fixture.project.join("app");
    std::os::unix::fs::symlink(&moved, &link).expect("link left behind");
    let terminal_id = TerminalId::random();
    let launch_session = json!({
        "version":1,"id":"11111111-1111-4111-8111-111111111111","resume":true
    });
    service
        .record_terminal_reopen_authority_value(&TerminalReopenAuthority {
            root: link.to_string_lossy().into_owned(),
            managed_worktree_key: None,
            terminal: TerminalWorkspaceTerminal {
                id: terminal_id,
                title: "Claude".to_owned(),
                cwd: link.to_string_lossy().into_owned(),
                session_key: None,
                launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
                    version: 1,
                    profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Claude,
                }),
                launch_session: Some(
                    serde_json::from_value(launch_session.clone()).expect("session"),
                ),
            },
        })
        .expect("proof recorded before the move");
    service
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            saved_terminal_workspace_payload(&link, terminal_id, &launch_session),
        )
        .expect("saved roster names the old path");
    MovedFolder {
        link,
        target: fs::canonicalize(&moved).expect("canonical new home"),
        terminal_id,
    }
}

#[test]
fn a_saved_folder_that_became_a_link_resolves_to_its_real_directory() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let moved = moved_folder_with_saved_pane(&fixture, &service);

    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.resolve_moved_terminal_folder,
                json!({"path":moved.link}),
            )
            .expect("resolve the saved link"),
        json!(moved.target)
    );
    // The real directory has not moved, and a path no saved pane uses is not
    // this operation's business, even when it is a link.
    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.resolve_moved_terminal_folder,
                json!({"path":moved.target}),
            )
            .expect("resolve the real directory"),
        Value::Null
    );
    let unsaved = fixture.project.join("unsaved-link");
    std::os::unix::fs::symlink(&moved.target, &unsaved).expect("unsaved link");
    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.resolve_moved_terminal_folder,
                json!({"path":unsaved}),
            )
            .expect("resolve an unsaved link"),
        Value::Null
    );
    assert!(
        service
            .dispatch_blocking(
                CHANNELS.resolve_moved_terminal_folder,
                json!({"path":"app"})
            )
            .is_err(),
        "a relative path is refused"
    );
}

#[test]
fn a_confirmed_move_carries_the_saved_pane_proof_with_the_roster() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let moved = moved_folder_with_saved_pane(&fixture, &service);
    let mut payload = saved_terminal_workspace_payload(
        &moved.target,
        moved.terminal_id,
        &json!({"version":1,"id":"11111111-1111-4111-8111-111111111111","resume":true}),
    );
    payload["movedFolders"] = json!([{"from":moved.link,"to":moved.target}]);

    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, payload)
        .expect("save the moved roster");
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    let TerminalReopenAuthorityState::Authority(authority) =
        reopened.terminal_reopen_authority_state(moved.terminal_id)
    else {
        panic!("the moved pane must keep its restart proof");
    };
    assert_eq!(Path::new(&authority.root), moved.target);
    assert_eq!(Path::new(&authority.terminal.cwd), moved.target);
    assert_eq!(
        authority
            .terminal
            .launch_session
            .and_then(|session| session.id),
        Some("11111111-1111-4111-8111-111111111111".to_owned())
    );
}

#[test]
fn a_move_to_anywhere_but_the_links_current_target_changes_nothing() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let moved = moved_folder_with_saved_pane(&fixture, &service);
    let elsewhere = fixture.home.join("Elsewhere");
    fs::create_dir_all(&elsewhere).expect("elsewhere");
    let elsewhere = fs::canonicalize(elsewhere).expect("canonical elsewhere");
    let mut payload = saved_terminal_workspace_payload(
        &elsewhere,
        moved.terminal_id,
        &json!({"version":1,"id":"11111111-1111-4111-8111-111111111111","resume":true}),
    );
    payload["movedFolders"] = json!([{"from":moved.link,"to":elsewhere}]);

    let error = service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, payload)
        .expect_err("a target the link does not lead to is refused");
    assert_eq!(error.code, "terminal-folder-move-changed");
    let TerminalReopenAuthorityState::Authority(authority) =
        service.terminal_reopen_authority_state(moved.terminal_id)
    else {
        panic!("the refused save must leave the saved proof alone");
    };
    assert_eq!(Path::new(&authority.root), moved.link);
}

#[test]
fn moved_folder_payloads_match_the_shared_fixture() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/moved-terminal-folder-v27.json"
    ))
    .expect("shared moved-folder fixture");
    let resolve: crate::payload::PathPayload =
        serde_json::from_value(fixture["resolve"].clone()).expect("resolve payload");
    assert_eq!(resolve.path, "/Users/me/Desktop/Projects/app");
    let save: crate::payload::SetTerminalWorkspacesPayload =
        serde_json::from_value(fixture["save"].clone()).expect("save payload");
    assert_eq!(save.moved_folders.len(), 1);
    assert_eq!(save.moved_folders[0].from, "/Users/me/Desktop/Projects/app");
    assert_eq!(save.moved_folders[0].to, "/Users/me/Developer/app");
}

#[test]
fn agent_permission_modes_save_through_agent_control_and_reach_launches() {
    use interface_contracts::engine::AgentAccess;
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    service.set_agents_root(fixture.home.join("Interface Agents"));
    save_research_agent(&service);
    assert_eq!(service.agent_access("research"), None);
    let response = agent_command(
        &service,
        json!({"kind":"savePermissionMode","agentId":"research","mode":"bypass"}),
    );
    assert_eq!(response["permissionModes"], json!({"research": "bypass"}));
    assert_eq!(service.agent_access("research"), Some(AgentAccess::Bypass));
    assert_eq!(
        service.prepare_agent_thread("research").unwrap().access,
        Some(AgentAccess::Bypass)
    );
    service
        .bind_agent_thread("research", "claude#thread")
        .unwrap();
    assert_eq!(
        service.agent_binding("claude#thread").unwrap().access,
        Some(AgentAccess::Bypass)
    );
    let response = agent_command(
        &service,
        json!({"kind":"savePermissionMode","agentId":"research","mode":"gated"}),
    );
    assert!(response.get("permissionModes").is_none());
    assert_eq!(service.agent_binding("claude#thread").unwrap().access, None);
    let refused = service
        .agent_control(json!({"kind":"savePermissionMode","agentId":"ghost","mode":"auto"}))
        .unwrap_err();
    assert_eq!(refused.code, "agent-state-refused");
}

fn engine_event(value: Value) -> interface_contracts::engine::EngineEvent {
    serde_json::from_value(value).expect("engine event")
}

#[test]
fn an_unopened_agent_store_fails_no_session_event() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.state).unwrap();
    fs::write(fixture.state.join("agents.v2.json"), b"{").unwrap();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(service.agent_revision(), None, "the store did not open");
    for event in [
        json!({"kind":"status.changed","status":"working"}),
        json!({"kind":"tool.call","callId":"1","tool":"Read","summary":"Read","input":{}}),
        json!({"kind":"tool.result","callId":"1","ok":true,"summary":"done"}),
        json!({"kind":"usage.updated","inputTokens":1,"outputTokens":1}),
        json!({"kind":"session.closed"}),
    ] {
        service
            .observe_agent_work("codex#plain", &engine_event(event))
            .expect("an ordinary chat is never failed by the agent store");
    }
}

#[test]
fn only_the_task_session_carries_a_failed_task_projection() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    let home = service.prepare_agent_thread("research").unwrap().home;
    service
        .begin_agent_work("research", "task", "Summarize", &home)
        .unwrap();
    service.bind_agent_work("task", "claude#task").unwrap();
    let call = |id: &str| {
        engine_event(
            json!({"kind":"tool.call","callId":id,"tool":"Read","summary":"Read","input":{}}),
        )
    };
    let state = temp.path().join("state");
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    let plain = service.observe_agent_work("codex#plain", &call("1"));
    let task = service.observe_agent_work("claude#task", &call("2"));
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    plain.expect("another session's event is not the task's failure");
    assert_eq!(
        task.expect_err("the task's write failed").code,
        super::AGENT_WORK_PROJECTION_FAILED
    );
}

#[test]
fn a_text_delta_never_waits_for_the_agent_store() {
    let (_temp, _home, _work, service) = tmp_service();
    let service = &service;
    let delta = &engine_event(json!({"kind":"message.delta","text":"token"}));
    std::thread::scope(|scope| {
        let _held = service.agents.lock().unwrap();
        let (sent, observed) = std::sync::mpsc::channel();
        scope.spawn(move || {
            sent.send(service.observe_agent_event("codex#plain", delta))
                .unwrap();
        });
        let observed = observed
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("a text delta skips the store lock");
        assert!(!observed.unwrap());
    });
}

#[test]
fn an_agent_event_reports_whether_it_committed_a_revision() {
    let (_temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    service.prepare_agent_thread("research").unwrap();
    service
        .bind_agent_thread("research", "claude#thread")
        .unwrap();
    // A newly bound thread is working; its turn ending is the first change.
    let idle = engine_event(json!({"kind":"status.changed","status":"idle"}));
    let before = service.agent_revision();
    assert!(service.observe_agent_event("claude#thread", &idle).unwrap());
    assert_ne!(service.agent_revision(), before);
    let after = service.agent_revision();
    assert!(!service.observe_agent_event("claude#thread", &idle).unwrap());
    assert!(!service.observe_agent_event("codex#plain", &idle).unwrap());
    assert_eq!(service.agent_revision(), after);
}

#[test]
fn finder_is_asked_to_reveal_a_home_only_after_the_store_lock_is_released() {
    let (_temp, _home, _work, service) = tmp_service();
    save_research_agent(&service);
    super::agent_control::REVEALED_UNLOCKED.set(None);
    agent_command(
        &service,
        json!({"kind":"revealAgentHome","agentId":"research"}),
    );
    assert_eq!(super::agent_control::REVEALED_UNLOCKED.get(), Some(true));
}

#[test]
fn review_request_refusals_keep_their_prose() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let root = fixture.project.to_string_lossy().into_owned();
    let refusal = |operation: &str, payload: Value| {
        let error = service
            .dispatch_blocking(operation, payload)
            .expect_err("refused before any file work");
        assert_eq!(error.code, "request-refused", "{operation}");
        error.message
    };

    let restore = "Could not restore that Review checkpoint.\nThe restore request was invalid; no files were changed.";
    for payload in [
        json!({"root": root, "id": "a", "paths": "note.txt"}),
        json!({"root": root, "id": "a", "paths": ["note.txt", 1]}),
        json!({"root": root, "id": "a", "paths": null}),
        json!({"root": root, "id": "a", "paths": vec!["x"; 201]}),
        json!({"root": "", "id": "a"}),
        json!({"root": root, "id": ""}),
        json!({"root": root}),
    ] {
        assert_eq!(
            refusal(CHANNELS.restore_review_checkpoint, payload),
            restore
        );
    }

    let run = "Could not run Review checks.\nChoose checks offered for an exact open project and try again.";
    assert_eq!(
        refusal(
            CHANNELS.run_review_checks,
            json!({"root": "", "selected": []})
        ),
        run
    );
    assert_eq!(
        refusal(
            CHANNELS.run_review_checks,
            json!({"root": root, "selected": ["a", "b", "c", "d", "e", "f"]})
        ),
        run
    );

    for (operation, lead) in [
        (
            CHANNELS.create_review_checkpoint,
            "Could not create a Review checkpoint.",
        ),
        (
            CHANNELS.list_review_checkpoints,
            "Could not list Review checkpoints.",
        ),
    ] {
        let expected = format!("{lead}\nChoose an exact open project first.");
        assert_eq!(refusal(operation, json!({"root": ""})), expected);
        assert_eq!(refusal(operation, json!({})), expected);
    }
    assert_eq!(
        refusal(CHANNELS.list_customization, json!({"root": ""})),
        "Could not list agent customization.\nChoose an open project before looking for its rules, skills and hooks."
    );
    assert_eq!(
        refusal(CHANNELS.discover_review_checks, json!({"root": ""})),
        "Could not discover Review checks.\nChoose an open project before looking for package scripts."
    );
}

#[test]
fn acp_profile_refusals_keep_their_prose_and_order() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let base = json!({
        "slug":"local-two",
        "displayName":"Local Two",
        "executablePath":"/opt/local/bin/agent",
        "args":[]
    });
    let with = |field: &str, value: Value| {
        let mut profile = base.clone();
        profile[field] = value;
        profile
    };
    let refusal = |profiles: Value| {
        let error = service
            .dispatch_blocking(CHANNELS.set_acp_profiles, json!({ "profiles": profiles }))
            .expect_err("invalid roster");
        assert_eq!(error.code, "invalid-acp-profiles");
        error
            .message
            .strip_prefix("Invalid ACP profile configuration: ")
            .expect("ACP prefix")
            .to_owned()
    };
    let slug = "slug must be 1-64 lowercase letters, digits, or hyphens";
    let name = "displayName must be 1-80 visible characters";
    let executable = "executablePath must name an absolute, non-shell executable path";
    let count = "args must contain at most 64 exact argv entries";
    let argv = "each argv entry must be a string of at most 1024 characters without NUL";
    let credential = "args cannot carry credential-shaped flags or values";
    let cases = [
        (with("slug", json!("Bad")), slug.to_owned()),
        (with("slug", json!("-x")), slug.to_owned()),
        (with("slug", json!("a".repeat(65))), slug.to_owned()),
        (with("slug", json!(5)), slug.to_owned()),
        (
            with("slug", json!("qwen")),
            "slug \"qwen\" is reserved for Interface's built-in ACP provider".to_owned(),
        ),
        (with("displayName", json!("  ")), name.to_owned()),
        (with("displayName", json!("a\u{7f}")), name.to_owned()),
        (with("displayName", json!("a".repeat(81))), name.to_owned()),
        (
            with("displayName", json!(5)),
            "displayName must be a string".to_owned(),
        ),
        (
            with("executablePath", json!("relative/agent")),
            executable.to_owned(),
        ),
        (
            with("executablePath", json!("/bin/ZSH")),
            executable.to_owned(),
        ),
        (
            with("executablePath", json!("/opt/agent/")),
            executable.to_owned(),
        ),
        (
            with("executablePath", json!("/opt/a\nb")),
            executable.to_owned(),
        ),
        // 2,049 characters but 4,097 UTF-16 units: Electron's length unit.
        (
            with(
                "executablePath",
                json!(format!("/{}", "\u{1F600}".repeat(2_048))),
            ),
            executable.to_owned(),
        ),
        (with("executablePath", json!(7)), executable.to_owned()),
        (with("args", json!("acp")), count.to_owned()),
        (with("args", json!(vec!["a"; 65])), count.to_owned()),
        (with("args", json!(["a".repeat(1_025)])), argv.to_owned()),
        (with("args", json!(["a\u{0}b"])), argv.to_owned()),
        (with("args", json!(["--api_key=1"])), credential.to_owned()),
        (with("args", json!(["--Token"])), credential.to_owned()),
    ];
    for (profile, expected) in cases {
        assert_eq!(refusal(json!([profile])), expected, "{profile}");
    }

    // Field order: the slug is judged before a mistyped display name, the
    // argv count before its entries, and each entry in turn.
    let mut both = with("slug", json!("Bad"));
    both["displayName"] = json!(5);
    assert_eq!(refusal(json!([both])), slug);
    let mut many = vec![json!(7)];
    many.extend(std::iter::repeat_n(json!("a"), 64));
    assert_eq!(refusal(json!([with("args", json!(many))])), count);
    assert_eq!(
        refusal(json!([with("args", json!(["--password", 7]))])),
        credential
    );
    // The wire's credential match is ASCII, as in Electron; a later
    // profile's error is reported before the store's stricter check runs.
    let reserved = with("slug", json!("grok"));
    assert_eq!(
        refusal(json!([with("args", json!(["to\u{212A}en"])), reserved])),
        "slug \"grok\" is reserved for Interface's built-in ACP provider"
    );
    assert_eq!(
        refusal(json!([with("args", json!(["to\u{212A}en"]))])),
        credential
    );
}

#[test]
fn provider_inventory_requires_an_exact_grant_and_source_membership() {
    let fixture = Fixture::new();
    let file = fixture.project.join("AGENTS.md");
    fs::write(&file, "# Safe project rules").unwrap();
    let file = fs::canonicalize(file).unwrap();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let request = json!({"kind":"read","root":fixture.project,"provider":"codex"});
    assert!(
        service
            .dispatch_blocking(CHANNELS.customization_control, request.clone())
            .is_err()
    );
    service
        .dispatch_blocking(CHANNELS.get_default_directory, Value::Null)
        .unwrap();
    let result = service
        .dispatch_blocking(CHANNELS.customization_control, request)
        .unwrap();
    assert_eq!(result["loaded"], "unknown");
    let result = service
        .dispatch_blocking(
            CHANNELS.customization_control,
            json!({"kind":"source","root":fixture.project,"provider":"codex","path":file}),
        )
        .unwrap();
    assert_eq!(result["sourcePath"], file.to_string_lossy().as_ref());
    assert!(service.dispatch_blocking(CHANNELS.customization_control,
        json!({"kind":"source","root":fixture.project,"provider":"codex","path":fixture.home.join("outside") })).is_err());
}

#[test]
fn local_runner_preference_is_rust_owned_and_survives_host_reopen() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        service
            .dispatch_blocking(CHANNELS.background_control, json!({"kind":"read"}))
            .unwrap(),
        json!({"enabled":true,"available":true})
    );
    assert_eq!(
        service
            .dispatch_blocking(
                CHANNELS.background_control,
                json!({"kind":"set","enabled":false})
            )
            .unwrap(),
        json!({"enabled":false,"available":true})
    );
    drop(service);
    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        reopened
            .dispatch_blocking(CHANNELS.background_control, json!({"kind":"read"}))
            .unwrap(),
        json!({"enabled":false,"available":true})
    );
}

#[test]
fn in_flight_roster_save_before_filing_does_not_strip_newly_spawned_reopen_authority() {
    let fixture = Fixture::new();
    let service = fixture.service(DesktopServiceReviewPorts::default());
    let existing_id = TerminalId::random();
    let new_shell_id = TerminalId::random();
    let root = fixture.project.to_string_lossy().into_owned();
    let existing_session = json!({"version":1,"resume":true});
    let stale_payload =
        saved_terminal_workspace_payload(&fixture.project, existing_id, &existing_session);
    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, stale_payload.clone())
        .expect("seed existing roster");
    let new_shell_authority = TerminalReopenAuthority {
        root: root.clone(),
        managed_worktree_key: None,
        terminal: TerminalWorkspaceTerminal {
            id: new_shell_id,
            title: "Interface-Rust".to_owned(),
            cwd: root,
            session_key: None,
            launch: Some(interface_contracts::terminal::TerminalLaunchRequestV1 {
                version: 1,
                profile_id: interface_contracts::terminal::TerminalLaunchProfileId::Shell,
            }),
            launch_session: None,
        },
    };
    service
        .record_terminal_reopen_authority_value(&new_shell_authority)
        .expect("record newly spawned shell authority before renderer files it");
    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, stale_payload.clone())
        .expect("in-flight roster save serialized before new shell was filed");
    assert_eq!(
        service
            .live_terminal_reopen_authority(new_shell_id)
            .expect("authority store remains available"),
        Some(new_shell_authority.clone()),
    );

    let mut filed_payload = stale_payload.clone();
    filed_payload["workspaces"][0]["terminals"]
        .as_array_mut()
        .expect("terminals array")
        .push(json!({
            "id":new_shell_id,
            "title":"Interface-Rust",
            "cwd":fixture.project,
            "launch":{"version":1,"profileId":"shell"}
        }));
    service
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, filed_payload)
        .expect("renderer files newly spawned shell in roster");
    assert_eq!(
        service.terminal_reopen_authority_state(new_shell_id),
        TerminalReopenAuthorityState::Authority(new_shell_authority.clone())
    );
    drop(service);

    let reopened = fixture.service(DesktopServiceReviewPorts::default());
    assert_eq!(
        reopened.terminal_reopen_authority_state(new_shell_id),
        TerminalReopenAuthorityState::Authority(new_shell_authority)
    );
    reopened
        .dispatch_blocking(CHANNELS.set_terminal_workspaces, stale_payload)
        .expect("removing a previously saved terminal still narrows its authority");
    assert_eq!(
        reopened
            .live_terminal_reopen_authority(new_shell_id)
            .expect("authority store"),
        None
    );
}
