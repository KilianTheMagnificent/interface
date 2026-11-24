use std::{fs, io::Cursor, os::unix::fs::PermissionsExt};

use tempfile::TempDir;

use super::*;

fn environment() -> (TempDir, AppEnvironment) {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let cwd = temp.path().join("project");
    let data = temp.path().join("iface-data");
    let app_data = temp.path().join("Interface");
    for directory in [&home, &cwd] {
        fs::create_dir(directory).expect("directory");
    }
    let paths = resolve_cli_paths(Some(data.clone().into_os_string()), &home);
    (
        temp,
        AppEnvironment {
            cwd,
            home,
            paths,
            user_data_root: app_data,
            stdin: String::new(),
            executable: PathBuf::from("/bin/true"),
            original_args: Vec::new(),
            runtime: None,
            processes: Arc::new(SystemBackgroundProcesses),
            customization_user_root: None,
        },
    )
}

#[test]
fn exec_stdin_is_unbounded_and_uses_utf8_replacement_decoding() {
    let parsed = crate::argv::parse_argv(&["exec".to_owned(), "-".to_owned()]).expect("exec argv");
    let mut input = vec![b'x'; AGENT_MAX_PROMPT_BYTES + 1];
    input.extend_from_slice(&[0xff, b'z']);
    let expected_bytes = input.len();
    let mut reader = Cursor::new(input);

    let text = read_stdin_from(&parsed, &mut reader).expect("exec stdin");
    assert!(text.starts_with('x'));
    assert!(text.ends_with("�z"));
    assert_eq!(reader.position(), expected_bytes as u64);
}

#[test]
fn agent_stdin_stops_at_the_limit_and_returns_json_usage_exit_two() {
    let parsed = crate::argv::parse_argv(&[
        "agent".to_owned(),
        "send".to_owned(),
        "reviewer".to_owned(),
        "-".to_owned(),
        "--output".to_owned(),
        "json".to_owned(),
    ])
    .expect("agent argv");
    let mut reader = Cursor::new(vec![b'x'; AGENT_MAX_PROMPT_BYTES * 4]);

    let error = read_stdin_from(&parsed, &mut reader).expect_err("oversized stdin");
    assert_eq!(error.exit_code, CLI_EXIT.validation);
    assert_eq!(reader.position(), (AGENT_MAX_PROMPT_BYTES + 1) as u64);
    let json: serde_json::Value = serde_json::from_str(error.stderr.trim()).expect("json error");
    assert_eq!(json["kind"], "usage");
    assert_eq!(json["operation"], "agent.send");
    assert_eq!(
        json["message"],
        "agent send stdin is limited to 64 KiB of UTF-8"
    );
}

#[test]
fn agent_stdin_fatally_rejects_invalid_utf8_at_exit_two() {
    let parsed = crate::argv::parse_argv(&[
        "agent".to_owned(),
        "steer".to_owned(),
        "reviewer".to_owned(),
        "-".to_owned(),
        "--output".to_owned(),
        "text".to_owned(),
    ])
    .expect("agent argv");
    let mut reader = Cursor::new(vec![0xff]);

    let error = read_stdin_from(&parsed, &mut reader).expect_err("invalid UTF-8");
    assert_eq!(error.exit_code, CLI_EXIT.validation);
    assert_eq!(
        error.stderr,
        "iface: agent steer stdin must be valid UTF-8\n"
    );
}

#[tokio::test]
async fn version_prints_the_product_version_without_touching_state() {
    let (_temp, mut environment) = environment();
    let parsed = crate::argv::parse_argv(&["--version".to_owned()]).expect("version argv");

    let result = run(parsed, &mut environment).await;

    assert_eq!(result.exit_code, CLI_EXIT.success);
    assert_eq!(result.stdout, format!("{PRODUCT_VERSION}\n"));
    assert!(result.stderr.is_empty());
    assert!(!environment.paths.data_dir.exists());
}

#[tokio::test]
async fn doctor_does_not_create_provider_or_runtime_state() {
    let (_temp, mut environment) = environment();
    let parsed = CliArgv {
        command: Some(CliCommand::Doctor),
        output: Some(OutputFormat::Json),
        ..CliArgv::default()
    };
    let result = run(parsed, &mut environment).await;
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(result.stdout.starts_with("{\"schemaVersion\":1,"));
    assert!(!environment.paths.data_dir.exists());
    assert!(!environment.user_data_root.exists());
}

#[tokio::test]
async fn agent_help_is_exact_and_does_not_touch_the_control_host_or_data_tree() {
    let (_temp, mut environment) = environment();
    let parsed = CliArgv {
        command: Some(CliCommand::Agent),
        agent_args: vec!["--help".into()],
        ..CliArgv::default()
    };

    let result = run(parsed, &mut environment).await;

    assert_eq!(result.exit_code, CLI_EXIT.success);
    assert_eq!(result.stdout, format!("{}\n", agent::AGENT_USAGE));
    assert!(result.stderr.is_empty());
    assert!(!environment.paths.data_dir.exists());
    assert!(!environment.user_data_root.exists());
}

#[tokio::test]
async fn foreground_exec_refuses_an_active_writer_lock_before_runtime_startup() {
    let (_temp, mut environment) = environment();
    ensure_private_root(&environment.paths.data_dir).expect("private data root");
    let lock = acquire_cli_instance_lock(&environment.paths.instance_lock).expect("lock");
    let parsed = CliArgv {
        command: Some(CliCommand::Exec),
        prompt: Some("hello".into()),
        ..CliArgv::default()
    };

    let result = run(parsed, &mut environment).await;

    assert_eq!(result.exit_code, CLI_EXIT.generic);
    assert!(result.stderr.contains("Another iface process"));
    assert!(environment.runtime.is_none());
    lock.release().expect("release");
}

#[tokio::test]
async fn production_exec_releases_its_writer_lock_and_runtime_on_local_refusal() {
    let (_temp, mut environment) = environment();
    let parsed = CliArgv {
        command: Some(CliCommand::Exec),
        prompt: Some("hello".into()),
        engine: Some("acp:unregistered".into()),
        ..CliArgv::default()
    };

    let result = run(parsed, &mut environment).await;

    assert_eq!(result.exit_code, CLI_EXIT.unsent);
    assert!(result.stderr.contains("registered"), "{result:?}");
    assert!(environment.runtime.is_none());
    assert!(!environment.paths.instance_lock.exists());
}

#[test]
fn exec_teardown_failures_warn_without_replacing_the_turns_outcome() {
    let accepted = AppResult {
        stdout: "done\n".into(),
        stderr: String::new(),
        exit_code: CLI_EXIT.success,
    };
    let kept = with_teardown_warnings(
        accepted,
        [
            Some((
                "runtime shutdown",
                "provider session close timed out during runtime shutdown".into(),
            )),
            None,
        ],
    );
    assert_eq!(kept.stdout, "done\n");
    assert_eq!(kept.exit_code, CLI_EXIT.success);
    assert_eq!(
        kept.stderr,
        "iface: warning: runtime shutdown failed after exec: provider session close timed out during runtime shutdown\n"
    );

    let unknown = AppResult {
        stdout: "{\"result\":\"outcome-unknown\"}\n".into(),
        stderr: "iface: Delivery unknown.\n".into(),
        exit_code: CLI_EXIT.unknown,
    };
    let kept = with_teardown_warnings(
        unknown,
        [
            Some(("runtime shutdown", "close failed".into())),
            Some(("instance lock release", "lock is gone\u{1b}[31m".into())),
        ],
    );
    assert_eq!(kept.exit_code, CLI_EXIT.unknown);
    assert_eq!(kept.stdout, "{\"result\":\"outcome-unknown\"}\n");
    assert_eq!(
        kept.stderr,
        "iface: Delivery unknown.\niface: warning: runtime shutdown failed after exec: close failed\niface: warning: instance lock release failed after exec: lock is gone\n"
    );
}

#[tokio::test]
async fn one_shot_client_never_opens_a_second_store_owner() {
    let (_temp, environment) = environment();
    let owner = open_one_shot_orchestration(&environment)
        .await
        .expect("owner");
    assert!(owner.runtime.is_some());
    assert!(!owner.live_provider_host);

    let client_data = environment.home.join("client-iface-data");
    let client_paths = resolve_cli_paths(
        Some(client_data.clone().into_os_string()),
        &environment.home,
    );
    let client_environment = AppEnvironment {
        cwd: environment.cwd.clone(),
        home: environment.home.clone(),
        paths: client_paths,
        user_data_root: environment.user_data_root.clone(),
        stdin: String::new(),
        executable: PathBuf::from("/bin/true"),
        original_args: Vec::new(),
        runtime: None,
        processes: Arc::new(SystemBackgroundProcesses),
        customization_user_root: None,
    };
    let client = open_one_shot_orchestration(&client_environment)
        .await
        .expect("client");
    assert!(client.runtime.is_none());
    assert!(client.live_provider_host);
    assert!(
        !client_data.exists(),
        "client mode opened local runtime state"
    );

    // The client answers from the owner's canonical store rather than one of
    // its own: both sides report the same host identity and revision.
    let from_client: interface_contracts::orchestration_control::OrchestrationControlSnapshotV1 =
        client
            .control
            .invoke(
                interface_contracts::orchestration_control::OrchestrationControlMethod::OrchestrationSnapshot,
                &serde_json::json!({}),
            )
            .await
            .expect("client snapshot");
    let from_owner: interface_contracts::orchestration_control::OrchestrationControlSnapshotV1 =
        owner
            .control
            .invoke(
                interface_contracts::orchestration_control::OrchestrationControlMethod::OrchestrationSnapshot,
                &serde_json::json!({}),
            )
            .await
            .expect("owner snapshot");
    assert_eq!(from_client.host_id, from_owner.host_id);
    assert_eq!(from_client.revision, from_owner.revision);

    client.close().await.expect("client close");
    assert!(environment.user_data_root.join("host.sock").exists());
    owner.close().await.expect("owner close");
    assert!(!environment.user_data_root.join("host.sock").exists());
    assert!(!environment.user_data_root.join("host.lock").exists());
}

#[test]
fn runtime_registers_builtins_and_owner_private_custom_acp_profiles_without_launching() {
    let temp = tempfile::tempdir().expect("tempdir");
    let home = temp.path().join("home");
    let data = temp.path().join("iface-data");
    fs::create_dir(&home).expect("home");
    let paths = resolve_cli_paths(Some(data.into_os_string()), &home);
    ensure_private_root(&paths.data_dir).expect("private data root");
    let mut profiles =
        interface_settings::AcpProfileStore::open(&paths.acp_profiles).expect("profile store");
    profiles
        .save(&[interface_contracts::ipc::AcpProfile {
            slug: "local-agent".into(),
            display_name: "Local Agent".into(),
            executable_path: "/fixtures/local-agent".into(),
            args: vec!["acp".into(), "stdio".into()],
        }])
        .expect("save profile");
    drop(profiles);

    let runtime = build_runtime(
        &paths,
        &home,
        None,
        None,
        &CliExecutableLookup::for_runtime(),
    )
    .expect("runtime");

    assert_eq!(
        runtime
            .registered_engines()
            .into_iter()
            .map(|engine| engine.to_string())
            .collect::<Vec<_>>(),
        vec!["acp:grok", "acp:local-agent", "acp:qwen", "claude", "codex"]
    );
}

#[test]
fn runtime_preserves_the_legacy_prompt_history_file() {
    let (_temp, environment) = environment();
    ensure_private_root(&environment.paths.data_dir).expect("private data root");
    let legacy = b"{\"at\":\"2026-08-27T00:00:00.000Z\",\"engine\":\"claude\",\"cwd\":\"/repo\",\"text\":\"hello\"}\n";
    fs::write(&environment.paths.history, legacy).expect("legacy prompt history");
    fs::set_permissions(
        &environment.paths.history,
        fs::Permissions::from_mode(0o600),
    )
    .expect("history mode");

    let runtime = build_runtime(
        &environment.paths,
        &environment.home,
        None,
        None,
        &CliExecutableLookup::for_runtime(),
    )
    .expect("runtime");

    assert_eq!(
        fs::read(&environment.paths.history).expect("history preserved"),
        legacy
    );
    assert!(!environment.paths.data_dir.join("engine-history").exists());
    drop(runtime);
}

#[test]
fn executable_lookup_uses_one_login_path_after_inherited_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inherited = temp.path().join("inherited");
    let login = temp.path().join("login");
    fs::create_dir(&inherited).expect("inherited");
    fs::create_dir(&login).expect("login");
    let executable = login.join("codex");
    fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("fixture executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).expect("executable mode");
    let lookup = CliExecutableLookup {
        inherited_path: Some(inherited.into_os_string()),
        login_path: OnceLock::new(),
        probe: Mutex::new(None),
    };
    lookup
        .login_path
        .set(Some(login.into_os_string()))
        .expect("login path");

    assert_eq!(lookup.find("codex"), Some(executable));
    assert!(lookup.find("../codex").is_none());
}

#[test]
fn overrides_and_optional_helpers_never_spawn_the_login_shell_lookup() {
    let temp = tempfile::tempdir().expect("tempdir");
    let grok = temp.path().join("grok");
    fs::write(&grok, b"#!/bin/sh\nexit 0\n").expect("fixture executable");
    fs::set_permissions(&grok, fs::Permissions::from_mode(0o700)).expect("executable mode");
    let lookup = CliExecutableLookup {
        inherited_path: Some(temp.path().join("empty").into_os_string()),
        login_path: OnceLock::new(),
        probe: Mutex::new(None),
    };

    assert_eq!(
        resolve_builtin_acp_executable_with(
            BuiltinAcpProvider::Grok,
            temp.path(),
            Some(grok.clone()),
            &lookup,
        ),
        grok
    );
    assert!(lookup.find_inherited("qwen-mlx-server").is_none());
    assert!(
        lookup.login_path.get().is_none(),
        "no login shell was consulted"
    );
}

#[test]
fn the_login_shell_probe_starts_early_only_for_a_provider_it_would_look_up() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inherited = temp.path().join("inherited");
    fs::create_dir(&inherited).expect("inherited");
    for binary in ["claude", "codex", "grok", "qwen"] {
        let executable = inherited.join(binary);
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("fixture executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("executable mode");
    }
    let lookup = |path: &Path| CliExecutableLookup {
        inherited_path: Some(path.as_os_str().to_owned()),
        login_path: OnceLock::new(),
        probe: Mutex::new(None),
    };
    assert!(!lookup(&inherited).login_shell_needed(|_| false));

    fs::remove_file(inherited.join("qwen")).expect("remove qwen");
    assert!(lookup(&inherited).login_shell_needed(|_| false));
    assert!(!lookup(&inherited).login_shell_needed(|name| name == "QWEN_EXECUTABLE"));
}

#[test]
fn machine_provider_resolution_accepts_an_ordinary_executable_candidate() {
    let temp = tempfile::tempdir().expect("tempdir");
    let command = temp.path().join("grok");
    fs::write(&command, b"#!/bin/sh\nexit 1\n").expect("candidate");
    fs::set_permissions(&command, fs::Permissions::from_mode(0o700)).expect("candidate mode");
    let lookup = CliExecutableLookup {
        inherited_path: Some(temp.path().as_os_str().to_owned()),
        login_path: OnceLock::new(),
        probe: Mutex::new(None),
    };

    assert_eq!(
        resolve_builtin_acp_executable_with(BuiltinAcpProvider::Grok, temp.path(), None, &lookup),
        command
    );
}

/// Each CLI update repoints the link and prunes the version it pointed at, so
/// a long-open TUI must keep launching through the link, as the desktop host
/// does.
#[test]
fn provider_resolution_keeps_the_link_not_the_version_it_points_at() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    let versions = temp.path().join("versions");
    fs::create_dir(&bin).expect("bin");
    fs::create_dir(&versions).expect("versions");
    let version = versions.join("1");
    fs::write(&version, b"#!/bin/sh\nexit 0\n").expect("version");
    fs::set_permissions(&version, fs::Permissions::from_mode(0o700)).expect("version mode");
    for binary in ["claude", "codex", "grok"] {
        std::os::unix::fs::symlink(&version, bin.join(binary)).expect("link");
    }
    let lookup = CliExecutableLookup {
        inherited_path: Some(bin.as_os_str().to_owned()),
        login_path: OnceLock::new(),
        probe: Mutex::new(None),
    };
    let home = temp.path().join("home");

    assert_eq!(
        resolve_claude_executable(&home, None, &lookup),
        Some(bin.join("claude"))
    );
    assert_eq!(
        resolve_codex_executable(&home, None, &lookup),
        Some(bin.join("codex"))
    );
    assert_eq!(
        resolve_builtin_acp_executable_with(BuiltinAcpProvider::Grok, &home, None, &lookup),
        bin.join("grok")
    );
    assert_eq!(
        resolve_claude_executable(&home, Some(bin.join("claude")), &lookup),
        Some(bin.join("claude")),
        "an override link is kept too"
    );
}

#[test]
fn claude_resolution_admits_a_file_without_an_execute_bit_as_the_desktop_host_does() {
    let temp = tempfile::tempdir().expect("tempdir");
    let claude = temp.path().join("claude");
    fs::write(&claude, b"#!/bin/sh\nexit 0\n").expect("claude");
    fs::set_permissions(&claude, fs::Permissions::from_mode(0o600)).expect("claude mode");
    let lookup = CliExecutableLookup {
        inherited_path: Some(temp.path().join("empty").into_os_string()),
        login_path: OnceLock::new(),
        probe: Mutex::new(None),
    };
    lookup.login_path.set(None).expect("login path");

    assert_eq!(
        resolve_claude_executable(temp.path(), Some(claude.clone()), &lookup),
        Some(claude)
    );
}

#[test]
fn background_flag_is_removed_as_one_exact_pair() {
    let args = ["exec", "--bg", "job", "hello", "job"].map(str::to_owned);
    assert_eq!(
        without_background_flag(&args, "job").collect::<Vec<_>>(),
        ["exec", "hello", "job"]
    );
}

/// `iface trigger`'s answer: sent and waiting succeed, refused is
/// definitely unsent (3), unknown stays unknown (4); with no Interface
/// running it says so and nothing was sent.
#[tokio::test]
async fn trigger_reports_each_outcome_with_its_exit_code() {
    let answer = |outcome: &str| {
        serde_json::json!({"id": "s-1", "name": "Nightly", "outcome": outcome,
            "detail": "An Away loop is running this thread."})
    };
    let sent = trigger_result(&answer("sent"));
    assert_eq!(sent.exit_code, CLI_EXIT.success);
    assert_eq!(sent.stdout, "Sent \"Nightly\" to its thread.\n");
    assert_eq!(
        trigger_result(&answer("waiting")).exit_code,
        CLI_EXIT.success
    );
    let refused = trigger_result(&answer("refused"));
    assert_eq!(refused.exit_code, CLI_EXIT.unsent);
    assert!(refused.stderr.contains("Away loop"));
    assert_eq!(
        trigger_result(&answer("skipped")).exit_code,
        CLI_EXIT.unsent
    );
    assert_eq!(
        trigger_result(&answer("unknown")).exit_code,
        CLI_EXIT.unknown
    );

    let temp = TempDir::new().unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut environment = AppEnvironment::system(Vec::new()).unwrap();
    environment.user_data_root = temp.path().join("Interface");
    fs::create_dir(&environment.user_data_root).unwrap();
    let missing = trigger_command("Nightly", &environment).await;
    assert_eq!(missing.exit_code, CLI_EXIT.unsent);
    assert!(missing.stderr.contains("not running"), "{}", missing.stderr);
}
