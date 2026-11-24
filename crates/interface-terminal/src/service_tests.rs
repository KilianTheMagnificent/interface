use super::provider_recovery::provider_session_should_replace;
use super::*;

#[test]
fn native_startup_replaces_a_stale_binding_with_the_proven_provider_identity() {
    for id in [
        "agy-d2d9dfef-c119-4df6-8827-2bad4e79eda6",
        "muse-01a15aed-3490-7fa0-b92a-ef4fae9c92f7",
    ] {
        let prior = TerminalLaunchSessionV1 {
            version: 1,
            id: Some("stale".into()),
            resume: true,
        };
        let next = TerminalProviderSession {
            id: id.into(),
            transition: TerminalProviderSessionTransition::Startup,
        };
        assert!(provider_session_should_replace(Some(&prior), &next));
    }
}

#[test]
fn provider_owner_gates_serialize_one_generation_without_blocking_another() {
    // Exercise the registry independently of PTY/provider I/O. Re-entrants share
    // exactly one gate until all holders release it; another generation/pane
    // can continue while that owner is blocked in its durable lookup.
    let gates = Mutex::new(BTreeMap::new());
    let id = TerminalId::random();
    let first = owner_gate(&gates, (id, 1));
    let same = owner_gate(&gates, (id, 1));
    let other = owner_gate(&gates, (TerminalId::random(), 1));
    let restarted = owner_gate(&gates, (id, 2));
    let _held = lock(&first);
    assert!(same.try_lock().is_err());
    assert!(other.try_lock().is_ok());
    assert!(restarted.try_lock().is_ok());
}

const CLAUDE_CHAT: &str = "12345678-1234-4234-8234-123456789abc";
const CODEX_THREAD: &str = "01a06e8d-c9ac-78e3-be73-19f2255b8932";

fn resume(id: &str) -> TerminalLaunchSessionV1 {
    TerminalLaunchSessionV1 {
        version: 1,
        id: Some(id.to_owned()),
        resume: true,
    }
}

fn startup(id: &str) -> TerminalProviderSession {
    TerminalProviderSession {
        id: id.to_owned(),
        transition: TerminalProviderSessionTransition::Startup,
    }
}

#[test]
fn armed_shells_are_promoted_to_the_provider_named_by_the_receipt() {
    assert_eq!(
        manual_provider_profile(TerminalLaunchProfileId::Claude, CODEX_THREAD),
        TerminalLaunchProfileId::Codex
    );
    assert_eq!(
        manual_provider_profile(TerminalLaunchProfileId::Claude, CLAUDE_CHAT),
        TerminalLaunchProfileId::Claude
    );
}

#[test]
fn a_codex_startup_replaces_a_stale_resume_binding_but_a_claude_one_does_not() {
    let stale = resume("01a06e8e-0940-72d0-adbc-8aa978ca7eeb");
    assert!(provider_session_should_replace(
        Some(&stale),
        &startup(CODEX_THREAD)
    ));
    assert!(!provider_session_should_replace(
        Some(&resume(CODEX_THREAD)),
        &startup(CODEX_THREAD)
    ));
    assert!(!provider_session_should_replace(
        Some(&resume(CLAUDE_CHAT)),
        &startup(CLAUDE_CHAT)
    ));
    assert!(!provider_session_should_replace(
        Some(&resume(CLAUDE_CHAT)),
        &startup("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")
    ));
}

#[test]
fn opencode_session_ids_are_accepted_and_never_displace_a_resume_binding() {
    let opencode_chat = "ses_f83a11783ffeRtqTE7hTSgvckz";
    assert!(is_provider_session_id(opencode_chat));
    assert!(is_provider_session_id(CLAUDE_CHAT));
    assert!(is_provider_session_id(CODEX_THREAD));
    assert!(!is_provider_session_id("ses_short"));
    assert!(!is_provider_session_id("ses_marked/escape"));
    assert!(!is_provider_session_id(&CODEX_THREAD.replace('7', "9")));

    // The first captured chat is the pane's chat for its whole life: a later
    // top-level creation in the same directory (a TUI `/new`, or a sibling
    // pane's session) never rewrites a durable resume binding.
    let stale = resume("ses_older1111111111111111111");
    assert!(provider_session_should_replace(
        Some(&resume_without_id()),
        &startup(opencode_chat)
    ));
    assert!(!provider_session_should_replace(
        Some(&stale),
        &startup("ses_newer222222222222222222")
    ));
    assert!(!provider_session_should_replace(
        Some(&resume(opencode_chat)),
        &startup(opencode_chat)
    ));
}

fn resume_without_id() -> TerminalLaunchSessionV1 {
    TerminalLaunchSessionV1 {
        version: 1,
        id: None,
        resume: false,
    }
}

#[test]
fn read_batch_merges_only_queued_output_and_drains_before_closing() {
    let buffer = Arc::new(OutputBuffer::new(
        TerminalId::random(),
        TerminalGeneration::initial(),
        30,
    ));
    let reader = buffer.claim().expect("claim output");
    for bytes in [b"aaaaaaaaaa", b"bbbbbbbbbb", b"cccccccccc"] {
        assert!(buffer.push(bytes.to_vec()));
    }
    // Queued chunks merge in order while they fit; the first always goes.
    assert_eq!(
        reader.read_batch(25).as_deref(),
        Some(&b"aaaaaaaaaabbbbbbbbbb"[..])
    );
    assert_eq!(reader.read_batch(5).as_deref(), Some(&b"cccccccccc"[..]));

    // With the buffer full, the writer parks until a batch frees space.
    assert!(buffer.push(vec![b'd'; 30]));
    let writer = {
        let buffer = Arc::clone(&buffer);
        thread::spawn(move || buffer.push(b"eeeeeeeeee".to_vec()))
    };
    assert_eq!(reader.read_batch(100), Some(vec![b'd'; 30]));
    assert!(writer.join().expect("writer thread"));

    // The wait has no timeout: it returns once output arrives.
    let late = {
        let buffer = Arc::clone(&buffer);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            buffer.push(b"ffffffffff".to_vec())
        })
    };
    assert_eq!(reader.read_batch(100).as_deref(), Some(&b"eeeeeeeeee"[..]));
    assert_eq!(reader.read_batch(100).as_deref(), Some(&b"ffffffffff"[..]));
    assert!(late.join().expect("late writer"));

    // Queued bytes are returned before the closed buffer reports its end.
    assert!(buffer.push(b"gg".to_vec()));
    buffer.close();
    assert_eq!(reader.read_batch(100).as_deref(), Some(&b"gg"[..]));
    assert_eq!(reader.read_batch(100), None);
}

/// Create and restart must install the same entry: the same fields, the same
/// provider-capture owner, lease and spawn, and the same ticks armed. These
/// read the private entry, so they live beside the service.
mod install {
    use super::*;
    use crate::launch::TerminalLaunchProgram;
    use interface_contracts::terminal::TerminalExit;
    use std::ffi::OsString;

    #[derive(Default)]
    struct Exit {
        exited: Mutex<bool>,
        changed: Condvar,
    }

    impl Exit {
        fn exit(&self) {
            *lock(&self.exited) = true;
            self.changed.notify_all();
        }
    }

    struct QuietProcess {
        exit: Arc<Exit>,
        identity: TerminalProcessIdentity,
    }

    impl TerminalInput for QuietProcess {
        fn write(&self, _bytes: &[u8]) -> Result<(), TerminalBackendError> {
            Ok(())
        }
        fn close(&self) {}
    }

    impl TerminalProcessControl for QuietProcess {
        fn resize(&self, _size: TerminalSize) -> Result<(), TerminalBackendError> {
            Ok(())
        }
        fn terminate_tree(&self) -> Result<(), TerminalBackendError> {
            self.exit.exit();
            Ok(())
        }
        fn process_id(&self) -> Option<u32> {
            Some(self.identity.process_id)
        }
        fn process_identity(&self) -> Option<TerminalProcessIdentity> {
            Some(self.identity.clone())
        }
    }

    struct QuietWaiter(Arc<Exit>);

    impl crate::embedded::TerminalProcessWaiter for QuietWaiter {
        fn wait(&mut self) -> Result<TerminalExit, TerminalBackendError> {
            let exited = lock(&self.0.exited);
            drop(
                self.0
                    .changed
                    .wait_while(exited, |exited| !*exited)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            Ok(TerminalExit {
                code: Some(0),
                signal: None,
            })
        }
    }

    #[derive(Default)]
    struct QuietBackend {
        specs: Mutex<Vec<TerminalSpawnSpec>>,
        exits: Mutex<Vec<Arc<Exit>>>,
    }

    impl TerminalPtyBackend for QuietBackend {
        fn spawn(&self, spec: &TerminalSpawnSpec) -> Result<SpawnedTerminal, TerminalBackendError> {
            let mut specs = lock(&self.specs);
            specs.push(spec.clone());
            let process_id = 20_000 + u32::try_from(specs.len()).unwrap_or(0);
            let exit = Arc::new(Exit::default());
            lock(&self.exits).push(Arc::clone(&exit));
            let process = Arc::new(QuietProcess {
                exit: Arc::clone(&exit),
                identity: TerminalProcessIdentity {
                    process_id,
                    process_group_id: process_id,
                    terminal_device: None,
                    process_start: format!("start {process_id}"),
                },
            });
            Ok(SpawnedTerminal {
                reader: Box::new(std::io::empty()),
                input: process.clone(),
                control: process,
                waiter: Box::new(QuietWaiter(exit)),
            })
        }
    }

    #[derive(Default)]
    struct Leases(Mutex<Vec<TerminalLeaseRequest>>);
    struct Lease;
    impl TerminalWorkspaceLease for Lease {}
    impl TerminalWorkspaceLeaseProvider for Leases {
        fn acquire(
            &self,
            request: &TerminalLeaseRequest,
        ) -> Result<Box<dyn TerminalWorkspaceLease>, TerminalBackendError> {
            lock(&self.0).push(request.clone());
            Ok(Box::new(Lease))
        }
    }

    #[derive(Default)]
    struct Owners(Mutex<Vec<(TerminalLaunchProfileId, TerminalProviderSessionCaptureOwner)>>);
    impl TerminalProviderSessionLookup for Owners {
        fn begin_capture(
            &self,
            profile_id: TerminalLaunchProfileId,
            owner: &TerminalProviderSessionCaptureOwner,
        ) -> Result<(), TerminalBackendError> {
            lock(&self.0).push((profile_id, owner.clone()));
            Ok(())
        }
        fn current_session(
            &self,
            _profile_id: TerminalLaunchProfileId,
            _process: &TerminalProcessIdentity,
            _capture_token: &str,
        ) -> Result<Option<TerminalProviderSession>, TerminalBackendError> {
            Ok(None)
        }
    }

    struct Quiet;
    impl TerminalLifecycleSink for Quiet {
        fn publish(&self, _event: TerminalLifecycleEvent) {}
    }

    /// Everything an installed entry holds, with the values a new generation
    /// must change (its number, capture token and process) reduced to their
    /// presence.
    #[allow(clippy::struct_excessive_bools)] // One flag per entry field compared.
    #[derive(Debug, PartialEq)]
    struct Installed {
        summary: TerminalSummary,
        fingerprint: CreateFingerprint,
        root: PathBuf,
        cwd: PathBuf,
        title_is_user: bool,
        title_lookup_at: bool,
        file_title_session: Option<String>,
        shell_display: Option<(
            Option<TerminalLaunchRequestV1>,
            String,
            Option<TerminalLaunchProfileId>,
        )>,
        capture_token: bool,
        provider_session_profile: Option<TerminalLaunchProfileId>,
        capture_kind: Option<ProviderSessionCaptureKind>,
        process: bool,
        provider_session_cwd: Option<PathBuf>,
        wake: bool,
        finalizing: bool,
        replacing: bool,
        closing: bool,
        ticks: (bool, bool, bool),
        startup_fetch: bool,
        lease: bool,
    }

    fn installed(service: &TerminalService, id: TerminalId) -> Installed {
        let state = lock(&service.shared.state);
        let entry = state.entries.get(&id).expect("installed entry");
        let mut summary = entry.summary.clone();
        summary.generation = TerminalGeneration::initial();
        summary.created_at = String::new();
        summary.updated_at = String::new();
        Installed {
            summary,
            fingerprint: entry.fingerprint.clone(),
            root: entry.root.clone(),
            cwd: entry.cwd.clone(),
            title_is_user: entry.title_is_user,
            title_lookup_at: entry.title_lookup_at.is_some(),
            file_title_session: entry.file_title_session.clone(),
            shell_display: entry
                .shell_display
                .as_ref()
                .map(|display| (display.launch, display.title.clone(), display.profile)),
            capture_token: entry.provider_session_capture_token.is_some(),
            provider_session_profile: entry.provider_session_profile,
            capture_kind: entry.provider_session_capture_kind.clone(),
            process: entry.provider_session_process.is_some(),
            provider_session_cwd: entry.provider_session_cwd.clone(),
            wake: entry.provider_session_wake.is_some(),
            finalizing: entry.provider_session_finalizing,
            replacing: entry.provider_session_replacing,
            closing: entry.closing,
            ticks: (
                entry.ticks.shell_display,
                entry.ticks.grok_title,
                entry.ticks.startup_download,
            ),
            startup_fetch: entry.startup_fetch.is_some(),
            lease: entry.resources.lease.is_some(),
        }
    }

    fn executable(directory: &tempfile::TempDir, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.path().join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("fake agent");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .expect("fake agent permissions");
        path
    }

    fn catalog(directory: &tempfile::TempDir, shell_capture: bool) -> TerminalLaunchCatalog {
        let claude = TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Claude,
            executable(directory, "claude"),
            vec![OsString::from(format!(
                "capture={}",
                crate::launch::CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER
            ))],
            Vec::new(),
        )
        .expect("Claude program")
        .with_provider_session_capture()
        .expect("Claude capture");
        let grok = TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Grok,
            executable(directory, "grok"),
            Vec::new(),
            Vec::new(),
        )
        .expect("Grok program");
        let catalog = TerminalLaunchCatalog::new([claude, grok]).expect("catalog");
        if !shell_capture {
            return catalog;
        }
        catalog
            .with_shell_provider_session_capture(vec![(
                OsString::from("INTERFACE_TEST_CAPTURE"),
                OsString::from(format!(
                    "capture={}",
                    crate::launch::CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER
                )),
            )])
            .expect("shell capture")
    }

    #[test]
    fn a_sign_in_pane_runs_the_cli_login_and_a_refreshed_catalog_serves_later_launches() {
        let directory = tempfile::Builder::new()
            .prefix("interface-sign-in-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("directory");
        let backend = Arc::new(QuietBackend::default());
        let service = TerminalService::new(TerminalServiceParts {
            launches: TerminalLaunchCatalog::shell_only(),
            ..TerminalServiceParts::new(
                TerminalServiceConfig::default(),
                backend.clone(),
                Arc::new(Leases::default()),
                Arc::new(Quiet),
            )
        })
        .expect("service");
        let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
        let claude_available = |service: &TerminalService| {
            service
                .launch_profiles()
                .iter()
                .any(|profile| profile.id == TerminalLaunchProfileId::Claude && profile.available)
        };
        let sign_in = |profile_id| CreateTerminalRequest {
            id: TerminalId::random(),
            cwd: directory.path().to_string_lossy().into_owned(),
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
            title: Some("Sign in".to_owned()),
            session_key: None,
            launch: Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id,
            }),
            launch_session: None,
            handoff: None,
            sign_in: true,
            account: None,
        };
        // Claude was not installed when Interface started.
        assert!(!claude_available(&service));
        assert!(matches!(
            service.create(sign_in(TerminalLaunchProfileId::Claude), &approved),
            Err(TerminalError::UnavailableLaunchProfile(
                TerminalLaunchProfileId::Claude
            ))
        ));
        // Installed meanwhile: Refresh swaps in a catalog that has it.
        service.replace_launch_catalog(catalog(&directory, false));
        assert!(claude_available(&service));
        let created = service
            .create(sign_in(TerminalLaunchProfileId::Claude), &approved)
            .expect("sign-in pane");
        assert_eq!(created.launch_session, None, "a sign-in captures no chat");
        let spec = lock(&backend.specs).last().cloned().expect("spawned");
        let ResolvedTerminalLaunch::Program {
            executable,
            arguments,
            ..
        } = spec.launch
        else {
            panic!("a sign-in runs the CLI itself");
        };
        assert_eq!(executable.file_name(), Some(std::ffi::OsStr::new("claude")));
        // Claude's own login, never the chat argv (or its capture token).
        assert_eq!(arguments, [OsString::from("auth"), OsString::from("login")]);
        assert!(matches!(
            service.create(sign_in(TerminalLaunchProfileId::Shell), &approved),
            Err(TerminalError::NoSignIn(TerminalLaunchProfileId::Shell))
        ));
    }

    /// The spawned launch with each generation's capture token blanked out.
    fn launch_without_token(spec: &TerminalSpawnSpec, token: Option<&str>) -> String {
        let launch = format!("{:?}", spec.launch);
        token.map_or(launch.clone(), |token| launch.replace(token, "<token>"))
    }

    fn await_exited(service: &TerminalService, id: TerminalId) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while lock(&service.shared.state)
            .entries
            .get(&id)
            .is_some_and(TerminalEntry::is_live)
        {
            assert!(Instant::now() < deadline, "the generation did not settle");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// What the create armed: (shell display, Grok title) ticks, and the
    /// capture's (token and wake, attached process).
    struct Armed {
        ticks: (bool, bool),
        capture: (bool, bool),
    }

    #[allow(clippy::too_many_lines)] // One create, exit and restart, compared field by field.
    fn assert_restart_installs_what_create_did(
        shell_capture: bool,
        launch: Option<TerminalLaunchProfileId>,
        launch_session: Option<&TerminalLaunchSessionV1>,
        armed: &Armed,
    ) {
        // Approved roots refuse the shared temp directory.
        let directory = tempfile::Builder::new()
            .prefix("interface-install-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("directory");
        let backend = Arc::new(QuietBackend::default());
        let leases = Arc::new(Leases::default());
        let owners = Arc::new(Owners::default());
        let service = TerminalService::new(TerminalServiceParts {
            launches: catalog(&directory, shell_capture),
            provider_sessions: owners.clone(),
            ..TerminalServiceParts::new(
                TerminalServiceConfig::default(),
                backend.clone(),
                leases.clone(),
                Arc::new(Quiet),
            )
        })
        .expect("service");
        // Hold the ticker off so nothing but the install writes the entry.
        lock(&service.shared.state).ticker = true;
        let approved = ApprovedTerminalRoots::new([directory.path().to_path_buf()]).expect("roots");
        let id = TerminalId::random();
        let created = service
            .create(
                CreateTerminalRequest {
                    sign_in: false,
                    id,
                    cwd: directory.path().to_string_lossy().into_owned(),
                    size: TerminalSize {
                        columns: 80,
                        rows: 24,
                    },
                    title: Some("Pane".to_owned()),
                    session_key: Some("codex#install".to_owned()),
                    launch: launch.map(|profile_id| TerminalLaunchRequestV1 {
                        version: 1,
                        profile_id,
                    }),
                    launch_session: launch_session.cloned(),
                    handoff: None,
                    account: None,
                },
                &approved,
            )
            .expect("create");
        let mut expected = installed(&service, id);
        assert_eq!(
            expected.ticks,
            (armed.ticks.0, armed.ticks.1, true),
            "ticks armed at create"
        );
        assert_eq!(
            (expected.capture_token, expected.wake, expected.process),
            (armed.capture.0, armed.capture.0, armed.capture.1),
            "capture armed at create"
        );
        assert!(expected.startup_fetch && expected.lease);
        let token = lock(&service.shared.state).entries[&id]
            .provider_session_capture_token
            .clone();

        lock(&backend.exits)[0].exit();
        await_exited(&service, id);
        let restarted = service
            .restart(
                TerminalGenerationRequest {
                    id,
                    generation: created.generation,
                },
                &approved,
            )
            .expect("restart");
        assert_eq!(restarted.generation, created.generation.next());
        let actual = installed(&service, id);
        let restart_token = lock(&service.shared.state).entries[&id]
            .provider_session_capture_token
            .clone();

        // A restart is a reopen: the create-time binding comes back as resume.
        expected.summary.launch_session = reopened(expected.summary.launch_session.as_ref());
        assert_eq!(actual, expected);
        assert!(
            token.is_none() || token != restart_token,
            "a restart rotates the capture token"
        );

        let owners = lock(&owners.0);
        assert_eq!(owners.len(), usize::from(token.is_some()) * 2);
        if let [(first_profile, first), (second_profile, second)] = owners.as_slice() {
            assert_eq!(first_profile, second_profile);
            assert_eq!(first.terminal_id, second.terminal_id);
            assert_eq!(first.terminal_generation, created.generation);
            assert_eq!(second.terminal_generation, restarted.generation);
            assert_eq!(
                (&first.root, &first.cwd, first.nested_shell),
                (&second.root, &second.cwd, second.nested_shell)
            );
            assert_eq!(
                reopened(first.prior_launch_session.as_ref()),
                second.prior_launch_session
            );
        }
        let leases = lock(&leases.0);
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[1].generation, restarted.generation);
        assert_eq!(
            (
                &leases[0].id,
                &leases[0].root,
                &leases[0].cwd,
                &leases[0].session_key
            ),
            (
                &leases[1].id,
                &leases[1].root,
                &leases[1].cwd,
                &leases[1].session_key
            )
        );
        let specs = lock(&backend.specs);
        assert_eq!(
            (&specs[0].cwd, specs[0].size),
            (&specs[1].cwd, specs[1].size)
        );
        if launch != Some(TerminalLaunchProfileId::Grok) {
            assert_eq!(
                launch_without_token(&specs[0], token.as_deref()),
                launch_without_token(&specs[1], restart_token.as_deref())
            );
        }
    }

    #[test]
    fn a_restarted_shell_is_installed_as_it_was_created() {
        assert_restart_installs_what_create_did(
            false,
            Some(TerminalLaunchProfileId::Shell),
            None,
            &Armed {
                ticks: (true, false),
                capture: (false, false),
            },
        );
    }

    #[test]
    fn a_restarted_armed_login_shell_is_installed_as_it_was_created() {
        assert_restart_installs_what_create_did(
            true,
            None,
            None,
            &Armed {
                ticks: (true, false),
                capture: (true, false),
            },
        );
    }

    #[test]
    fn a_restarted_claude_capture_is_installed_as_it_was_created() {
        assert_restart_installs_what_create_did(
            true,
            Some(TerminalLaunchProfileId::Claude),
            Some(&resume(CLAUDE_CHAT)),
            &Armed {
                ticks: (false, false),
                capture: (true, true),
            },
        );
    }

    #[test]
    fn a_restarted_grok_pane_keeps_its_title_refresh() {
        assert_restart_installs_what_create_did(
            false,
            Some(TerminalLaunchProfileId::Grok),
            Some(&TerminalLaunchSessionV1 {
                version: 1,
                id: Some(CLAUDE_CHAT.to_owned()),
                resume: false,
            }),
            &Armed {
                ticks: (false, true),
                capture: (false, false),
            },
        );
    }

    #[test]
    fn delegate_directory_authorizes_against_the_calling_panes_own_root() {
        struct SelfPidBackend;
        impl TerminalPtyBackend for SelfPidBackend {
            fn spawn(
                &self,
                _spec: &TerminalSpawnSpec,
            ) -> Result<SpawnedTerminal, TerminalBackendError> {
                let exit = Arc::new(Exit::default());
                let pid = std::process::id();
                let process = Arc::new(QuietProcess {
                    exit: Arc::clone(&exit),
                    identity: TerminalProcessIdentity {
                        process_id: pid,
                        process_group_id: pid,
                        terminal_device: None,
                        process_start: format!("start {pid}"),
                    },
                });
                Ok(SpawnedTerminal {
                    reader: Box::new(std::io::empty()),
                    input: process.clone(),
                    control: process,
                    waiter: Box::new(QuietWaiter(exit)),
                })
            }
        }

        let pane_root = tempfile::Builder::new()
            .prefix("interface-delegate-pane-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("pane_root");
        let other_root = tempfile::Builder::new()
            .prefix("interface-delegate-other-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .expect("other_root");
        let nested = pane_root.path().join("sub");
        std::fs::create_dir_all(&nested).unwrap();

        let service = TerminalService::new(TerminalServiceParts {
            launches: catalog(&pane_root, false),
            ..TerminalServiceParts::new(
                TerminalServiceConfig::default(),
                Arc::new(SelfPidBackend),
                Arc::new(Leases::default()),
                Arc::new(Quiet),
            )
        })
        .expect("service");
        lock(&service.shared.state).ticker = true;
        let approved = ApprovedTerminalRoots::new([
            pane_root.path().to_path_buf(),
            other_root.path().to_path_buf(),
        ])
        .expect("approved");
        let id = TerminalId::random();
        let created = service
            .create(
                CreateTerminalRequest {
                    sign_in: false,
                    id,
                    cwd: pane_root.path().to_string_lossy().into_owned(),
                    size: TerminalSize {
                        columns: 80,
                        rows: 24,
                    },
                    title: Some("Pane".to_owned()),
                    session_key: None,
                    launch: None,
                    launch_session: None,
                    handoff: None,
                    account: None,
                },
                &approved,
            )
            .expect("create");
        let request = TerminalGenerationRequest {
            id,
            generation: created.generation,
        };

        assert_eq!(
            service
                .delegate_directory(request, std::process::id(), &approved, Some(&nested))
                .expect("subdirectory inside own root"),
            nested
        );
        assert!(
            service
                .delegate_directory(
                    request,
                    std::process::id(),
                    &approved,
                    Some(other_root.path())
                )
                .is_err(),
            "a pane must not delegate into a different approved workspace root"
        );
    }
}
