//! Cold-start restore matrix (opportunity review F2, 2026-10-15).
//!
//! Between 31 Aug and 7 Oct about two dozen fixes were for saved panes that
//! did not come back after a restart: they resumed the wrong chat, opened a
//! fresh one, were refused, or sat blank. Each row here saves one pane the
//! way the app does, restarts the host, sends the renderer's reopen request
//! and records what happened, across
//!
//! - every terminal profile, on fake fixed executables;
//! - how the pane got its chat: a launcher tile whose provider id the host
//!   never confirmed, one whose id the host filed (its capture), or a chat
//!   opened from the Session Library;
//! - its folder: as saved, opened through a symlink, moved with a link left
//!   at the old path, or gone;
//! - the front workspace or a parked one (asleep before the restart and
//!   reopened only after the front one has been put back and saved);
//! - a clean shutdown, or a host that died without one (its processes still
//!   running, nothing settled).
//!
//! Every pane must come back running, or fail with a code and a sentence the
//! renderer shows as a reopen failure. A create that answers without a
//! process, or that hangs, fails the row. A pane whose folder moved and left
//! a link must also reopen once the owner follows the move.
//!
//! The rows are pinned in `electron/src/fixtures/terminal-restore-matrix-v1.json`.
//! The renderer's `terminal-restore-matrix.test.ts` plays the same rows
//! through its own reopen planning (the saved entry to the request, the
//! request to a running or failed tab), so neither side can drift alone.
//! After a deliberate change, rewrite the fixture with
//! `INTERFACE_RESTORE_MATRIX_WRITE=1 cargo test -p interface-host restore_matrix`
//! and read the diff.
//!
//! The gate runs one row per profile, chat origin and folder (68 rows), with
//! the placement and shutdown rotating across them; every other row of the
//! fixture is checked with `INTERFACE_RESTORE_MATRIX=full` (272 rows).
//! Outside this matrix: the startup sweep that settles a dead host's capture
//! receipts on real processes (`claude_terminal_session_tests.rs`) and the
//! renderer's mounted center (`terminal-reopen-fixture.tsx`).

use std::collections::BTreeMap;

use interface_contracts::terminal::{TerminalLaunchRequestV1, TerminalLaunchSessionV1};
use serde_json::Value;

use super::*;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../electron/src/fixtures/terminal-restore-matrix-v1.json"
);
const FIXTURE: &str =
    include_str!("../../../electron/src/fixtures/terminal-restore-matrix-v1.json");

/// The id the renderer mints on a launcher tile (`newChatBinding`).
const MINTED: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
/// The id the renderer mints when a reopen has no usable saved chat.
const MINTED_ON_REOPEN: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
/// What each provider's own session store calls the pane's chat.
const CLAUDE_CHAT: &str = "44444444-4444-4444-8444-444444444444";
const CODEX_THREAD: &str = "0199e000-0000-7000-8000-000000000007";
const OPENCODE_SESSION: &str = "ses_matrix0001";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Origin {
    /// A launcher tile; the provider id never reached the host.
    Launcher,
    /// A launcher tile whose provider id the host filed.
    Captured,
    /// A chat opened from the Session Library.
    Library,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Folder {
    Plain,
    Linked,
    MovedWithLink,
    Gone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Placement {
    Front,
    Parked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostEnd {
    Clean,
    Killed,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    profile: TerminalLaunchProfileId,
    origin: Origin,
    folder: Folder,
    placement: Placement,
    host: HostEnd,
}

const PROFILES: [TerminalLaunchProfileId; 10] = [
    TerminalLaunchProfileId::Shell,
    TerminalLaunchProfileId::Claude,
    TerminalLaunchProfileId::Codex,
    TerminalLaunchProfileId::Grok,
    TerminalLaunchProfileId::Opencode,
    TerminalLaunchProfileId::Qwen,
    TerminalLaunchProfileId::Gemini,
    TerminalLaunchProfileId::Omp,
    TerminalLaunchProfileId::Muse,
    TerminalLaunchProfileId::Kimi,
];

fn profile_name(profile: TerminalLaunchProfileId) -> String {
    serde_json::to_value(profile)
        .expect("profile id")
        .as_str()
        .expect("profile id text")
        .to_owned()
}

/// Profiles whose pane keeps a chat (the renderer's `RESUMABLE`).
fn resumable(profile: TerminalLaunchProfileId) -> bool {
    matches!(
        profile,
        TerminalLaunchProfileId::Claude
            | TerminalLaunchProfileId::Codex
            | TerminalLaunchProfileId::Grok
            | TerminalLaunchProfileId::Opencode
    )
}

/// Profiles the renderer mints a pane handle for (`MINTS_CHAT_ID`).
fn mints(profile: TerminalLaunchProfileId) -> bool {
    matches!(
        profile,
        TerminalLaunchProfileId::Claude
            | TerminalLaunchProfileId::Codex
            | TerminalLaunchProfileId::Grok
    )
}

impl Case {
    fn name(self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            profile_name(self.profile),
            match self.origin {
                Origin::Launcher => "launcher",
                Origin::Captured => "captured",
                Origin::Library => "library",
            },
            match self.folder {
                Folder::Plain => "plain",
                Folder::Linked => "linked",
                Folder::MovedWithLink => "moved-with-link",
                Folder::Gone => "gone",
            },
            match self.placement {
                Placement::Front => "front",
                Placement::Parked => "parked",
            },
            match self.host {
                HostEnd::Clean => "clean",
                HostEnd::Killed => "killed",
            },
        )
    }

    /// The provider's own id for this pane's chat, once one exists.
    fn provider_chat(self) -> Option<&'static str> {
        match (self.profile, self.origin) {
            (_, Origin::Launcher) => None,
            // A Grok chat from the Session Library has a UUID like Claude's.
            (TerminalLaunchProfileId::Claude, _)
            | (TerminalLaunchProfileId::Grok, Origin::Library) => Some(CLAUDE_CHAT),
            (TerminalLaunchProfileId::Codex, _) => Some(CODEX_THREAD),
            (TerminalLaunchProfileId::Opencode, _) => Some(OPENCODE_SESSION),
            // Grok keeps the id Interface gives it.
            (TerminalLaunchProfileId::Grok, Origin::Captured) => Some(MINTED),
            _ => None,
        }
    }
}

/// Every row, in fixture order.
fn all_cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for profile in PROFILES {
        let origins: &[Origin] = if resumable(profile) {
            &[Origin::Launcher, Origin::Captured, Origin::Library]
        } else {
            &[Origin::Launcher]
        };
        for &origin in origins {
            for folder in [
                Folder::Plain,
                Folder::Linked,
                Folder::MovedWithLink,
                Folder::Gone,
            ] {
                for placement in [Placement::Front, Placement::Parked] {
                    for host in [HostEnd::Clean, HostEnd::Killed] {
                        cases.push(Case {
                            profile,
                            origin,
                            folder,
                            placement,
                            host,
                        });
                    }
                }
            }
        }
    }
    cases
}

/// The gate's rows: one per profile, chat origin and folder, the placement
/// and shutdown rotating so each folder meets each of their four pairings.
fn gate_cases() -> Vec<Case> {
    // Each chunk of four is one profile/origin/folder over the four
    // placement × shutdown pairings, in that order.
    all_cases()
        .chunks(4)
        .enumerate()
        .map(|(group, chunk)| chunk[group % 4])
        .collect()
}

/// The provider capture receipts: once a row's chat is filed, the provider
/// keeps reporting it for that profile, before and after the restart, as the
/// receipt store does.
#[derive(Default)]
struct Captures {
    filed: Mutex<Option<(TerminalLaunchProfileId, String)>>,
}

impl Captures {
    fn file(&self, profile: TerminalLaunchProfileId, chat: &str) {
        *mutex_lock(&self.filed) = Some((profile, chat.to_owned()));
    }
}

impl TerminalProviderSessionLookup for Captures {
    fn begin_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _owner: &TerminalProviderSessionCaptureOwner,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    fn record_process(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
        _process: &TerminalProcessIdentity,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }

    fn current_session(
        &self,
        profile_id: TerminalLaunchProfileId,
        _process: &TerminalProcessIdentity,
        _capture_token: &str,
    ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
        Ok(mutex_lock(&self.filed)
            .as_ref()
            .filter(|(profile, _)| *profile == profile_id)
            .map(|(_, chat)| TerminalProviderSession {
                id: chat.clone(),
                transition: TerminalProviderSessionTransition::Resume,
            }))
    }

    fn end_capture(
        &self,
        _profile_id: TerminalLaunchProfileId,
        _capture_token: &str,
    ) -> Result<(), TerminalBackendError> {
        Ok(())
    }
}

/// The launch catalog the app builds, on fixed fake executables: Claude,
/// Codex and `OpenCode` capture their chat, and Codex and Grok reopens look in
/// their own session stores.
struct Programs {
    _directory: TempDir,
    codex_sessions: PathBuf,
    grok_sessions: PathBuf,
    opencode_receipts: PathBuf,
}

impl Programs {
    fn new() -> Self {
        let directory = TempDir::new().expect("launch directory");
        let codex_sessions = directory.path().join("codex-sessions");
        let grok_sessions = directory.path().join("grok-sessions");
        let opencode_receipts = directory.path().join("opencode-receipts");
        for store in [&codex_sessions, &grok_sessions, &opencode_receipts] {
            fs::create_dir_all(store).expect("provider store");
        }
        Self {
            _directory: directory,
            codex_sessions,
            grok_sessions,
            opencode_receipts,
        }
    }

    fn catalog(&self) -> TerminalLaunchCatalog {
        let directory = self.codex_sessions.parent().expect("launch directory");
        let program = |profile: TerminalLaunchProfileId,
                       arguments: Vec<OsString>,
                       environment: Vec<(OsString, OsString)>| {
            let name = format!("fake-{}", profile_name(profile));
            let path = directory.join(&name);
            if !path.exists() {
                fs::write(&path, "#!/bin/sh\nexit 0\n").expect("fake executable");
                #[cfg(unix)]
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("permissions");
            }
            TerminalLaunchProgram::new(profile, &path, arguments, environment)
                .expect("fixed launch program")
        };
        let mut programs = vec![
            program(
                TerminalLaunchProfileId::Claude,
                vec![OsString::from(format!(
                    "--capture={CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER}"
                ))],
                Vec::new(),
            )
            .with_provider_session_capture()
            .expect("capturing Claude"),
            program(TerminalLaunchProfileId::Codex, Vec::new(), Vec::new())
                .with_provider_session_capture()
                .expect("capturing Codex"),
            program(
                TerminalLaunchProfileId::Opencode,
                Vec::new(),
                vec![(
                    OsString::from("INTERFACE_OPENCODE_RECEIPTS_ROOT"),
                    self.opencode_receipts.clone().into_os_string(),
                )],
            )
            .with_provider_session_capture()
            .expect("capturing OpenCode"),
        ];
        for profile in [
            TerminalLaunchProfileId::Grok,
            TerminalLaunchProfileId::Qwen,
            TerminalLaunchProfileId::Gemini,
            TerminalLaunchProfileId::Omp,
            TerminalLaunchProfileId::Muse,
            TerminalLaunchProfileId::Kimi,
        ] {
            programs.push(program(profile, Vec::new(), Vec::new()));
        }
        TerminalLaunchCatalog::new(programs)
            .expect("launch catalog")
            .with_codex_sessions_root(self.codex_sessions.clone())
            .with_grok_sessions_root(self.grok_sessions.clone())
    }

    /// The provider still holds the chat the pane was bound to.
    fn keep_chat(&self, profile: TerminalLaunchProfileId, chat: &str) {
        match profile {
            TerminalLaunchProfileId::Codex => {
                // Codex files a thread under the day its v7 id was minted.
                let millis = i64::from_str_radix(&format!("{}{}", &chat[..8], &chat[9..13]), 16)
                    .expect("v7 timestamp");
                let (year, month, day) =
                    interface_common::time::civil_from_days(millis.div_euclid(86_400_000));
                let folder = self
                    .codex_sessions
                    .join(format!("{year:04}/{month:02}/{day:02}"));
                fs::create_dir_all(&folder).expect("codex day");
                fs::write(
                    folder.join(format!(
                        "rollout-{year:04}-{month:02}-{day:02}T00-00-00-{chat}.jsonl"
                    )),
                    b"{}\n",
                )
                .expect("codex rollout");
            }
            TerminalLaunchProfileId::Grok => {
                fs::create_dir_all(self.grok_sessions.join("%2Fproject").join(chat))
                    .expect("grok session");
            }
            _ => {}
        }
    }
}

impl Fixture {
    /// Start a host on this fixture's files the way the app restarts.
    /// `Clean` closes the previous host first. `Killed` never closes it:
    /// nothing is flushed or settled, its panes' programs get the hangup a
    /// closing PTY sends, and its file locks go with the process.
    async fn restart_host(
        self,
        end: HostEnd,
        launches: TerminalLaunchCatalog,
        captures: Arc<Captures>,
    ) -> Self {
        let Fixture {
            temporary,
            root,
            default_root,
            desktop,
            backend,
            releases,
            pushes,
            dispatcher,
            mut plane,
            electron_input,
            electron_output,
        } = self;
        match end {
            HostEnd::Clean => {
                dispatcher.close().await;
                if let Some(plane) = plane.take() {
                    plane.shutdown().await;
                }
            }
            HostEnd::Killed => {
                drop(plane.take());
                let running = mutex_lock(&backend.processes).clone();
                for process in running {
                    process.stop();
                }
            }
        }
        let previous = Arc::downgrade(&desktop);
        // The last handles may run synchronous teardown that sends on the
        // lifecycle channel; that must not happen on a runtime thread.
        tokio::task::spawn_blocking(move || {
            drop((dispatcher, desktop, pushes, electron_input, electron_output));
        })
        .await
        .expect("drop the previous host");
        eventually("the previous host lets go of its files", || {
            previous.upgrade().is_none()
        })
        .await;
        let desktop = Arc::new(
            DesktopService::open(
                DesktopServiceConfig {
                    appearance_file: temporary.path().join("appearance.json"),
                    acp_profiles_file: temporary.path().join("acp-profiles.json"),
                    terminal_workspaces_file: temporary.path().join("terminal-workspaces.json"),
                    home_directory: temporary.path().join("home"),
                    customization_user_root: None,
                    review_checkpoint_store_path: Some(temporary.path().join("review-checkpoints")),
                },
                DesktopServiceReviewPorts::default(),
            )
            .expect("restarted desktop service"),
        );
        let (push_sender, pushes) = mpsc::channel(256);
        let (electron_input, host_input) = tokio::io::duplex(64 * 1024);
        let (host_output, electron_output) = tokio::io::duplex(64 * 1024);
        let mut plane = TerminalDataPlane::start(
            host_input,
            host_output,
            TerminalDataTransportConfig::default(),
        );
        let provider_sessions: Arc<dyn TerminalProviderSessionLookup> = captures;
        let dispatcher = Arc::new(
            TerminalHostDispatcher::with_backend_launches_and_claude_store(
                Arc::new(FakeRuntime::default()),
                Arc::clone(&desktop),
                releases.clone(),
                push_sender,
                Some(plane.outbound.clone()),
                TerminalServiceConfig::default(),
                backend.clone(),
                launches,
                None,
                Some(provider_sessions),
            )
            .expect("restarted terminal dispatcher"),
        );
        let failures = plane.subscribe_failures();
        let (_unused_sender, empty_receiver) = mpsc::channel(1);
        let inbound = std::mem::replace(&mut plane.inbound, empty_receiver);
        dispatcher.start_input_pump(inbound).expect("input pump");
        dispatcher
            .start_failure_monitor(failures)
            .expect("failure monitor");
        Fixture {
            temporary,
            root,
            default_root,
            desktop,
            backend,
            releases,
            pushes,
            dispatcher,
            plane: Some(plane),
            electron_input,
            electron_output,
        }
    }
}

/// The renderer's `reopenedChatBinding`, with its reopen mint fixed.
fn renderer_reopen_binding(
    profile: TerminalLaunchProfileId,
    saved: Option<&TerminalLaunchSessionV1>,
) -> Option<TerminalLaunchSessionV1> {
    let binding = |id: Option<&str>, resume: bool| {
        Some(TerminalLaunchSessionV1 {
            version: 1,
            id: id.map(str::to_owned),
            resume,
        })
    };
    if !resumable(profile) {
        return None;
    }
    let saved = saved.filter(|saved| saved.version == 1);
    if profile == TerminalLaunchProfileId::Claude && saved.is_some_and(|saved| saved.id.is_none()) {
        return binding(None, true);
    }
    if profile == TerminalLaunchProfileId::Opencode {
        return match saved.and_then(|saved| saved.id.as_deref()) {
            Some(id) if id.starts_with("ses_") => binding(Some(id), true),
            _ => binding(None, true),
        };
    }
    if let Some(id) = saved.and_then(|saved| saved.id.as_deref()).filter(|id| {
        uuid::Uuid::parse_str(id).is_ok_and(|id| matches!(id.get_version_num(), 4 | 7))
    }) {
        return binding(Some(id), true);
    }
    if mints(profile) {
        binding(Some(MINTED_ON_REOPEN), false)
    } else {
        binding(None, false)
    }
}

/// How the spawned program was asked to open its chat.
fn resume_shape(spec: &TerminalSpawnSpec, profile: TerminalLaunchProfileId) -> String {
    let ResolvedTerminalLaunch::Program {
        arguments,
        environment,
        ..
    } = &spec.launch
    else {
        return "shell".to_owned();
    };
    let arguments = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let flag = |prefix: &str| {
        arguments
            .iter()
            .find_map(|argument| argument.strip_prefix(prefix).map(str::to_owned))
    };
    let named = |id: &str| match id {
        CLAUDE_CHAT | CODEX_THREAD | OPENCODE_SESSION => "the chat".to_owned(),
        MINTED => "the launcher's chat".to_owned(),
        MINTED_ON_REOPEN => "a new chat".to_owned(),
        other => format!("unexpected id {other}"),
    };
    match profile {
        TerminalLaunchProfileId::Claude | TerminalLaunchProfileId::Grok => {
            if let Some(id) = flag("--resume=") {
                format!("resume {}", named(&id))
            } else if arguments.iter().any(|argument| argument == "--resume") {
                "picker".to_owned()
            } else if let Some(id) = flag("--session-id=") {
                format!("start {}", named(&id))
            } else {
                "fresh".to_owned()
            }
        }
        TerminalLaunchProfileId::Codex => match arguments.first().map(String::as_str) {
            Some("resume") => match arguments.get(1).map(String::as_str) {
                Some("--last") => "resume the folder's last chat".to_owned(),
                Some(id) if !id.starts_with('-') => format!("resume {}", named(id)),
                _ => "picker".to_owned(),
            },
            _ => "fresh".to_owned(),
        },
        TerminalLaunchProfileId::Opencode => {
            if let Some(id) = flag("--session=") {
                format!("resume {}", named(&id))
            } else if environment
                .iter()
                .any(|(name, _)| name == "INTERFACE_OPENCODE_RESUME_PICKER")
            {
                "picker".to_owned()
            } else {
                "fresh".to_owned()
            }
        }
        _ => "fresh".to_owned(),
    }
}

/// Paths in a row, written as the fixture's placeholders.
struct Places {
    root: PathBuf,
    default: PathBuf,
    link: PathBuf,
    moved: PathBuf,
    pane: String,
    front: String,
}

impl Places {
    fn text(&self, value: &str) -> String {
        let moved = self.moved.to_string_lossy();
        let link = self.link.to_string_lossy();
        let root = self.root.to_string_lossy();
        value
            .replace(moved.as_ref(), "<moved>")
            .replace(link.as_ref(), "<link>")
            .replace(root.as_ref(), "<root>")
            .replace(self.default.to_string_lossy().as_ref(), "<default>")
            .replace(&self.pane, "<pane>")
            .replace(&self.front, "<front>")
    }

    fn json(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.json(item)).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), self.json(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

const REOPEN_DEADLINE: Duration = Duration::from_secs(10);

/// One reopen as the renderer sends it, and what came of it.
async fn reopen(fixture: &Fixture, request: &Value, profile: TerminalLaunchProfileId) -> Value {
    let spawned = mutex_lock(&fixture.backend.specs).len();
    let answer = tokio::time::timeout(
        REOPEN_DEADLINE,
        fixture
            .dispatcher
            .dispatch(CHANNELS.create_terminal, request.clone()),
    )
    .await
    .unwrap_or_else(|_| panic!("the reopen hung: {request}"));
    let specs = mutex_lock(&fixture.backend.specs);
    match answer {
        Ok(summary) => {
            assert_eq!(
                specs.len(),
                spawned + 1,
                "a reopen that answers must have started exactly one process: {summary}"
            );
            assert_eq!(summary["id"], request["id"]);
            assert_eq!(summary["state"], "running", "{summary}");
            json!({
                "kind": "reopened",
                "resume": resume_shape(specs.last().expect("spawned"), profile),
            })
        }
        Err(error) => {
            assert!(
                !error.code.is_empty() && !error.message.trim().is_empty(),
                "a refused reopen says why: {error:?}"
            );
            json!({
                "kind": "failed",
                "code": error.code,
                "message": error.message,
                "retryable": error.retryable,
                "replaySafe": error.replay_safe,
            })
        }
    }
}

fn workspace(id: &str, terminals: &[&Value]) -> Value {
    json!({
        "id": id,
        "name": id,
        "createdAt": "2026-10-15T00:00:00Z",
        "updatedAt": "2026-10-15T00:00:00Z",
        "terminals": terminals,
    })
}

fn save(fixture: &Fixture, front: &[&Value], parked: &[&Value]) {
    let mut workspaces = vec![workspace("front", front)];
    if !parked.is_empty() {
        workspaces.push(workspace("parked", parked));
    }
    fixture
        .desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces": workspaces}),
        )
        .expect("save the roster");
}

/// What the renderer files for a created pane (`rememberTerminal`).
fn saved_entry(summary: &Value, binding: Option<&TerminalLaunchSessionV1>) -> Value {
    let mut entry = json!({
        "id": summary["id"],
        "title": summary["title"],
        "cwd": summary["cwd"],
    });
    if !summary["launch"].is_null() {
        entry["launch"] = summary["launch"].clone();
    }
    let session = if summary["launchSession"].is_null() {
        binding.map(|binding| serde_json::to_value(binding).expect("binding"))
    } else {
        Some(summary["launchSession"].clone())
    };
    if let Some(session) = session {
        entry["launchSession"] = session;
    }
    entry
}

/// The renderer's reopen request for a saved entry (`reopenRequests`).
fn reopen_request(entry: &Value, profile: TerminalLaunchProfileId) -> Value {
    let saved: Option<TerminalLaunchSessionV1> =
        serde_json::from_value(entry["launchSession"].clone()).ok();
    let mut request = json!({
        "id": entry["id"],
        "cwd": entry["cwd"],
        "title": entry["title"],
    });
    if !entry["launch"].is_null() {
        request["launch"] = entry["launch"].clone();
    }
    if let Some(binding) = renderer_reopen_binding(profile, saved.as_ref()) {
        request["launchSession"] = serde_json::to_value(binding).expect("binding");
    }
    request
}

#[allow(clippy::too_many_lines)] // One row, in the order the app lives it.
async fn run_case(case: Case) -> Value {
    let trace = |step: &str| {
        if std::env::var_os("INTERFACE_RESTORE_MATRIX_TRACE").is_some() {
            eprintln!("restore matrix: {}: {step}", case.name());
        }
    };
    let programs = Programs::new();
    let captures = Arc::new(Captures::default());
    let mut fixture = Fixture::new_with_launches_and_provider_sessions(
        programs.catalog(),
        Some(captures.clone()),
    );
    let pane = uuid::Uuid::new_v4();
    let front_id = uuid::Uuid::new_v4();
    let temporary = fs::canonicalize(fixture.temporary.path()).expect("canonical temporary");
    let places = Places {
        root: fixture.root.clone(),
        default: fixture.default_root.clone(),
        link: temporary.join("project-link"),
        moved: temporary.join("moved-project"),
        pane: pane.to_string(),
        front: front_id.to_string(),
    };
    #[cfg(unix)]
    std::os::unix::fs::symlink(&places.root, &places.link).expect("link to the project");

    // The front workspace always holds a plain terminal of its own.
    let front = fixture
        .dispatcher
        .dispatch(
            CHANNELS.create_terminal,
            create_payload(front_id, &fixture.root),
        )
        .await
        .expect("front terminal");
    let front_entry = saved_entry(&front, None);

    trace("front created");
    // The pane under test, launched the way its origin does.
    let cwd = match case.folder {
        Folder::Linked => places.link.clone(),
        _ => places.root.clone(),
    };
    let mut first = create_payload(pane, &cwd);
    let launch =
        (case.profile != TerminalLaunchProfileId::Shell).then_some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: case.profile,
        });
    if let Some(launch) = launch {
        first["launch"] = serde_json::to_value(launch).expect("launch");
    }
    let first_binding = match case.origin {
        Origin::Library => case.provider_chat().map(|chat| TerminalLaunchSessionV1 {
            version: 1,
            id: Some(chat.to_owned()),
            resume: true,
        }),
        Origin::Launcher | Origin::Captured if resumable(case.profile) => {
            Some(TerminalLaunchSessionV1 {
                version: 1,
                id: mints(case.profile).then(|| MINTED.to_owned()),
                resume: false,
            })
        }
        _ => None,
    };
    if let Some(chat) = case.provider_chat() {
        programs.keep_chat(case.profile, chat);
    }
    if let Some(binding) = &first_binding {
        first["launchSession"] = serde_json::to_value(binding).expect("binding");
    }
    let created = fixture
        .dispatcher
        .dispatch(CHANNELS.create_terminal, first)
        .await
        .unwrap_or_else(|error| panic!("{}: first launch: {error:?}", case.name()));
    let mut entry = saved_entry(&created, first_binding.as_ref());
    save(
        &fixture,
        &if case.placement == Placement::Front {
            vec![&front_entry, &entry]
        } else {
            vec![&front_entry]
        },
        &if case.placement == Placement::Parked {
            vec![&entry]
        } else {
            vec![]
        },
    );

    trace("pane created and saved");
    // The host files the provider's own id the way its capture does. Grok
    // has no capture: it keeps the launcher's id, which its store now holds.
    if case.origin == Origin::Captured
        && case.profile != TerminalLaunchProfileId::Grok
        && let Some(chat) = case.provider_chat()
    {
        let mut summary = fixture
            .dispatcher
            .service
            .snapshot()
            .terminals
            .into_iter()
            .find(|terminal| terminal.id == TerminalId::new(pane))
            .expect("live pane");
        summary.launch_session = Some(TerminalLaunchSessionV1 {
            version: 1,
            id: Some(chat.to_owned()),
            resume: true,
        });
        // The host files it first; Claude's and Codex's land in the roster
        // at once, OpenCode's once the renderer has filed the upsert too.
        let first = fixture.desktop.persist_terminal_provider_reopen(&summary);
        entry["launchSession"] = json!({"version": 1, "id": chat, "resume": true});
        captures.file(case.profile, chat);
        save(
            &fixture,
            &if case.placement == Placement::Front {
                vec![&front_entry, &entry]
            } else {
                vec![&front_entry]
            },
            &if case.placement == Placement::Parked {
                vec![&entry]
            } else {
                vec![]
            },
        );
        fixture
            .desktop
            .persist_terminal_provider_reopen(&summary)
            .unwrap_or_else(|error| {
                panic!("{}: file the chat: {first:?}, then {error:?}", case.name())
            });
    }

    trace("captured");
    // A parked workspace's panes sleep before the restart.
    if case.placement == Placement::Parked {
        fixture
            .dispatcher
            .dispatch(
                CHANNELS.set_terminal_workspace_activity,
                json!({"version": 1, "id": pane, "generation": created["generation"], "active": false}),
            )
            .await
            .unwrap_or_else(|error| panic!("{}: park: {error:?}", case.name()));
    }

    trace("parked");
    // The renderer's roster as it stood at the end, read back from the host.
    let roster = fixture
        .desktop
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
        .expect("saved roster");
    let saved = roster
        .as_array()
        .expect("roster")
        .iter()
        .flat_map(|workspace| {
            workspace["terminals"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .find(|terminal| terminal["id"] == json!(pane))
        .unwrap_or_else(|| panic!("{}: the pane was saved", case.name()));
    assert_eq!(
        saved["launchSession"],
        entry["launchSession"],
        "{}: the host's roster and the renderer's agree on the chat",
        case.name()
    );

    trace("roster read");
    // The project moves or disappears while the app is closed.
    match case.folder {
        Folder::MovedWithLink => {
            fs::rename(&places.root, &places.moved).expect("move the project");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&places.moved, &places.root).expect("link left behind");
        }
        Folder::Gone => {
            fs::rename(&places.root, &places.moved).expect("move the project away");
            fs::remove_dir_all(&places.moved).expect("delete the project");
        }
        Folder::Plain | Folder::Linked => {}
    }

    trace("folder changed");
    fixture = fixture
        .restart_host(case.host, programs.catalog(), captures.clone())
        .await;

    trace("restarted");
    // The front workspace comes back first; a parked one only when the owner
    // opens it, after the front's reopen has been saved again.
    let front_outcome = reopen(
        &fixture,
        &reopen_request(&front_entry, TerminalLaunchProfileId::Shell),
        TerminalLaunchProfileId::Shell,
    )
    .await;
    if case.placement == Placement::Parked {
        let roster = fixture
            .desktop
            .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
            .expect("roster after the front reopen");
        fixture
            .desktop
            .dispatch_blocking(
                CHANNELS.set_terminal_workspaces,
                json!({"workspaces": roster}),
            )
            .expect("the renderer saves the front workspace again");
    }
    trace("front reopened");
    let request = reopen_request(&saved, case.profile);
    let outcome = reopen(&fixture, &request, case.profile).await;
    trace("pane reopened");
    let mut row = json!({
        "case": case.name(),
        "saved": places.json(&saved),
        "request": places.json(&request),
        "outcome": places.json(&outcome),
        "front": places.json(&front_outcome),
    });

    if row["outcome"]["kind"] == "failed" && request.get("launchSession").is_none() {
        // A pane without a chat comes back as a plain terminal if it can
        // (`shellFallbackRequests`): in its folder, then in the default one.
        let default = places.default.to_string_lossy().into_owned();
        let mut folders = vec![request["cwd"].as_str().expect("cwd").to_owned()];
        if folders[0] != default {
            folders.push(default);
        }
        let mut fallbacks = Vec::new();
        for cwd in folders {
            let plain = json!({"id": request["id"], "cwd": cwd, "title": "Terminal"});
            let answer = reopen(&fixture, &plain, TerminalLaunchProfileId::Shell).await;
            let reopened = answer["kind"] == "reopened";
            fallbacks
                .push(json!({"request": places.json(&plain), "outcome": places.json(&answer)}));
            if reopened {
                break;
            }
        }
        row["fallbacks"] = Value::Array(fallbacks);
    }
    if row["outcome"]["kind"] == "failed" && !fell_back(&row) {
        // What the failed tab offers: a moved folder's new place.
        let moved_to = fixture
            .desktop
            .dispatch_blocking(
                CHANNELS.resolve_moved_terminal_folder,
                json!({"path": request["cwd"]}),
            )
            .expect("moved-folder lookup");
        row["outcome"]["movedTo"] = places.json(&moved_to);
        if !moved_to.is_null() {
            // The owner follows the move: the roster and the proofs move in
            // one save, then the failed pane tries again there.
            let roster = fixture
                .desktop
                .dispatch_blocking(CHANNELS.get_terminal_workspaces, json!(null))
                .expect("roster before the move");
            let from = request["cwd"].as_str().expect("cwd").to_owned();
            let to = moved_to.as_str().expect("moved to").to_owned();
            let relocated = serde_json::from_str::<Value>(
                &roster
                    .to_string()
                    .replace(&format!("\"{from}\""), &format!("\"{to}\"")),
            )
            .expect("relocated roster");
            fixture
                .desktop
                .dispatch_blocking(
                    CHANNELS.set_terminal_workspaces,
                    json!({"workspaces": relocated, "movedFolders": [{"from": from, "to": to}]}),
                )
                .unwrap_or_else(|error| panic!("{}: follow the move: {error:?}", case.name()));
            let mut retry = request.clone();
            retry["cwd"] = json!(to);
            row["afterMove"] = places.json(&reopen(&fixture, &retry, case.profile).await);
        }
    }
    trace("done");
    fixture.shutdown().await;
    row
}

/// Whether the pane came back as a plain terminal after its own reopen failed.
fn fell_back(row: &Value) -> bool {
    row["fallbacks"]
        .as_array()
        .and_then(|fallbacks| fallbacks.last())
        .is_some_and(|last| last["outcome"]["kind"] == "reopened")
}

fn fixture_rows() -> BTreeMap<String, Value> {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("restore matrix fixture");
    fixture["rows"]
        .as_array()
        .expect("fixture rows")
        .iter()
        .map(|row| {
            (
                row["case"].as_str().expect("case name").to_owned(),
                row.clone(),
            )
        })
        .collect()
}

/// What is wrong with a row, whatever the fixture says: a tab that neither
/// came back nor said why, a moved folder that does not reopen once followed,
/// or a known chat that does not resume as itself.
fn blank_or_lost(case: Case, row: &Value) -> Vec<String> {
    let name = case.name();
    let outcome = &row["outcome"];
    let mut wrong = Vec::new();
    for tab in [outcome, &row["front"]] {
        let said_why =
            tab["kind"] == "failed" && tab["code"].as_str().is_some_and(|code| !code.is_empty());
        if tab["kind"] != "reopened" && !said_why {
            wrong.push(format!(
                "{name}: a tab neither reopened nor said why: {tab}"
            ));
        }
    }
    if outcome["kind"] == "failed"
        && case.folder == Folder::MovedWithLink
        && !fell_back(row)
        && row["afterMove"]["kind"] != "reopened"
    {
        wrong.push(format!(
            "{name}: following the moved folder does not bring the pane back: {}",
            row["afterMove"]
        ));
    }
    // The bug class this matrix exists for: a chat the host knows, in a
    // folder that is still there, comes back as that chat.
    let known = if case.profile == TerminalLaunchProfileId::Grok && case.origin == Origin::Captured
    {
        "resume the launcher's chat"
    } else {
        "resume the chat"
    };
    if case.origin != Origin::Launcher
        && matches!(case.folder, Folder::Plain | Folder::Linked)
        && outcome["resume"] != known
    {
        wrong.push(format!(
            "{name}: a known chat did not resume as itself: {outcome}"
        ));
    }
    wrong
}

async fn run_matrix(cases: Vec<Case>) {
    let write = std::env::var_os("INTERFACE_RESTORE_MATRIX_WRITE").is_some();
    let expected = if write {
        BTreeMap::new()
    } else {
        fixture_rows()
    };
    let mut rows = Vec::new();
    let mut wrong = Vec::new();
    // Rows are independent temporary hosts; a few at a time keeps a loaded
    // machine's test threads free.
    for batch in cases.chunks(8) {
        let results = futures::future::join_all(batch.iter().map(|&case| run_case(case))).await;
        for (case, row) in batch.iter().zip(results) {
            wrong.extend(blank_or_lost(*case, &row));
            if !write {
                let name = case.name();
                match expected.get(&name) {
                    None => wrong.push(format!("{name} is missing from {FIXTURE_PATH}")),
                    Some(pinned) if pinned != &row => wrong.push(format!(
                        "{name} changed; if that is deliberate, rewrite the fixture (see the module docs)\n  now:    {row}\n  pinned: {pinned}"
                    )),
                    Some(_) => {}
                }
            }
            rows.push(row);
        }
    }
    if write {
        // One row per line keeps a deliberate change's diff readable.
        let lines = rows
            .iter()
            .map(|row| format!("    {}", serde_json::to_string(row).expect("row")))
            .collect::<Vec<_>>()
            .join(",\n");
        fs::write(
            FIXTURE_PATH,
            format!(
                "{{\n  \"version\": 1,\n  \"about\": \"Cold-start restore matrix. apps/interface-host/src/terminal_restore_matrix_tests.rs writes it (INTERFACE_RESTORE_MATRIX_WRITE=1); electron/src/renderer/terminal-restore-matrix.test.ts replays it.\",\n  \"rows\": [\n{lines}\n  ]\n}}\n"
            ),
        )
        .expect("write the restore matrix fixture");
    }
    assert!(
        wrong.is_empty(),
        "{} restore matrix rows are wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_matrix_every_saved_pane_reopens_or_says_why_never_blank() {
    let full = std::env::var("INTERFACE_RESTORE_MATRIX").is_ok_and(|mode| mode == "full")
        || std::env::var_os("INTERFACE_RESTORE_MATRIX_WRITE").is_some();
    let mut cases = if full { all_cases() } else { gate_cases() };
    if let Ok(only) = std::env::var("INTERFACE_RESTORE_MATRIX_ONLY") {
        cases.retain(|case| case.name().contains(&only));
    }
    if !full && std::env::var_os("INTERFACE_RESTORE_MATRIX_ONLY").is_none() {
        assert_eq!(
            cases.len(),
            72,
            "one gate row per profile, origin and folder"
        );
        assert_eq!(all_cases().len(), fixture_rows().len());
    }
    run_matrix(cases).await;
}
