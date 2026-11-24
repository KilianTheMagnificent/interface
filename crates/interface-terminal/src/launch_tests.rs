use std::{collections::BTreeSet, fs};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use tempfile::TempDir;

use super::*;

fn executable(directory: &TempDir, name: &str) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("fake executable");
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("permissions");
    path
}

fn managed_session_catalog(directory: &TempDir) -> TerminalLaunchCatalog {
    TerminalLaunchCatalog::new([
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Claude,
            executable(directory, "claude"),
            Vec::new(),
            Vec::new(),
        )
        .expect("claude program"),
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Codex,
            executable(directory, "codex"),
            Vec::new(),
            Vec::new(),
        )
        .expect("codex program"),
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Grok,
            executable(directory, "grok"),
            Vec::new(),
            Vec::new(),
        )
        .expect("grok program"),
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Gemini,
            executable(directory, "agy"),
            Vec::new(),
            Vec::new(),
        )
        .expect("antigravity program"),
    ])
    .expect("catalog")
}

fn bound_session(id: Option<&str>, resume: bool) -> TerminalLaunchSessionV1 {
    TerminalLaunchSessionV1 {
        version: 1,
        id: id.map(str::to_owned),
        resume,
    }
}

fn resolved_session_arguments(
    catalog: &TerminalLaunchCatalog,
    profile_id: TerminalLaunchProfileId,
    session: &TerminalLaunchSessionV1,
    codex_cwd_shared: bool,
) -> Result<Vec<OsString>, TerminalError> {
    match catalog.resolve_with_peers(
        Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id,
        }),
        Some(session),
        codex_cwd_shared,
        None,
    ) {
        Ok(ResolvedTerminalLaunch::Program { arguments, .. }) => Ok(arguments),
        Ok(
            ResolvedTerminalLaunch::LoginShell
            | ResolvedTerminalLaunch::LoginShellWithEnvironment { .. },
        ) => panic!("a profile must not fall back"),
        Err(error) => Err(error),
    }
}

#[test]
fn profiles_are_fixed_ordered_and_hide_process_material() {
    let directory = TempDir::new().expect("temporary directory");
    let codex = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        executable(&directory, "codex"),
        vec!["--fixed".into()],
        vec![("PATH".into(), "/fixed/bin".into())],
    )
    .expect("program");
    let catalog = TerminalLaunchCatalog::new([codex]).expect("catalog");
    let profiles = catalog.profiles();
    assert_eq!(profiles.len(), 9);
    assert_eq!(profiles[0].id, TerminalLaunchProfileId::Shell);
    assert_eq!(profiles[0].label, "Terminal");
    assert!(profiles[0].available);
    assert_eq!(profiles[1].label, "Claude Code");
    assert_eq!(profiles[2].id, TerminalLaunchProfileId::Codex);
    assert!(profiles[2].available);
    assert_eq!(profiles[4].id, TerminalLaunchProfileId::Gemini);
    assert_eq!(profiles[4].label, "Antigravity");
    assert_eq!(profiles[5].id, TerminalLaunchProfileId::Omp);
    assert_eq!(profiles[5].label, "OMP");
    assert!(!profiles[5].available);
    assert_eq!(profiles[6].id, TerminalLaunchProfileId::Opencode);
    assert_eq!(profiles[6].label, "OpenCode");
    assert!(!profiles[6].available);
    assert_eq!(profiles[7].id, TerminalLaunchProfileId::Muse);
    assert_eq!(profiles[7].label, "Muse Code");
    assert!(!profiles[7].available);
    assert_eq!(profiles[8].id, TerminalLaunchProfileId::Kimi);
    assert_eq!(profiles[8].label, "Kimi Code");
    assert!(!profiles[8].available);
    assert!(!profiles[4].available);
    assert_eq!(
        profiles[4].unavailable_reason.as_deref(),
        Some("Antigravity CLI was not found in the login shell or a supported install location.")
    );
    assert!(
        profiles
            .iter()
            .all(|profile| profile.id != TerminalLaunchProfileId::Qwen)
    );
    assert!(
        profiles
            .iter()
            .all(|profile| !profile.label.contains("/fixed/bin"))
    );
    assert!(profiles.iter().all(|profile| {
        profile
            .unavailable_reason
            .as_deref()
            .is_none_or(|reason| !reason.contains("--fixed"))
    }));
}

#[test]
#[cfg(unix)]
fn updated_launcher_targets_are_revalidated_without_stale_fallback() {
    use std::os::unix::fs::symlink;

    let directory = TempDir::new().expect("temporary directory");
    let old = executable(&directory, "old-codex");
    let launcher = directory.path().join("codex");
    symlink(&old, &launcher).expect("initial launcher");
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        &launcher,
        Vec::new(),
        Vec::new(),
    )
    .expect("validated launcher");
    let catalog = TerminalLaunchCatalog::new([program]).expect("catalog");
    let non_executable = executable(&directory, "non-executable");
    fs::set_permissions(&non_executable, fs::Permissions::from_mode(0o600))
        .expect("remove executable permission");
    for target in [
        directory.path().join("missing"),
        directory.path().to_path_buf(),
        non_executable,
        launcher.clone(),
    ] {
        let replacement = directory.path().join("next");
        symlink(target, &replacement).expect("replacement link");
        fs::rename(replacement, &launcher).expect("replace launcher");
        assert_eq!(
            catalog.resolve_with_peers(
                Some(TerminalLaunchRequestV1 {
                    version: 1,
                    profile_id: TerminalLaunchProfileId::Codex,
                }),
                None,
                false,
                None,
            ),
            Err(TerminalError::UnavailableLaunchProfile(
                TerminalLaunchProfileId::Codex
            )),
            "an invalid update must not launch the retained old release"
        );
        assert!(old.is_file());
    }
}

#[test]
fn resolution_accepts_only_versioned_available_profiles() {
    let directory = TempDir::new().expect("temporary directory");
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Claude,
        executable(&directory, "claude"),
        Vec::new(),
        Vec::new(),
    )
    .expect("program");
    let expected = ResolvedTerminalLaunch::try_from(&program).expect("valid launch");
    let catalog = TerminalLaunchCatalog::new([program]).expect("catalog");
    assert_eq!(
        catalog.resolve_with_peers(None, None, false, None),
        Ok(ResolvedTerminalLaunch::LoginShell)
    );
    assert_eq!(
        catalog.resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            None,
            false,
            None,
        ),
        Ok(expected),
        "an unbound pane stays the curated program with no chat argv"
    );
    assert_eq!(
        catalog.resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 2,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            None,
            false,
            None,
        ),
        Err(TerminalError::UnsupportedLaunchVersion(2))
    );
    assert_eq!(
        catalog.resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Gemini,
            }),
            None,
            false,
            None,
        ),
        Err(TerminalError::UnavailableLaunchProfile(
            TerminalLaunchProfileId::Gemini
        ))
    );
}

#[test]
fn claude_and_grok_chat_bindings_never_request_worktrees() {
    let directory = TempDir::new().expect("temporary directory");
    let catalog = managed_session_catalog(&directory);

    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Claude,
            &bound_session(Some("abc-123"), false),
            false,
        ),
        Ok(vec![OsString::from("--session-id=abc-123")]),
        "a fresh Claude pane must stay in the checkout Interface selected"
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Grok,
            &bound_session(Some("abc-123"), false),
            false,
        ),
        Ok(vec![OsString::from("--session-id=abc-123")]),
        "a fresh Grok pane must stay in the checkout Interface selected"
    );
    let resolved = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            Some(&bound_session(Some("abc-123"), false)),
            false,
            None,
        )
        .expect("claude start binding");
    let ResolvedTerminalLaunch::Program { environment, .. } = resolved else {
        panic!("claude must resolve to a program");
    };
    assert!(
        environment.is_empty(),
        "chat bindings must not inject retired Project Agent session env"
    );
}

#[test]
fn codex_chat_binding_uses_managed_metadata_and_cwd_scoped_last() {
    let directory = TempDir::new().expect("temporary directory");
    let catalog = managed_session_catalog(&directory);

    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(None, false),
            false,
        ),
        Ok(Vec::new()),
        "the Codex CLI has no worktree flag to be given"
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(None, true),
            false,
        ),
        Ok(vec![OsString::from("resume"), OsString::from("--last")])
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(Some("abc-123"), true),
            false,
        ),
        Ok(vec![OsString::from("resume"), OsString::from("--last")])
    );
}

#[test]
fn codex_reopen_drops_last_when_a_live_pane_shares_the_cwd() {
    let directory = TempDir::new().expect("temporary directory");
    let catalog = managed_session_catalog(&directory);

    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(None, true),
            false,
        ),
        Ok(vec![OsString::from("resume"), OsString::from("--last")])
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(None, true),
            true,
        ),
        Ok(vec![OsString::from("resume")])
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(None, false),
            true,
        ),
        Ok(Vec::new())
    );

    // The CLIs that accept an exact id are already collision-free, so a
    // shared cwd must not change what they are asked to open.
    for profile_id in [
        TerminalLaunchProfileId::Claude,
        TerminalLaunchProfileId::Grok,
    ] {
        assert_eq!(
            resolved_session_arguments(
                &catalog,
                profile_id,
                &bound_session(Some("abc-123"), true),
                true,
            ),
            Ok(vec![OsString::from("--resume=abc-123")]),
        );
        assert_eq!(
            resolved_session_arguments(
                &catalog,
                profile_id,
                &bound_session(Some("abc-123"), false),
                true,
            ),
            Ok(vec![OsString::from("--session-id=abc-123")]),
        );
    }
}

#[test]
fn codex_reopens_a_captured_thread_exactly() {
    let directory = TempDir::new().expect("temporary directory");
    let catalog = managed_session_catalog(&directory);
    // The hook-reported thread id is UUID v7; it reopens exactly whether
    // or not another Codex pane shares the cwd.
    let thread = "01a06978-1e6e-7911-bec8-2df09b45b2b6";
    for codex_cwd_shared in [false, true] {
        assert_eq!(
            resolved_session_arguments(
                &catalog,
                TerminalLaunchProfileId::Codex,
                &bound_session(Some(thread), true),
                codex_cwd_shared,
            ),
            Ok(vec![OsString::from("resume"), OsString::from(thread)])
        );
    }
    // A pre-capture renderer handle is UUID v4 and names no Codex thread.
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(Some("11111111-1111-4111-8111-111111111111"), true),
            false,
        ),
        Ok(vec![OsString::from("resume"), OsString::from("--last")])
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Codex,
            &bound_session(Some(thread), false),
            false,
        ),
        Ok(Vec::new())
    );
}

#[test]
fn codex_reopen_opens_fresh_when_the_rollout_is_gone() {
    let directory = TempDir::new().expect("temporary directory");
    let sessions = directory.path().join("sessions");
    let catalog = managed_session_catalog(&directory).with_codex_sessions_root(sessions.clone());
    // 2026-09-03T23:30:00Z in the v7 timestamp; the day folder is local.
    let thread = "01a0699b-98c0-7abc-8def-0123456789ab";
    let resolve = |session: &TerminalLaunchSessionV1, shared: bool| {
        resolved_session_arguments(&catalog, TerminalLaunchProfileId::Codex, session, shared)
    };
    assert_eq!(
        resolve(&bound_session(Some(thread), true), false),
        Ok(Vec::new())
    );
    let day = sessions.join("2026/09/04");
    std::fs::create_dir_all(&day).expect("day folder");
    std::fs::write(
        day.join(format!("rollout-2026-09-03T19-30-00-{thread}.jsonl")),
        "{}\n",
    )
    .expect("rollout");
    assert_eq!(
        resolve(&bound_session(Some(thread), true), true),
        Ok(vec![OsString::from("resume"), OsString::from(thread)])
    );
    std::fs::remove_dir_all(&day).expect("remove day");
    assert_eq!(
        resolve(&bound_session(Some(thread), true), true),
        Ok(Vec::new())
    );
    // A legacy renderer handle still takes the folder's newest thread.
    assert_eq!(
        resolve(
            &bound_session(Some("11111111-1111-4111-8111-111111111111"), true),
            false,
        ),
        Ok(vec![OsString::from("resume"), OsString::from("--last")])
    );
}

#[test]
fn grok_reopen_resumes_only_a_session_grok_already_holds() {
    let directory = TempDir::new().expect("temporary directory");
    let sessions = directory.path().join("sessions");
    let catalog = managed_session_catalog(&directory).with_grok_sessions_root(sessions.clone());
    let id = "11111111-1111-4111-8111-111111111111";
    let resolve = |session: &TerminalLaunchSessionV1| {
        resolved_session_arguments(&catalog, TerminalLaunchProfileId::Grok, session, false)
    };
    let new_chat = Ok(vec![OsString::from(format!("--session-id={id}"))]);
    let resumed = Ok(vec![OsString::from(format!("--resume={id}"))]);
    // Nothing in Grok's store yet: the pane opens a new chat under its minted
    // id even when the saved binding asked to resume.
    assert_eq!(resolve(&bound_session(Some(id), true)), new_chat);
    assert_eq!(resolve(&bound_session(Some(id), false)), new_chat);
    // Grok holds the chat now. The saved flag stays `resume:false` for life
    // (no hook advances a Grok binding), and `--session-id` on an id Grok
    // already holds fails with "already in use", so the store decides.
    let session_dir = sessions.join("%2FUsers%2Fsomeone%2Fproject").join(id);
    std::fs::create_dir_all(&session_dir).expect("session dir");
    assert_eq!(resolve(&bound_session(Some(id), false)), resumed);
    assert_eq!(resolve(&bound_session(Some(id), true)), resumed);
    // A stray file named after the id, or the id at the wrong depth, is not
    // a session.
    std::fs::remove_dir_all(&session_dir).expect("remove session");
    std::fs::write(sessions.join(id), "").expect("stray file");
    std::fs::create_dir_all(sessions.join("%2Fother").join(id).join("nested")).expect("nested");
    assert_eq!(resolve(&bound_session(Some(id), true)), resumed);
    std::fs::remove_dir_all(sessions.join("%2Fother")).expect("remove other");
    assert_eq!(resolve(&bound_session(Some(id), true)), new_chat);
}

#[test]
fn codex_capture_token_rides_the_environment_not_argv() {
    let directory = TempDir::new().expect("temporary directory");
    let codex = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        executable(&directory, "codex"),
        vec!["-c".into(), "features.hooks=true".into()],
        Vec::new(),
    )
    .expect("program")
    .with_provider_session_capture()
    .expect("capturing Codex program");
    let catalog = TerminalLaunchCatalog::new([codex]).expect("catalog");
    let request = TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Codex,
    };
    let token = "22222222-2222-4222-8222-222222222222";
    let Ok(ResolvedTerminalLaunch::Program {
        arguments,
        environment,
        ..
    }) = catalog.resolve_with_peers(
        Some(request),
        Some(&bound_session(None, false)),
        false,
        Some(token),
    )
    else {
        panic!("a capturing Codex launch resolves to a program");
    };
    assert_eq!(
        arguments,
        vec![OsString::from("-c"), OsString::from("features.hooks=true")]
    );
    assert!(environment.contains(&(
        OsString::from(CODEX_SESSION_CAPTURE_TOKEN_ENV),
        OsString::from(token)
    )));
    assert!(
        catalog
            .resolve_with_peers(Some(request), None, false, None)
            .is_err(),
        "a capturing profile must not spawn without its token"
    );
    assert_eq!(
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Grok,
            executable(&directory, "grok"),
            Vec::new(),
            Vec::new(),
        )
        .expect("grok program")
        .with_provider_session_capture()
        .expect_err("only Claude and Codex capture"),
        TerminalLaunchProgramError::UnsupportedSessionCaptureProfile
    );
}

#[test]
fn antigravity_rejects_obsolete_gemini_chat_bindings() {
    let directory = TempDir::new().expect("temporary directory");
    let catalog = managed_session_catalog(&directory);

    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Gemini,
            &bound_session(Some("abc-123"), false),
            false,
        ),
        Err(TerminalError::UnbindableLaunchProfile(
            TerminalLaunchProfileId::Gemini
        )),
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Gemini,
            &bound_session(Some("abc-123"), true),
            false,
        ),
        Err(TerminalError::UnbindableLaunchProfile(
            TerminalLaunchProfileId::Gemini
        )),
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Gemini,
            &bound_session(None, true),
            false,
        ),
        Err(TerminalError::UnbindableLaunchProfile(
            TerminalLaunchProfileId::Gemini
        ))
    );
}

#[test]
fn invalid_chat_bindings_fail_closed() {
    let directory = TempDir::new().expect("temporary directory");
    let catalog = managed_session_catalog(&directory);

    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Claude,
            &bound_session(Some("--dangerous ok"), true),
            false,
        ),
        Err(TerminalError::InvalidLaunchSessionId)
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Claude,
            &bound_session(None, true),
            false,
        ),
        Ok(vec![OsString::from("--resume")])
    );
    // The host's own shape for a fresh Claude pane whose receipt never
    // arrived (`public_provider_session_state`): a cold reopen must open the
    // picker, not fail the pane on every restart.
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Claude,
            &bound_session(None, false),
            false,
        ),
        Ok(vec![OsString::from("--resume")])
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Claude,
            &TerminalLaunchSessionV1 {
                version: 2,
                id: Some("abc-123".into()),
                resume: true,
            },
            false,
        ),
        Err(TerminalError::UnsupportedLaunchSessionVersion(2))
    );
    assert_eq!(
        catalog.resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Shell,
            }),
            Some(&bound_session(Some("abc-123"), true)),
            false,
            None,
        ),
        Err(TerminalError::UnbindableLaunchProfile(
            TerminalLaunchProfileId::Shell
        ))
    );
}

#[test]
fn pane_environment_rides_every_launch_including_login_shells() {
    let directory = TempDir::new().expect("temporary directory");
    let socket = (
        OsString::from("INTERFACE_PANE_MCP_SOCKET"),
        OsString::from("/tmp/iface.sock"),
    );
    let catalog = TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Grok,
        executable(&directory, "grok"),
        vec![OsString::from("--no-leader")],
        vec![(OsString::from("PATH"), OsString::from("/usr/bin"))],
    )
    .expect("grok program")])
    .expect("catalog")
    .with_pane_environment(vec![socket.clone()])
    .expect("pane environment");

    let shell = catalog
        .resolve_with_peers(None, None, false, None)
        .expect("login shell");
    assert_eq!(
        shell,
        ResolvedTerminalLaunch::LoginShellWithEnvironment {
            environment: vec![socket.clone()],
        }
    );
    let program = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: TERMINAL_LAUNCH_REQUEST_VERSION,
                profile_id: TerminalLaunchProfileId::Grok,
            }),
            None,
            false,
            None,
        )
        .expect("grok launch");
    let ResolvedTerminalLaunch::Program {
        arguments,
        environment,
        ..
    } = program
    else {
        panic!("a profile launch is a program");
    };
    assert_eq!(arguments, vec![OsString::from("--no-leader")]);
    // The profile's own entries come first; the pane entry is appended last so
    // nothing in the catalog can shadow it.
    assert_eq!(
        environment,
        vec![
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            socket.clone(),
        ]
    );
    assert_eq!(
        TerminalLaunchCatalog::shell_only()
            .resolve_with_peers(None, None, false, None)
            .expect("bare shell"),
        ResolvedTerminalLaunch::LoginShell
    );
    assert_eq!(
        TerminalLaunchCatalog::shell_only()
            .with_pane_environment(vec![(OsString::from("BAD=NAME"), OsString::from("x"))])
            .err(),
        Some(TerminalLaunchCatalogError::InvalidPaneEnvironment)
    );
}

#[test]
fn handoff_prompt_lands_as_one_trailing_positional_argument() {
    for profile in [
        TerminalLaunchProfileId::Claude,
        TerminalLaunchProfileId::Codex,
        TerminalLaunchProfileId::Grok,
    ] {
        let mut launch = ResolvedTerminalLaunch::Program {
            executable: PathBuf::from("/bin/echo"),
            arguments: vec![OsString::from("--session-id=abc")],
            environment: Vec::new(),
        };
        bind_handoff_prompt(&mut launch, profile, "Read /tmp/h.md").expect("bind");
        let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
            unreachable!()
        };
        assert_eq!(
            arguments,
            vec![
                OsString::from("--session-id=abc"),
                OsString::from("Read /tmp/h.md"),
            ]
        );
    }

    let mut antigravity = ResolvedTerminalLaunch::Program {
        executable: PathBuf::from("/bin/echo"),
        arguments: vec![OsString::from("--fixed")],
        environment: Vec::new(),
    };
    bind_handoff_prompt(
        &mut antigravity,
        TerminalLaunchProfileId::Gemini,
        "Read /tmp/h.md",
    )
    .expect("bind Antigravity prompt");
    let ResolvedTerminalLaunch::Program { arguments, .. } = antigravity else {
        unreachable!()
    };
    assert_eq!(
        arguments,
        vec![
            OsString::from("--fixed"),
            OsString::from("--prompt-interactive"),
            OsString::from("Read /tmp/h.md"),
        ]
    );
}

#[test]
fn handoff_prompt_refuses_shell_qwen_and_oversized_prompts() {
    let mut shell = ResolvedTerminalLaunch::LoginShell;
    assert_eq!(
        bind_handoff_prompt(&mut shell, TerminalLaunchProfileId::Shell, "x"),
        Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Shell
        ))
    );
    let mut program = ResolvedTerminalLaunch::Program {
        executable: PathBuf::from("/bin/echo"),
        arguments: Vec::new(),
        environment: Vec::new(),
    };
    assert_eq!(
        bind_handoff_prompt(&mut program, TerminalLaunchProfileId::Qwen, "x"),
        Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Qwen
        ))
    );
    let oversized = "p".repeat(4_097);
    assert_eq!(
        bind_handoff_prompt(&mut program, TerminalLaunchProfileId::Claude, &oversized),
        Err(TerminalError::InvalidHandoff)
    );
    assert_eq!(
        bind_handoff_prompt(&mut program, TerminalLaunchProfileId::Claude, ""),
        Err(TerminalError::InvalidHandoff)
    );
    assert_eq!(
        bind_handoff_prompt(&mut program, TerminalLaunchProfileId::Claude, "a\0b"),
        Err(TerminalError::InvalidHandoff)
    );
    let ResolvedTerminalLaunch::Program { arguments, .. } = program else {
        unreachable!()
    };
    assert!(
        arguments.is_empty(),
        "a refused pass-off must not touch argv"
    );
}

#[test]
fn invalid_or_duplicate_programs_fail_closed() {
    let directory = TempDir::new().expect("temporary directory");
    assert_eq!(
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Shell,
            executable(&directory, "shell"),
            Vec::new(),
            Vec::new(),
        ),
        Err(TerminalLaunchProgramError::ShellProfile)
    );
    assert_eq!(
        TerminalLaunchProgram::new(
            TerminalLaunchProfileId::Codex,
            directory.path().join("missing"),
            Vec::new(),
            Vec::new(),
        ),
        Err(TerminalLaunchProgramError::UnsafeExecutable)
    );
    let one = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Codex,
        executable(&directory, "codex"),
        Vec::new(),
        Vec::new(),
    )
    .expect("first");
    assert_eq!(
        TerminalLaunchCatalog::new([one.clone(), one]),
        Err(TerminalLaunchCatalogError::DuplicateProfile)
    );
    assert_eq!(
        PROFILE_ORDER.into_iter().collect::<BTreeSet<_>>().len(),
        PROFILE_ORDER.len()
    );
}

#[test]
fn omp_launches_directly_and_only_accepts_an_initial_prompt_binding() {
    let directory = TempDir::new().unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Omp,
        executable(&directory, "omp"),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let catalog = TerminalLaunchCatalog::new([program]).unwrap();
    let request = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Omp,
    });
    let mut launch = catalog
        .resolve_with_peers(request, None, false, None)
        .unwrap();
    let ResolvedTerminalLaunch::Program { arguments, .. } = &launch else {
        panic!("OMP must not fall back");
    };
    assert!(arguments.is_empty());
    assert!(!catalog.captures_provider_session(TerminalLaunchProfileId::Omp));
    assert!(matches!(
        catalog.resolve_with_peers(request, Some(&bound_session(None, true)), false, None),
        Err(TerminalError::UnbindableLaunchProfile(
            TerminalLaunchProfileId::Omp
        ))
    ));
    bind_handoff_prompt(
        &mut launch,
        TerminalLaunchProfileId::Omp,
        "Review the changes",
    )
    .unwrap();
    let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
        panic!("OMP direct launch");
    };
    assert_eq!(arguments, vec![OsString::from("Review the changes")]);
}
#[test]
fn opencode_launches_directly_and_only_accepts_an_initial_prompt_binding() {
    let directory = TempDir::new().unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Opencode,
        executable(&directory, "opencode"),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let catalog = TerminalLaunchCatalog::new([program]).unwrap();
    let request = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Opencode,
    });
    let mut launch = catalog
        .resolve_with_peers(request, None, false, None)
        .unwrap();
    let ResolvedTerminalLaunch::Program { arguments, .. } = &launch else {
        panic!("OpenCode must not fall back");
    };
    assert!(arguments.is_empty());
    // Capture is host-side configuration, not a program property: the plain
    // test catalog arms nothing, so a pane built from it cannot publish a
    // fresh minted handle as provider truth.
    assert!(!catalog.captures_provider_session(TerminalLaunchProfileId::Opencode));
    // A captured `ses_…` binding reopens the exact chat; an id-less resume
    // requires the private picker bridge when no ID was saved.
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Opencode,
            &bound_session(Some(OPENCODE_CHAT), true),
            false,
        ),
        Ok(vec![OsString::from(format!("--session={OPENCODE_CHAT}"))])
    );
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Opencode,
            &bound_session(None, true),
            false,
        ),
        Err(TerminalError::ProviderSessionCaptureUnavailable(
            TerminalLaunchProfileId::Opencode
        ))
    );
    bind_handoff_prompt(
        &mut launch,
        TerminalLaunchProfileId::Opencode,
        "Review the changes",
    )
    .unwrap();
    let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
        panic!("OpenCode direct launch");
    };
    assert_eq!(
        arguments,
        vec![
            OsString::from("--prompt"),
            OsString::from("Review the changes")
        ]
    );
}

fn opencode_catalog(directory: &TempDir) -> TerminalLaunchCatalog {
    TerminalLaunchCatalog::new([TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Opencode,
        executable(directory, "opencode"),
        Vec::new(),
        vec![(
            OsString::from("INTERFACE_OPENCODE_RECEIPTS_ROOT"),
            directory.path().as_os_str().to_owned(),
        )],
    )
    .expect("opencode program")])
    .expect("catalog")
}

const OPENCODE_CHAT: &str = "ses_f83a11783ffeRtqTE7hTSgvckz";

#[test]
fn opencode_preserves_exact_ids_without_a_lossy_recent_session_probe() {
    let directory = TempDir::new().unwrap();
    let catalog = opencode_catalog(&directory);
    for shared in [false, true] {
        for id in [OPENCODE_CHAT, "ses_oldBeyondListing256"] {
            assert_eq!(
                resolved_session_arguments(
                    &catalog,
                    TerminalLaunchProfileId::Opencode,
                    &bound_session(Some(id), true),
                    shared
                ),
                Ok(vec![OsString::from(format!("--session={id}"))])
            );
        }
    }
}

#[test]
fn opencode_idless_reopens_request_the_picker_in_both_shared_and_single_folders() {
    let directory = TempDir::new().unwrap();
    let catalog = opencode_catalog(&directory);
    for shared in [false, true] {
        for id in [None, Some("11111111-1111-4111-8111-111111111111")] {
            let resolved = catalog
                .resolve_with_peers(
                    Some(TerminalLaunchRequestV1 {
                        version: 1,
                        profile_id: TerminalLaunchProfileId::Opencode,
                    }),
                    Some(&bound_session(id, true)),
                    shared,
                    None,
                )
                .unwrap();
            let ResolvedTerminalLaunch::Program {
                arguments,
                environment,
                ..
            } = resolved
            else {
                panic!("program")
            };
            assert!(arguments.is_empty());
            assert!(environment.contains(&(
                OsString::from("INTERFACE_OPENCODE_RESUME_PICKER"),
                OsString::from("1")
            )));
        }
    }
    assert_eq!(
        resolved_session_arguments(
            &catalog,
            TerminalLaunchProfileId::Opencode,
            &bound_session(None, false),
            false
        ),
        Ok(Vec::new())
    );
}

#[test]
fn native_checkpoint_bindings_resume_only_the_exact_agy_muse_or_kimi_session() {
    let fixtures = [
        include_str!("../../../electron/src/fixtures/terminal-native-resume-v53.json"),
        include_str!("../../../electron/src/fixtures/terminal-kimi-resume-v55.json"),
    ]
    .map(|fixture| serde_json::from_str::<serde_json::Value>(fixture).unwrap());
    for row in fixtures.iter().flat_map(|rows| rows.as_array().unwrap()) {
        let profile: TerminalLaunchProfileId =
            serde_json::from_value(row["profile"].clone()).unwrap();
        let directory = TempDir::new().unwrap();
        let program = TerminalLaunchProgram::new(
            profile,
            executable(&directory, "native-cli"),
            Vec::new(),
            Vec::new(),
        )
        .unwrap()
        .with_provider_session_capture()
        .unwrap();
        let catalog = TerminalLaunchCatalog::new([program]).unwrap();
        let request = Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id: profile,
        });
        let session = bound_session(Some(row["id"].as_str().unwrap()), true);
        let launch = catalog
            .resolve_with_peers(
                request,
                Some(&session),
                false,
                Some("d2d9dfef-c119-4df6-8827-2bad4e79eda6"),
            )
            .unwrap();
        let ResolvedTerminalLaunch::Program { arguments, .. } = launch else {
            panic!("program")
        };
        assert_eq!(
            arguments,
            vec![
                OsString::from(row["resumeArg"].as_str().unwrap()),
                OsString::from(row["providerId"].as_str().unwrap())
            ]
        );
        assert!(
            catalog
                .resolve_with_peers(
                    request,
                    Some(&bound_session(
                        Some(row["providerId"].as_str().unwrap()),
                        true
                    )),
                    false,
                    Some("d2d9dfef-c119-4df6-8827-2bad4e79eda6")
                )
                .is_err()
        );
    }
}

#[test]
fn muse_launches_directly_and_refuses_resume_and_pass_off_bindings() {
    let directory = TempDir::new().unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Muse,
        executable(&directory, "muse"),
        Vec::new(),
        Vec::new(),
    )
    .unwrap();
    let catalog = TerminalLaunchCatalog::new([program]).unwrap();
    let request = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Muse,
    });
    let mut launch = catalog
        .resolve_with_peers(request, None, false, None)
        .unwrap();
    let ResolvedTerminalLaunch::Program { arguments, .. } = &launch else {
        panic!("Muse Code must not fall back");
    };
    assert!(arguments.is_empty());
    assert!(!catalog.captures_provider_session(TerminalLaunchProfileId::Muse));
    assert!(matches!(
        catalog.resolve_with_peers(request, Some(&bound_session(None, true)), false, None),
        Err(TerminalError::UnbindableLaunchProfile(
            TerminalLaunchProfileId::Muse
        ))
    ));
    assert!(matches!(
        bind_handoff_prompt(
            &mut launch,
            TerminalLaunchProfileId::Muse,
            "Review the changes"
        ),
        Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Muse
        ))
    ));
}

#[test]
fn kimi_resumes_only_a_hook_reported_session_and_types_its_pass_off() {
    let directory = TempDir::new().unwrap();
    let program = TerminalLaunchProgram::new(
        TerminalLaunchProfileId::Kimi,
        executable(&directory, "kimi"),
        Vec::new(),
        Vec::new(),
    )
    .unwrap()
    .with_provider_session_capture()
    .unwrap();
    let catalog = TerminalLaunchCatalog::new([program]).unwrap();
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Kimi));
    let request = Some(TerminalLaunchRequestV1 {
        version: 1,
        profile_id: TerminalLaunchProfileId::Kimi,
    });
    let token = "d2d9dfef-c119-4df6-8827-2bad4e79eda6";
    let mut launch = catalog
        .resolve_with_peers(request, None, false, Some(token))
        .unwrap();
    let ResolvedTerminalLaunch::Program {
        arguments,
        environment,
        ..
    } = &launch
    else {
        panic!("Kimi Code must not fall back");
    };
    assert!(arguments.is_empty());
    assert!(
        environment
            .iter()
            .any(|(name, value)| { name == crate::native_checkpoint::TOKEN_ENV && value == token })
    );
    // Kimi mints its own ids: a fresh binding or one without its proof is
    // refused rather than guessed.
    for session in [
        bound_session(Some("kimi-3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73"), false),
        bound_session(None, true),
        bound_session(Some("3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73"), true),
    ] {
        assert!(matches!(
            catalog.resolve_with_peers(request, Some(&session), false, Some(token)),
            Err(TerminalError::UnbindableLaunchProfile(
                TerminalLaunchProfileId::Kimi
            ))
        ));
    }
    assert_eq!(
        bind_handoff_prompt(&mut launch, TerminalLaunchProfileId::Kimi, "Read /tmp/h.md"),
        Ok(crate::launch::HandoffDelivery::Typed(
            "Read /tmp/h.md".into()
        ))
    );
    let ResolvedTerminalLaunch::Program { arguments, .. } = &launch else {
        unreachable!()
    };
    assert!(arguments.is_empty(), "Kimi rejects positional prompts");
    assert_eq!(
        bind_handoff_prompt(&mut launch, TerminalLaunchProfileId::Kimi, "a\rb"),
        Err(TerminalError::HandoffUnsupportedProfile(
            TerminalLaunchProfileId::Kimi
        ))
    );
}

#[test]
fn sign_in_runs_each_cli_s_own_login_and_only_where_one_exists() {
    let directory = TempDir::new().expect("directory");
    let catalog = managed_session_catalog(&directory);
    let sign_in = |profile_id| {
        catalog.resolve_sign_in(Some(TerminalLaunchRequestV1 {
            version: 1,
            profile_id,
        }))
    };
    for (profile, argv) in [
        (TerminalLaunchProfileId::Claude, vec!["auth", "login"]),
        (TerminalLaunchProfileId::Codex, vec!["login"]),
        (TerminalLaunchProfileId::Grok, vec!["login"]),
    ] {
        let Ok(ResolvedTerminalLaunch::Program { arguments, .. }) = sign_in(profile) else {
            panic!("{profile:?} signs in with its CLI");
        };
        assert_eq!(
            arguments,
            argv.into_iter().map(OsString::from).collect::<Vec<_>>()
        );
    }
    assert_eq!(
        sign_in(TerminalLaunchProfileId::Gemini),
        Err(TerminalError::NoSignIn(TerminalLaunchProfileId::Gemini))
    );
    assert_eq!(
        sign_in(TerminalLaunchProfileId::Qwen),
        Err(TerminalError::NoSignIn(TerminalLaunchProfileId::Qwen))
    );
    assert_eq!(
        catalog.resolve_sign_in(None),
        Err(TerminalError::NoSignIn(TerminalLaunchProfileId::Shell))
    );
}
