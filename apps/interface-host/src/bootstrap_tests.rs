#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
use interface_contracts::terminal::{
    TERMINAL_LAUNCH_SESSION_VERSION, TerminalGeneration, TerminalId, TerminalLaunchSessionV1,
    TerminalSize, TerminalState, TerminalSummary,
};
use interface_contracts::{
    engine::{LaunchOptions, SessionKind},
    ipc::CHANNELS,
    terminal::{TerminalLaunchProfileId, TerminalLaunchRequestV1},
};
use interface_terminal::ResolvedTerminalLaunch;
use serde_json::json;

use super::*;

/// A zsh launch for overlay tests that cannot inherit the test runner's own
/// armed manual-capture pane. Without this, running the gate inside an
/// Interface pane exports the production host and provider executables into
/// the overlay under test, and the shell takes the capture-wrapper branch
/// the fixture never armed.
fn isolated_zsh() -> Command {
    let mut command = Command::new("/bin/zsh");
    for name in crate::claude_manual_wrapper::MANUAL_ENVIRONMENT {
        command.env_remove(name);
    }
    command.env_remove(crate::pane_mcp::PANE_MCP_SOCKET_ENV);
    command
}

#[test]
fn opencode_activity_overlay_installs_only_the_private_transport() {
    let root = tempfile::tempdir().unwrap();
    let config = install_opencode_activity_transport(root.path()).unwrap();
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
    let plugin = settings["plugin"][0].as_str().unwrap();
    assert!(Path::new(plugin).starts_with(root.path()));
    assert_eq!(
        std::fs::read_to_string(plugin).unwrap(),
        interface_terminal::OPENCODE_ACTIVITY_TRANSPORT
    );
    assert_eq!(settings.as_object().unwrap().len(), 1);
    assert_eq!(
        install_opencode_activity_transport(root.path()).unwrap(),
        config
    );
}

#[test]
fn unchanged_launch_overlays_are_not_rewritten_on_every_boot() {
    use std::os::unix::fs::MetadataExt;

    let root = tempfile::tempdir().unwrap();
    let zsh = install_manual_claude_zsh_overlay(root.path()).unwrap();
    let opencode = install_opencode_activity_transport(root.path())
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let files = [
        zsh.join(".zshenv"),
        zsh.join(".zprofile"),
        zsh.join(".zshrc"),
        opencode.join("activity-v1.mjs"),
        opencode.join("tui-v1.json"),
    ];
    let inode = |path: &Path| std::fs::metadata(path).unwrap().ino();
    let before: Vec<_> = files.iter().map(|path| inode(path)).collect();
    install_manual_claude_zsh_overlay(root.path()).unwrap();
    install_opencode_activity_transport(root.path()).unwrap();
    // An atomic rewrite renames a new file into place; identical bytes stay put.
    assert_eq!(
        files.iter().map(|path| inode(path)).collect::<Vec<_>>(),
        before
    );

    // Different bytes are still replaced with the pinned contents.
    std::fs::write(&files[2], "# edited\n").unwrap();
    std::fs::write(&files[3], "// edited\n").unwrap();
    install_manual_claude_zsh_overlay(root.path()).unwrap();
    install_opencode_activity_transport(root.path()).unwrap();
    assert_eq!(std::fs::read_to_string(&files[2]).unwrap(), ZSHRC);
    assert_eq!(
        std::fs::read_to_string(&files[3]).unwrap(),
        interface_terminal::OPENCODE_ACTIVITY_TRANSPORT
    );
    assert_ne!(inode(&files[2]), before[2]);
    assert_eq!(inode(&files[0]), before[0]);
}

#[cfg(unix)]
fn executable_script(directory: &Path, name: &str, body: &str) -> PathBuf {
    let script = directory.join(name);
    std::fs::write(&script, body).expect("write lookup fixture");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("make lookup fixture executable");
    script
}

#[cfg(unix)]
fn executable_lookup(shell: PathBuf, timeout: Duration) -> LoginShellExecutableLookup {
    LoginShellExecutableLookup {
        shell,
        timeout,
        login_path: OnceLock::new(),
        started: Mutex::new(None),
    }
}

fn open_test_runtime_at(
    runtime_root: &Path,
    active_profiles: &[AcpProfile],
    home: &Path,
) -> Result<SessionRuntime, Box<dyn Error>> {
    Ok(open_runtime_at_with_epoch(
        runtime_root,
        active_profiles,
        "test-runtime-epoch",
        home,
        true,
        None,
        None,
        env!("CARGO_PKG_VERSION"),
        None,
    )?
    .runtime)
}

#[cfg(unix)]
#[test]
fn an_early_login_shell_probe_runs_on_its_own_and_is_used_once() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let binary_directory = temporary.path().join("bin");
    std::fs::create_dir(&binary_directory).expect("binary directory");
    executable_script(&binary_directory, "codex", "#!/bin/sh\nexit 0\n");
    let shell = executable_script(
        temporary.path(),
        "fake-shell",
        "#!/bin/sh\nroot=${0%/*}\nprintf x >> \"$root/invocations\"\nprintf '__INTERFACE_LOGIN_PATH__%s\\n' \"$root/bin\"\n",
    );
    let invocations = temporary.path().join("invocations");
    let probe = LoginShellPathProbe::start_with(shell, Duration::from_secs(5));
    // The shell starts before anything asks for a provider.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !invocations.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the early probe did not start its shell"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let lookup = LoginShellExecutableLookup::new(Some(probe));
    assert_eq!(lookup.find("codex"), Some(binary_directory.join("codex")));
    assert_eq!(lookup.find("claude"), None);
    assert_eq!(
        std::fs::read_to_string(&invocations).expect("invocation log"),
        "x"
    );
}

#[cfg(unix)]
#[test]
fn a_finished_probe_hands_its_path_to_the_cli_lookup() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let binary_directory = temporary.path().join("bin");
    std::fs::create_dir(&binary_directory).expect("binary directory");
    executable_script(&binary_directory, "codex", "#!/bin/sh\nexit 0\n");
    let shell = executable_script(
        temporary.path(),
        "fake-shell",
        "#!/bin/sh\nroot=${0%/*}\nprintf '__INTERFACE_LOGIN_PATH__%s\\n' \"$root/bin\"\n",
    );
    let login_path = LoginShellPathProbe::start_with(shell, Duration::from_secs(5)).finish();

    assert_eq!(
        provider_executable_on_path("codex", login_path.as_deref()),
        Some(binary_directory.join("codex"))
    );
    assert_eq!(
        provider_executable_on_path("../codex", login_path.as_deref()),
        None
    );
    assert_eq!(
        provider_executable_on_path("claude", login_path.as_deref()),
        None
    );
}

#[cfg(unix)]
#[test]
fn login_shell_path_is_cached_for_all_provider_names() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let binary_directory = temporary.path().join("bin");
    std::fs::create_dir(&binary_directory).expect("binary directory");
    let names = [
        "claude", "codex", "grok", "qwen", "agy", "omp", "opencode", "muse", "kimi",
    ];
    for name in names {
        executable_script(&binary_directory, name, "#!/bin/sh\nexit 0\n");
    }
    let shell = executable_script(
        temporary.path(),
        "fake-shell",
        "#!/bin/sh\nroot=${0%/*}\nprintf x >> \"$root/invocations\"\nprintf '__INTERFACE_LOGIN_PATH__/stale\\ninteractive banner\\n__INTERFACE_LOGIN_PATH__%s\\n' \"$root/bin\"\n",
    );
    let lookup = executable_lookup(shell, Duration::from_secs(1));

    for name in names {
        assert_eq!(lookup.find(name), Some(binary_directory.join(name)));
    }
    assert_eq!(
        resolve_opencode_executable(&temporary.path().join("home"), &lookup),
        binary_directory.join("opencode")
    );
    assert_eq!(
        resolve_omp_executable(&temporary.path().join("home"), &lookup),
        binary_directory.join("omp")
    );
    assert_eq!(
        resolve_muse_executable(&temporary.path().join("home"), &lookup),
        binary_directory.join("muse")
    );
    assert_eq!(
        resolve_kimi_executable(&temporary.path().join("home"), &lookup),
        binary_directory.join("kimi")
    );
    assert_eq!(
        resolve_antigravity_executable(&temporary.path().join("home"), None, &lookup),
        binary_directory.join("agy")
    );
    assert_eq!(
        std::fs::read(temporary.path().join("invocations")).expect("invocation record"),
        b"x"
    );
}

#[cfg(unix)]
#[test]
fn opencode_discovery_accepts_installer_and_npm_paths_but_skips_non_executables() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path();
    let installer_bin = home.join(".opencode/bin");
    let npm_bin = home.join(".local/bin");
    std::fs::create_dir_all(&installer_bin).unwrap();
    std::fs::create_dir_all(&npm_bin).unwrap();
    let lookup = executable_lookup(home.join("unused-shell"), Duration::from_secs(1));
    lookup.login_path.set(None).unwrap();
    let installer = installer_bin.join("opencode");
    std::fs::write(&installer, "not executable").unwrap();
    let npm = executable_script(&npm_bin, "opencode", "#!/bin/sh\nexit 0\n");
    assert_eq!(resolve_opencode_executable(home, &lookup), npm);
    executable_script(&installer_bin, "opencode", "#!/bin/sh\nexit 0\n");
    assert_eq!(resolve_opencode_executable(home, &lookup), installer);
}

#[cfg(unix)]
#[test]
fn muse_discovery_uses_the_installer_launcher_but_skips_non_executables() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path();
    let installer_bin = home.join(".local/bin");
    std::fs::create_dir_all(&installer_bin).unwrap();
    let lookup = executable_lookup(home.join("unused-shell"), Duration::from_secs(1));
    lookup.login_path.set(None).unwrap();
    let launcher = installer_bin.join("muse");
    std::fs::write(&launcher, "not executable").unwrap();
    // Unavailable resolves to the installer location so the launcher can say
    // where Muse Code was expected.
    assert_eq!(resolve_muse_executable(home, &lookup), launcher);
    assert!(!executable_file(&launcher));
    executable_script(&installer_bin, "muse", "#!/bin/sh\nexit 0\n");
    assert_eq!(resolve_muse_executable(home, &lookup), launcher);
    assert!(executable_file(&launcher));
}

#[cfg(unix)]
#[test]
fn kimi_discovery_uses_the_installer_binary_but_skips_non_executables() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path();
    let installer_bin = home.join(".kimi-code/bin");
    std::fs::create_dir_all(&installer_bin).unwrap();
    let lookup = executable_lookup(home.join("unused-shell"), Duration::from_secs(1));
    lookup.login_path.set(None).unwrap();
    let installed = installer_bin.join("kimi");
    std::fs::write(&installed, "not executable").unwrap();
    // Unavailable resolves to the installer location so the launcher can say
    // where Kimi Code was expected.
    assert_eq!(resolve_kimi_executable(home, &lookup), installed);
    assert!(!executable_file(&installed));
    executable_script(&installer_bin, "kimi", "#!/bin/sh\nexit 0\n");
    assert_eq!(resolve_kimi_executable(home, &lookup), installed);
    assert!(executable_file(&installed));
}

#[cfg(unix)]
#[test]
fn explicit_antigravity_override_precedes_a_different_login_path_installation() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let home = temporary.path().join("home");
    let login_bin = temporary.path().join("login-bin");
    std::fs::create_dir(&login_bin).expect("login bin");
    let explicit = executable_script(temporary.path(), "agy-explicit", "#!/bin/sh\nexit 0\n");
    executable_script(&login_bin, "agy", "#!/bin/sh\nexit 0\n");
    let lookup = executable_lookup(
        temporary.path().join("unused-shell"),
        Duration::from_secs(1),
    );
    lookup
        .login_path
        .set(Some(login_bin.into_os_string()))
        .expect("login path");

    assert_eq!(
        resolve_antigravity_executable(&home, Some(explicit.clone()), &lookup),
        explicit
    );
}

#[cfg(unix)]
#[test]
fn stalled_login_shell_consumes_one_shared_lookup_budget() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let shell = executable_script(
        temporary.path(),
        "stalled-shell",
        "#!/bin/sh\nexec /bin/sleep 2\n",
    );
    let lookup = executable_lookup(shell, Duration::from_millis(200));
    let started = Instant::now();

    for name in [
        "claude", "codex", "grok", "qwen", "agy", "omp", "opencode", "muse", "kimi",
    ] {
        assert_eq!(lookup.find(name), None);
    }
    assert!(started.elapsed() < Duration::from_millis(600));
}

#[cfg(unix)]
#[test]
fn a_background_job_holding_stdout_keeps_the_finished_shells_path() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let binary_directory = temporary.path().join("bin");
    std::fs::create_dir(&binary_directory).expect("binary directory");
    executable_script(&binary_directory, "codex", "#!/bin/sh\nexit 0\n");
    let shell = executable_script(
        temporary.path(),
        "background-shell",
        "#!/bin/sh\nroot=${0%/*}\n(trap '' HUP; /bin/sleep 2) &\nprintf '__INTERFACE_LOGIN_PATH__%s\\n' \"$root/bin\"\nexit 0\n",
    );
    let lookup = executable_lookup(shell, Duration::from_secs(5));
    let started = Instant::now();

    // The shell has exited with its marker printed; the job still holding
    // stdout neither discards that PATH nor holds the lookup to its budget.
    assert_eq!(lookup.find("codex"), Some(binary_directory.join("codex")));
    assert!(started.elapsed() < Duration::from_millis(600));
}

#[cfg(unix)]
#[test]
fn the_last_marker_a_login_shell_prints_wins_over_an_earlier_one() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let binary_directory = temporary.path().join("bin");
    std::fs::create_dir(&binary_directory).expect("binary directory");
    executable_script(&binary_directory, "codex", "#!/bin/sh\nexit 0\n");
    let shell = executable_script(
        temporary.path(),
        "late-marker-shell",
        "#!/bin/sh\nroot=${0%/*}\nprintf '__INTERFACE_LOGIN_PATH__/stale\\n'\n/bin/sleep 0.2\nprintf '__INTERFACE_LOGIN_PATH__%s\\n' \"$root/bin\"\n",
    );
    let lookup = executable_lookup(shell, Duration::from_secs(5));

    assert_eq!(lookup.find("codex"), Some(binary_directory.join("codex")));
}

#[cfg(unix)]
#[test]
fn login_path_lookup_rejects_unsafe_names_and_non_executable_entries() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let binary_directory = temporary.path().join("bin");
    std::fs::create_dir(&binary_directory).expect("binary directory");
    let non_executable = binary_directory.join("codex");
    std::fs::write(&non_executable, "not executable").expect("non-executable fixture");
    std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o600))
        .expect("non-executable mode");
    let lookup = executable_lookup(
        temporary.path().join("unused-shell"),
        Duration::from_secs(1),
    );
    lookup
        .login_path
        .set(Some(binary_directory.into_os_string()))
        .expect("fixture login path");

    assert_eq!(lookup.find("codex"), None);
    assert_eq!(lookup.find("../codex"), None);
    assert_eq!(lookup.find("codex;touch-bad"), None);

    let relative_lookup = executable_lookup(
        temporary.path().join("unused-shell"),
        Duration::from_secs(1),
    );
    relative_lookup
        .login_path
        .set(Some(OsString::from("relative/bin")))
        .expect("relative fixture path");
    assert_eq!(relative_lookup.find("codex"), None);
}

#[cfg(unix)]
#[test]
fn manual_claude_zsh_overlay_preserves_user_startup_and_existing_functions() {
    let shell = Path::new("/bin/zsh");
    if !shell.is_file() {
        return;
    }
    let temporary = tempfile::tempdir().expect("temporary directory");
    let runtime = temporary.path().join("runtime");
    let home = temporary.path().join("home");
    std::fs::create_dir(&runtime).expect("runtime directory");
    std::fs::create_dir(&home).expect("home directory");
    std::fs::write(
        home.join(".zshrc"),
        "export INTERFACE_TEST_USER_RC=loaded\nfunction claude { print original-claude; }\n",
    )
    .expect("user zshrc");
    let overlay = install_manual_claude_zsh_overlay(&runtime).expect("zsh overlay");

    let output = isolated_zsh()
        .args([
            "-lic",
            "[[ $INTERFACE_TEST_USER_RC == loaded ]] || exit 21; claude",
        ])
        .env("HOME", &home)
        .env("ZDOTDIR", &overlay)
        .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
        .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &home)
        .env(CLAUDE_MANUAL_HOST_ENV, "/definitely/not/invoked")
        .output()
        .expect("launch isolated zsh");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "original-claude"
    );

    std::fs::write(
        home.join(".zshrc"),
        "export INTERFACE_TEST_USER_RC=loaded\nunsetopt RCS\n",
    )
    .expect("plain user zshrc");
    let wrapped = isolated_zsh()
        .args([
            "-lic",
            "[[ $INTERFACE_TEST_USER_RC == loaded && ! -o RCS ]] || exit 22; whence -w claude",
        ])
        .env("HOME", &home)
        .env("ZDOTDIR", &overlay)
        .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
        .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &home)
        .env(CLAUDE_MANUAL_HOST_ENV, "/definitely/not/invoked")
        .output()
        .expect("launch wrapped zsh");

    assert!(wrapped.status.success());
    assert_eq!(
        String::from_utf8_lossy(&wrapped.stdout).trim(),
        "claude: function"
    );
}

#[cfg(unix)]
#[test]
fn manual_codex_overlay_adds_titles_and_preserves_user_overrides() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().expect("temporary directory");
    let home = temporary.path().join("home");
    let runtime = temporary.path().join("runtime");
    std::fs::create_dir(&home).expect("home");
    std::fs::create_dir(&runtime).expect("runtime");
    let codex = home.join("codex");
    std::fs::write(&codex, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").expect("fake CLI");
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o700)).expect("executable");
    let overlay = install_manual_claude_zsh_overlay(&runtime).expect("overlay");
    for (startup, expected) in [
        (
            "",
            "-c\ntui.terminal_title=[\"spinner\",\"app-name\",\"fast-mode\",\"model\",\"status\",\"thread\"]\n-c\ntui.keymap.global.toggle_fast_mode=[\"shift-f12\"]\n-c\ntui.notification_method=osc9\nresume\nchat-id\n",
        ),
        ("function codex { print custom-codex; }\n", "custom-codex\n"),
        (
            "alias codex='print custom-alias'\n",
            "custom-alias resume chat-id\n",
        ),
    ] {
        std::fs::write(home.join(".zshrc"), startup).expect("startup fixture");
        let output = isolated_zsh()
            .args(["-lic", "codex resume chat-id"])
            .env("HOME", &home)
            .env("ZDOTDIR", &overlay)
            .env("PATH", format!("{}:/usr/bin:/bin", home.display()))
            .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
            .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &home)
            .output()
            .expect("isolated zsh");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }

    // With a host-resolved Codex, bare `codex` goes through the capture
    // wrapper instead, carrying the user's arguments untouched.
    let host = home.join("host");
    std::fs::write(&host, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").expect("fake host");
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o700)).expect("executable");
    std::fs::write(home.join(".zshrc"), "").expect("startup fixture");
    let output = isolated_zsh()
        .args(["-lic", "codex resume chat-id"])
        .env("HOME", &home)
        .env("ZDOTDIR", &overlay)
        .env("PATH", format!("{}:/usr/bin:/bin", home.display()))
        .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
        .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &home)
        .env(CLAUDE_MANUAL_HOST_ENV, &host)
        .env(CODEX_MANUAL_EXECUTABLE_ENV, &codex)
        .output()
        .expect("isolated zsh");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("{CODEX_MANUAL_WRAPPER_MODE}\nresume\nchat-id\n")
    );
}

#[cfg(unix)]
#[test]
fn manual_claude_zsh_overlay_wraps_when_zshenv_disables_later_startup_files() {
    let shell = Path::new("/bin/zsh");
    if !shell.is_file() {
        return;
    }
    let temporary = tempfile::tempdir().expect("temporary directory");
    let runtime = temporary.path().join("runtime");
    let home = temporary.path().join("home");
    std::fs::create_dir(&runtime).expect("runtime directory");
    std::fs::create_dir(&home).expect("home directory");
    std::fs::write(
        home.join(".zshenv"),
        "export INTERFACE_TEST_USER_ZSHENV=loaded\nunsetopt RCS\n",
    )
    .expect("user zshenv");
    std::fs::write(
        home.join(".zshrc"),
        "export INTERFACE_TEST_USER_ZSHRC=unexpected\n",
    )
    .expect("user zshrc");
    let overlay = install_manual_claude_zsh_overlay(&runtime).expect("zsh overlay");

    let output = isolated_zsh()
        .args([
            "-lic",
            "[[ $INTERFACE_TEST_USER_ZSHENV == loaded && -z ${INTERFACE_TEST_USER_ZSHRC:-} && ! -o RCS && $ZDOTDIR == $HOME ]] || exit 23; whence -w claude",
        ])
        .env("HOME", &home)
        .env("ZDOTDIR", &overlay)
        .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
        .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &home)
        .env(CLAUDE_MANUAL_HOST_ENV, "/definitely/not/invoked")
        .output()
        .expect("launch isolated zsh");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "claude: function"
    );
}

#[cfg(unix)]
#[test]
fn manual_claude_zsh_overlay_tracks_unset_zdotdir_between_startup_files() {
    let shell = Path::new("/bin/zsh");
    if !shell.is_file() {
        return;
    }
    let temporary = tempfile::tempdir().expect("temporary directory");
    let runtime = temporary.path().join("runtime");
    let home = temporary.path().join("home");
    let custom = temporary.path().join("custom-zdotdir");
    for directory in [&runtime, &home, &custom] {
        std::fs::create_dir(directory).expect("fixture directory");
    }
    let overlay = install_manual_claude_zsh_overlay(&runtime).expect("zsh overlay");
    std::fs::write(
        custom.join(".zshenv"),
        "export INTERFACE_TEST_CUSTOM_ZSHENV=loaded\nunset ZDOTDIR\n",
    )
    .expect("custom zshenv");
    std::fs::write(
        home.join(".zprofile"),
        "export INTERFACE_TEST_HOME_ZPROFILE=loaded\n",
    )
    .expect("home zprofile");
    std::fs::write(
        home.join(".zshrc"),
        "export INTERFACE_TEST_HOME_ZSHRC=loaded\n",
    )
    .expect("home zshrc");

    let after_zshenv = isolated_zsh()
        .args([
            "-lic",
            "[[ $INTERFACE_TEST_CUSTOM_ZSHENV == loaded && $INTERFACE_TEST_HOME_ZPROFILE == loaded && $INTERFACE_TEST_HOME_ZSHRC == loaded && $ZDOTDIR == $HOME ]] || exit 24; whence -w claude",
        ])
        .env("HOME", &home)
        .env("ZDOTDIR", &overlay)
        .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
        .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &custom)
        .env(CLAUDE_MANUAL_HOST_ENV, "/definitely/not/invoked")
        .output()
        .expect("launch zsh after zshenv unset");
    assert!(after_zshenv.status.success());
    assert_eq!(
        String::from_utf8_lossy(&after_zshenv.stdout).trim(),
        "claude: function"
    );

    std::fs::write(
        custom.join(".zshenv"),
        "export INTERFACE_TEST_CUSTOM_ZSHENV=loaded\n",
    )
    .expect("custom zshenv");
    std::fs::write(
        custom.join(".zprofile"),
        "export INTERFACE_TEST_CUSTOM_ZPROFILE=loaded\nunset ZDOTDIR\n",
    )
    .expect("custom zprofile");
    std::fs::write(
        home.join(".zshrc"),
        "export INTERFACE_TEST_HOME_ZSHRC_AFTER_PROFILE=loaded\n",
    )
    .expect("home zshrc");

    let after_zprofile = isolated_zsh()
        .args([
            "-lic",
            "[[ $INTERFACE_TEST_CUSTOM_ZSHENV == loaded && $INTERFACE_TEST_CUSTOM_ZPROFILE == loaded && $INTERFACE_TEST_HOME_ZSHRC_AFTER_PROFILE == loaded && $ZDOTDIR == $HOME ]] || exit 25; whence -w claude",
        ])
        .env("HOME", &home)
        .env("ZDOTDIR", &overlay)
        .env(CLAUDE_MANUAL_OVERLAY_ZDOTDIR_ENV, &overlay)
        .env(CLAUDE_MANUAL_USER_ZDOTDIR_ENV, &custom)
        .env(CLAUDE_MANUAL_HOST_ENV, "/definitely/not/invoked")
        .output()
        .expect("launch zsh after zprofile unset");
    assert!(after_zprofile.status.success());
    assert_eq!(
        String::from_utf8_lossy(&after_zprofile.stdout).trim(),
        "claude: function"
    );
}

#[cfg(unix)]
#[test]
fn inherited_overlay_zdotdir_falls_back_to_the_real_home() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().expect("temporary directory");
    let home = temporary.path().join("home");
    let overlay = temporary.path().join("overlay");
    let overlay_alias = temporary.path().join("overlay-alias");
    let custom = temporary.path().join("custom-zdotdir");
    for directory in [&home, &overlay, &custom] {
        std::fs::create_dir(directory).expect("fixture directory");
    }
    symlink(&overlay, &overlay_alias).expect("overlay alias");

    assert_eq!(
        manual_claude_user_zdotdir(&home, &overlay, Some(overlay.clone())),
        home
    );
    assert_eq!(
        manual_claude_user_zdotdir(&home, &overlay, Some(overlay_alias)),
        home
    );
    assert_eq!(
        manual_claude_user_zdotdir(&home, &overlay, Some(custom.clone())),
        custom
    );
    assert_eq!(
        manual_claude_user_zdotdir(&home, &overlay, Some(PathBuf::from("relative"))),
        home
    );
}

#[cfg(unix)]
#[test]
fn non_zsh_login_shell_does_not_arm_manual_claude_capture() {
    use std::os::unix::fs::symlink;

    const CAPTURE_TOKEN: &str = "11111111-1111-4111-8111-111111111111";

    let temporary = tempfile::tempdir().expect("temporary directory");
    let home = temporary.path().join("home");
    let disguised_directory = temporary.path().join("disguised-shell");
    std::fs::create_dir(&home).expect("home directory");
    std::fs::create_dir(&disguised_directory).expect("disguised shell directory");
    let disguised_zsh = disguised_directory.join("zsh");
    symlink("/bin/sh", &disguised_zsh).expect("non-zsh target with a zsh basename");
    assert!(!zsh_login_shell(&disguised_zsh));
    if Path::new("/bin/zsh").is_file() {
        let renamed_zsh = temporary.path().join("renamed-shell");
        symlink("/bin/zsh", &renamed_zsh).expect("renamed zsh link");
        assert!(zsh_login_shell(&renamed_zsh));
    }
    let claude = executable_script(temporary.path(), "claude", "#!/bin/sh\nexit 0\n");
    let missing = temporary.path().join("missing");
    let catalog = terminal_launch_catalog(
        temporary.path(),
        &home,
        &disguised_zsh,
        &claude,
        &missing,
        &missing,
        &missing,
        &missing,
        &missing,
        &missing,
        &missing,
        &missing,
        &home.join(".kimi-code"),
        &temporary.path().join("receipts"),
        Path::new("/tmp/iface-test/panes/iface.sock"),
    )
    .expect("terminal launch catalog");

    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Claude));
    assert!(!catalog.captures_provider_session(TerminalLaunchProfileId::Shell));
    // No manual capture, but every shell still carries the pane socket.
    assert_eq!(
        catalog
            .resolve_with_peers(None, None, false, Some(CAPTURE_TOKEN))
            .expect("ordinary login shell"),
        ResolvedTerminalLaunch::LoginShellWithEnvironment {
            environment: vec![
                (
                    OsString::from(PANE_MCP_SOCKET_ENV),
                    OsString::from("/tmp/iface-test/panes/iface.sock"),
                ),
                (OsString::from("AGY_CLI_OSC7501"), OsString::from("on")),
            ],
        }
    );
}

#[cfg(unix)]
#[test]
fn nested_recovery_allows_only_canonical_authority_descendants() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("project");
    let child = root.join("nested");
    let file = root.join("not-a-directory");
    let sibling = temporary.path().join("project-sibling");
    std::fs::create_dir_all(&child).expect("nested project directory");
    std::fs::write(&file, b"fixture").expect("file fixture");
    std::fs::create_dir(&sibling).expect("sibling directory");
    let root = std::fs::canonicalize(root).expect("canonical root");
    let child = std::fs::canonicalize(child).expect("canonical child");
    let file = std::fs::canonicalize(file).expect("canonical file");
    let sibling = std::fs::canonicalize(sibling).expect("canonical sibling");
    let lexical_alias = child.join("..");
    let child_alias = root.join("nested-alias");
    let escaping_alias = root.join("escaping-alias");
    let removed = root.join("removed");
    symlink(&child, &child_alias).expect("child alias");
    symlink(&sibling, &escaping_alias).expect("escaping alias");

    assert!(recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        child.to_str().expect("UTF-8 child"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        child.to_str().expect("UTF-8 child"),
        false,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        file.to_str().expect("UTF-8 file"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        sibling.to_str().expect("UTF-8 sibling"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        lexical_alias.to_str().expect("UTF-8 alias"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        child_alias.to_str().expect("UTF-8 child alias"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        root.to_str().expect("UTF-8 root"),
        escaping_alias.to_str().expect("UTF-8 escaping alias"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        file.to_str().expect("UTF-8 file"),
        file.to_str().expect("UTF-8 file"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        child_alias.to_str().expect("UTF-8 child alias"),
        child_alias.to_str().expect("UTF-8 child alias"),
        true,
    ));
    assert!(!recovered_cwd_matches_authority(
        root.to_str().expect("UTF-8 root"),
        removed.to_str().expect("UTF-8 removed path"),
        removed.to_str().expect("UTF-8 removed path"),
        true,
    ));
    assert!(
        recovered_cwd_matches_authority(
            root.to_str().expect("UTF-8 root"),
            removed.to_str().expect("UTF-8 removed path"),
            removed.to_str().expect("UTF-8 removed path"),
            false,
        ),
        "direct captures retain their exact-string authority behavior"
    );
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)] // One end-to-end recovery, kept whole.
async fn nested_manual_claude_crash_recovery_persists_the_child_cwd() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let home = temporary.path().join("home");
    let root = home.join("Desktop/Projects/project");
    let child = root.join("nested");
    std::fs::create_dir_all(&child).expect("nested project directory");
    std::fs::create_dir(home.join(".claude")).expect("customization directory");
    let root = std::fs::canonicalize(root).expect("canonical root");
    let child = std::fs::canonicalize(child).expect("canonical child");
    let root_text = root.to_string_lossy().into_owned();
    let child_text = child.to_string_lossy().into_owned();
    let terminal_id = TerminalId::random();
    let shell_launch = TerminalLaunchRequestV1 {
        version: TERMINAL_LAUNCH_REQUEST_VERSION,
        profile_id: TerminalLaunchProfileId::Shell,
    };
    let desktop = DesktopService::open(
        DesktopServiceConfig {
            appearance_file: state.join("appearance.json"),
            acp_profiles_file: state.join("acp-profiles.json"),
            terminal_workspaces_file: state.join("terminal-workspaces.json"),
            home_directory: home.clone(),
            customization_user_root: Some(home.join(".claude")),
            review_checkpoint_store_path: Some(state.join("review-checkpoints")),
        },
        DesktopServiceReviewPorts::default(),
    )
    .expect("desktop service");
    desktop
        .record_terminal_reopen_authority(&TerminalSummary {
            id: terminal_id,
            generation: TerminalGeneration::initial(),
            title: "Shell".to_owned(),
            root: root_text.clone(),
            cwd: root_text.clone(),
            session_key: None,
            launch: Some(shell_launch),
            launch_session: None,
            state: TerminalState::Running,
            size: TerminalSize::default(),
            created_at: "2026-09-03T00:00:00.000Z".to_owned(),
            updated_at: "2026-09-03T00:00:00.000Z".to_owned(),
            exit: None,
            passed_off_from: None,
            fast_mode: None,
            startup_download: None,
            attention: None,
            replaces: None,
        })
        .expect("shell reopen authority");
    desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces":[{
                "id":"workspace-1",
                "name":"Agents",
                "createdAt":"2026-09-03T00:00:00.000Z",
                "updatedAt":"2026-09-03T00:00:00.000Z",
                "terminals":[{
                    "id":terminal_id,
                    "title":"Shell",
                    "cwd":root_text,
                    "launch":{"version":1,"profileId":"shell"}
                }]
            }]}),
        )
        .expect("saved shell roster");
    let confirmed = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some("43434343-4343-4343-8343-434343434343".to_owned()),
        resume: true,
    };
    let recovered = crate::provider_capture::RecoveredClaudeTerminalSession {
        terminal_id,
        terminal_generation: TerminalGeneration::initial(),
        root: root_text.clone(),
        cwd: child_text.clone(),
        prior_launch_session: None,
        launch_session: confirmed.clone(),
        profile_id: TerminalLaunchProfileId::Claude,
        nested_shell: true,
        nested_replacement: false,
    };

    assert_eq!(
        reconcile_recovered_claude_terminal_session(&desktop, &recovered),
        StartupReceiptDisposition::Durable
    );
    let TerminalReopenAuthorityState::Authority(authority) =
        desktop.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("recovered nested Claude identity must remain authoritative");
    };
    assert_eq!(authority.root, root_text);
    assert_eq!(authority.terminal.cwd, child_text);
    assert_eq!(
        authority.terminal.launch,
        Some(TerminalLaunchRequestV1 {
            version: TERMINAL_LAUNCH_REQUEST_VERSION,
            profile_id: TerminalLaunchProfileId::Claude,
        })
    );
    assert_eq!(authority.terminal.launch_session, Some(confirmed));
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)] // One recovery scenario pins the full nested replacement state.
async fn nested_manual_claude_replacement_recovers_over_a_durable_shell_demotion() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let home = temporary.path().join("home");
    let root = home.join("Desktop/Projects/project");
    let child = root.join("nested");
    std::fs::create_dir_all(&child).expect("nested project directory");
    std::fs::create_dir(home.join(".claude")).expect("customization directory");
    let root = std::fs::canonicalize(root).expect("canonical root");
    let child = std::fs::canonicalize(child).expect("canonical child");
    let root_text = root.to_string_lossy().into_owned();
    let child_text = child.to_string_lossy().into_owned();
    let terminal_id = TerminalId::random();
    let shell_launch = TerminalLaunchRequestV1 {
        version: TERMINAL_LAUNCH_REQUEST_VERSION,
        profile_id: TerminalLaunchProfileId::Shell,
    };
    let prior = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some("45454545-4545-4545-8545-454545454545".to_owned()),
        resume: true,
    };
    let desktop = DesktopService::open(
        DesktopServiceConfig {
            appearance_file: state.join("appearance.json"),
            acp_profiles_file: state.join("acp-profiles.json"),
            terminal_workspaces_file: state.join("terminal-workspaces.json"),
            home_directory: home.clone(),
            customization_user_root: Some(home.join(".claude")),
            review_checkpoint_store_path: Some(state.join("review-checkpoints")),
        },
        DesktopServiceReviewPorts::default(),
    )
    .expect("desktop service");
    desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces":[{
                "id":"workspace-1",
                "name":"Agents",
                "createdAt":"2026-09-03T00:00:00.000Z",
                "updatedAt":"2026-09-03T00:00:00.000Z",
                "terminals":[{
                    "id":terminal_id,
                    "title":"Claude",
                    "cwd":root_text,
                    "launch":{"version":1,"profileId":"claude"},
                    "launchSession":prior
                }]
            }]}),
        )
        .expect("saved predecessor roster");
    desktop
        .record_terminal_reopen_authority(&TerminalSummary {
            id: terminal_id,
            generation: TerminalGeneration::initial(),
            title: "Shell".to_owned(),
            root: root_text.clone(),
            cwd: root_text.clone(),
            session_key: None,
            launch: Some(shell_launch),
            launch_session: None,
            state: TerminalState::Running,
            size: TerminalSize::default(),
            created_at: "2026-09-03T00:00:00.000Z".to_owned(),
            updated_at: "2026-09-03T00:00:00.000Z".to_owned(),
            exit: None,
            passed_off_from: None,
            fast_mode: None,
            startup_download: None,
            attention: None,
            replaces: None,
        })
        .expect("simulate authority-first shell demotion crash winner");
    let replacement = TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some("56565656-5656-4656-8656-565656565656".to_owned()),
        resume: true,
    };
    let recovered = crate::provider_capture::RecoveredClaudeTerminalSession {
        terminal_id,
        terminal_generation: TerminalGeneration::initial(),
        root: root_text.clone(),
        cwd: child_text.clone(),
        prior_launch_session: Some(prior.clone()),
        launch_session: replacement.clone(),
        profile_id: TerminalLaunchProfileId::Claude,
        nested_shell: true,
        nested_replacement: true,
    };

    assert_eq!(
        reconcile_recovered_claude_terminal_session(&desktop, &recovered),
        StartupReceiptDisposition::Durable
    );
    let TerminalReopenAuthorityState::Authority(authority) =
        desktop.terminal_reopen_authority_state(terminal_id)
    else {
        panic!("proven replacement must supersede the durable shell demotion");
    };
    assert_eq!(authority.root, root_text);
    assert_eq!(authority.terminal.cwd, child_text);
    assert_eq!(
        authority.terminal.launch.map(|launch| launch.profile_id),
        Some(TerminalLaunchProfileId::Claude)
    );
    assert_eq!(authority.terminal.launch_session, Some(replacement));
    let saved = desktop
        .dispatch_blocking(CHANNELS.get_terminal_workspaces, serde_json::Value::Null)
        .expect("saved replacement roster");
    assert_eq!(
        saved[0]["terminals"][0]["launchSession"]["id"],
        "56565656-5656-4656-8656-565656565656"
    );
    assert_ne!(
        saved[0]["terminals"][0]["launchSession"]["id"],
        prior.id.expect("prior session id")
    );
}

/// Antigravity, Muse and `OpenCode` panes can move to another conversation after
/// their first one is saved (a new Antigravity conversation, Muse
/// clear/resume/fork, an `OpenCode` session switch). The host captures the new
/// id, so it must reach the saved roster: otherwise the capture can never be
/// retired and every later reopen of the pane is refused. Production 21 Oct:
/// an Antigravity pane's saved id stayed one conversation behind its restart
/// proof and the pane never came back.
#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)] // One scenario per captured provider, live and startup paths.
async fn changed_native_provider_session_reaches_the_saved_roster() {
    for (profile, first, second) in [
        (
            TerminalLaunchProfileId::Gemini,
            "agy-c0944a23-c993-4b46-ab82-e0780c239b11",
            "agy-c189aadb-a383-4d50-a846-4a22e17c7082",
        ),
        (
            TerminalLaunchProfileId::Muse,
            "muse-01a15b88-9662-7ce0-b98e-0c2b512a773a",
            "muse-01a15c2b-b8e0-7a70-9750-da934a463e06",
        ),
        (
            TerminalLaunchProfileId::Opencode,
            "ses_firstSession01",
            "ses_secondSession02",
        ),
    ] {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let state = temporary.path().join("state");
        let home = temporary.path().join("home");
        let root = home.join("Developer/Projects/project");
        std::fs::create_dir_all(&root).expect("project directory");
        std::fs::create_dir(home.join(".claude")).expect("customization directory");
        let root = std::fs::canonicalize(root).expect("canonical root");
        let root_text = root.to_string_lossy().into_owned();
        let desktop = DesktopService::open(
            DesktopServiceConfig {
                appearance_file: state.join("appearance.json"),
                acp_profiles_file: state.join("acp-profiles.json"),
                terminal_workspaces_file: state.join("terminal-workspaces.json"),
                home_directory: home.clone(),
                customization_user_root: Some(home.join(".claude")),
                review_checkpoint_store_path: Some(state.join("review-checkpoints")),
            },
            DesktopServiceReviewPorts::default(),
        )
        .expect("desktop service");
        let session = |id: &str| TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some(id.to_owned()),
            resume: true,
        };
        let launch = TerminalLaunchRequestV1 {
            version: TERMINAL_LAUNCH_REQUEST_VERSION,
            profile_id: profile,
        };
        let summary = |id: TerminalId, binding: &str| TerminalSummary {
            id,
            generation: TerminalGeneration::initial(),
            title: "Agent".to_owned(),
            root: root_text.clone(),
            cwd: root_text.clone(),
            session_key: None,
            launch: Some(launch),
            launch_session: Some(session(binding)),
            state: TerminalState::Running,
            size: TerminalSize::default(),
            created_at: "2026-10-20T21:08:19.000Z".to_owned(),
            updated_at: "2026-10-20T21:08:19.000Z".to_owned(),
            exit: None,
            passed_off_from: None,
            fast_mode: None,
            startup_download: None,
            attention: None,
            replaces: None,
        };
        let live = TerminalId::random();
        let recovered_id = TerminalId::random();
        let entry = |id: TerminalId| {
            json!({
                "id": id,
                "title": "Agent",
                "cwd": root_text,
                "launch": launch,
                "launchSession": session(first),
            })
        };
        desktop
            .dispatch_blocking(
                CHANNELS.set_terminal_workspaces,
                json!({"workspaces":[{
                    "id":"workspace-1",
                    "name":"Agents",
                    "createdAt":"2026-10-20T21:08:19.000Z",
                    "updatedAt":"2026-10-20T21:08:19.000Z",
                    "terminals":[entry(live), entry(recovered_id)]
                }]}),
            )
            .expect("saved first conversation");
        for id in [live, recovered_id] {
            desktop
                .record_terminal_reopen_authority(&summary(id, first))
                .expect("first conversation's restart proof");
        }
        let saved_binding = |id: TerminalId| {
            let saved = desktop
                .dispatch_blocking(CHANNELS.get_terminal_workspaces, serde_json::Value::Null)
                .expect("saved roster");
            saved[0]["terminals"]
                .as_array()
                .expect("terminals")
                .iter()
                .find(|terminal| terminal["id"] == json!(id))
                .expect("saved pane")["launchSession"]["id"]
                .clone()
        };

        // Live: the running pane reports its new conversation.
        assert_eq!(
            desktop
                .persist_terminal_provider_reopen(&summary(live, second))
                .unwrap_or_else(|error| panic!("{profile:?}: live capture: {error:?}")),
            SavedTerminalProviderSessionPersistence::Durable,
            "{profile:?}"
        );
        assert_eq!(saved_binding(live), second, "{profile:?}");

        // Startup: an earlier host advanced the restart proof but not the
        // roster, and left the capture behind for the next launch to file.
        desktop
            .record_terminal_reopen_authority(&summary(recovered_id, second))
            .expect("restart proof ahead of the roster");
        let recovered = crate::provider_capture::RecoveredClaudeTerminalSession {
            terminal_id: recovered_id,
            terminal_generation: TerminalGeneration::initial(),
            root: root_text.clone(),
            cwd: root_text.clone(),
            prior_launch_session: Some(session(first)),
            launch_session: session(second),
            profile_id: profile,
            nested_shell: false,
            nested_replacement: false,
        };
        assert_eq!(
            reconcile_recovered_claude_terminal_session(&desktop, &recovered),
            StartupReceiptDisposition::Durable,
            "{profile:?}"
        );
        assert_eq!(saved_binding(recovered_id), second, "{profile:?}");
        let TerminalReopenAuthorityState::Authority(authority) =
            desktop.terminal_reopen_authority_state(recovered_id)
        else {
            panic!("{profile:?}: the recovered pane keeps its restart proof");
        };
        assert_eq!(authority.terminal.launch_session, Some(session(second)));
    }
}

/// Every provider the host captures a session for can move a saved pane to
/// another conversation, and that move must reach the saved roster through
/// the live and the startup path. The cases come from the host's captured set,
/// so a provider added there without a reopen case fails here: Kimi Code was
/// made a captured provider without being allowed to move a saved pane, and
/// its panes never reopened after a new chat.
#[cfg(unix)]
#[tokio::test]
async fn every_captured_provider_session_change_reaches_the_saved_roster() {
    // Exhaustive on purpose: a new profile does not compile until it says
    // whether it is captured and, if so, which two sessions it moves between.
    let sample = |profile: TerminalLaunchProfileId| match profile {
        TerminalLaunchProfileId::Claude => Some((
            "0a7c4f3e-1d2b-4c5a-8e6f-7a8b9c0d1e2f",
            "1b8d5a4f-2e3c-4d6b-9f7a-8b9c0d1e2f3a",
        )),
        TerminalLaunchProfileId::Codex => Some((
            "2c9e6b5a-3f4d-4e7c-8a8b-9c0d1e2f3a4b",
            "3d0f7c6b-4a5e-4f8d-9b9c-0d1e2f3a4b5c",
        )),
        TerminalLaunchProfileId::Gemini => Some((
            "agy-c0944a23-c993-4b46-ab82-e0780c239b11",
            "agy-c189aadb-a383-4d50-a846-4a22e17c7082",
        )),
        TerminalLaunchProfileId::Muse => Some((
            "muse-01a15b88-9662-7ce0-b98e-0c2b512a773a",
            "muse-01a15c2b-b8e0-7a70-9750-da934a463e06",
        )),
        TerminalLaunchProfileId::Opencode => Some(("ses_firstSession01", "ses_secondSession02")),
        TerminalLaunchProfileId::Kimi => Some((
            "kimi-5f1e2d3c-4b5a-4697-8877-665544332211",
            "kimi-6a2f3e4d-5c6b-4708-9988-776655443322",
        )),
        TerminalLaunchProfileId::Shell
        | TerminalLaunchProfileId::Grok
        | TerminalLaunchProfileId::Qwen
        | TerminalLaunchProfileId::Omp => None,
    };
    let profiles = [
        TerminalLaunchProfileId::Shell,
        TerminalLaunchProfileId::Claude,
        TerminalLaunchProfileId::Codex,
        TerminalLaunchProfileId::Grok,
        TerminalLaunchProfileId::Qwen,
        TerminalLaunchProfileId::Gemini,
        TerminalLaunchProfileId::Omp,
        TerminalLaunchProfileId::Opencode,
        TerminalLaunchProfileId::Muse,
        TerminalLaunchProfileId::Kimi,
    ];
    let mut checked = 0;
    for profile in profiles {
        let captured = crate::provider_capture::captured_profile(profile);
        assert_eq!(
            sample(profile).is_some(),
            captured,
            "{profile:?}: give every captured provider a reopen case"
        );
        if let Some((first, second)) = sample(profile) {
            assert_provider_session_change_reaches_the_saved_roster(profile, first, second);
            checked += 1;
        }
    }
    assert!(checked >= 6);
}

#[cfg(unix)]
#[allow(clippy::too_many_lines)] // The live and the startup path, one scenario.
fn assert_provider_session_change_reaches_the_saved_roster(
    profile: TerminalLaunchProfileId,
    first: &str,
    second: &str,
) {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let state = temporary.path().join("state");
    let home = temporary.path().join("home");
    let root = home.join("Developer/Projects/project");
    std::fs::create_dir_all(&root).expect("project directory");
    std::fs::create_dir(home.join(".claude")).expect("customization directory");
    let root = std::fs::canonicalize(root).expect("canonical root");
    let root_text = root.to_string_lossy().into_owned();
    let desktop = DesktopService::open(
        DesktopServiceConfig {
            appearance_file: state.join("appearance.json"),
            acp_profiles_file: state.join("acp-profiles.json"),
            terminal_workspaces_file: state.join("terminal-workspaces.json"),
            home_directory: home.clone(),
            customization_user_root: Some(home.join(".claude")),
            review_checkpoint_store_path: Some(state.join("review-checkpoints")),
        },
        DesktopServiceReviewPorts::default(),
    )
    .expect("desktop service");
    let session = |id: &str| TerminalLaunchSessionV1 {
        version: TERMINAL_LAUNCH_SESSION_VERSION,
        id: Some(id.to_owned()),
        resume: true,
    };
    let launch = TerminalLaunchRequestV1 {
        version: TERMINAL_LAUNCH_REQUEST_VERSION,
        profile_id: profile,
    };
    let summary = |id: TerminalId, binding: &str| TerminalSummary {
        id,
        generation: TerminalGeneration::initial(),
        title: "Agent".to_owned(),
        root: root_text.clone(),
        cwd: root_text.clone(),
        session_key: None,
        launch: Some(launch),
        launch_session: Some(session(binding)),
        state: TerminalState::Running,
        size: TerminalSize::default(),
        created_at: "2026-10-20T21:08:19.000Z".to_owned(),
        updated_at: "2026-10-20T21:08:19.000Z".to_owned(),
        exit: None,
        passed_off_from: None,
        fast_mode: None,
        startup_download: None,
        attention: None,
        replaces: None,
    };
    let live = TerminalId::random();
    let recovered_id = TerminalId::random();
    let entry = |id: TerminalId| {
        json!({
            "id": id,
            "title": "Agent",
            "cwd": root_text,
            "launch": launch,
            "launchSession": session(first),
        })
    };
    desktop
        .dispatch_blocking(
            CHANNELS.set_terminal_workspaces,
            json!({"workspaces":[{
                "id":"workspace-1",
                "name":"Agents",
                "createdAt":"2026-10-20T21:08:19.000Z",
                "updatedAt":"2026-10-20T21:08:19.000Z",
                "terminals":[entry(live), entry(recovered_id)]
            }]}),
        )
        .expect("saved first conversation");
    for id in [live, recovered_id] {
        desktop
            .record_terminal_reopen_authority(&summary(id, first))
            .expect("first conversation's restart proof");
    }
    let saved_binding = |id: TerminalId| {
        let saved = desktop
            .dispatch_blocking(CHANNELS.get_terminal_workspaces, serde_json::Value::Null)
            .expect("saved roster");
        saved[0]["terminals"]
            .as_array()
            .expect("terminals")
            .iter()
            .find(|terminal| terminal["id"] == json!(id))
            .expect("saved pane")["launchSession"]["id"]
            .clone()
    };

    // Live: the running pane reports its new conversation.
    assert_eq!(
        desktop
            .persist_terminal_provider_reopen(&summary(live, second))
            .unwrap_or_else(|error| panic!("{profile:?}: live capture: {error:?}")),
        SavedTerminalProviderSessionPersistence::Durable,
        "{profile:?}"
    );
    assert_eq!(saved_binding(live), second, "{profile:?}");

    // Startup: an earlier host advanced the restart proof but not the
    // roster, and left the capture behind for the next launch to file.
    desktop
        .record_terminal_reopen_authority(&summary(recovered_id, second))
        .expect("restart proof ahead of the roster");
    let recovered = crate::provider_capture::RecoveredClaudeTerminalSession {
        terminal_id: recovered_id,
        terminal_generation: TerminalGeneration::initial(),
        root: root_text.clone(),
        cwd: root_text.clone(),
        prior_launch_session: Some(session(first)),
        launch_session: session(second),
        profile_id: profile,
        nested_shell: false,
        nested_replacement: false,
    };
    assert_eq!(
        reconcile_recovered_claude_terminal_session(&desktop, &recovered),
        StartupReceiptDisposition::Durable,
        "{profile:?}"
    );
    assert_eq!(saved_binding(recovered_id), second, "{profile:?}");
    let TerminalReopenAuthorityState::Authority(authority) =
        desktop.terminal_reopen_authority_state(recovered_id)
    else {
        panic!("{profile:?}: the recovered pane keeps its restart proof");
    };
    assert_eq!(authority.terminal.launch_session, Some(session(second)));
}

// Three curated resolutions now each carry the pane cwd argument, which puts
// this single-catalog assertion three lines over the default limit.
#[allow(clippy::too_many_lines)]
#[cfg(unix)]
#[test]
fn terminal_launch_catalog_exposes_only_canonical_curated_programs() {
    const CAPTURE_TOKEN: &str = "11111111-1111-4111-8111-111111111111";

    let temporary = tempfile::tempdir().expect("temporary directory");
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).expect("home directory");
    let claude = executable_script(temporary.path(), "claude", "#!/bin/sh\nexit 0\n");
    let codex = executable_script(temporary.path(), "codex", "#!/bin/sh\nexit 0\n");
    let grok = executable_script(temporary.path(), "grok", "#!/bin/sh\nexit 0\n");
    let qwen = executable_script(temporary.path(), "qwen", "#!/bin/sh\nexit 0\n");
    let antigravity = executable_script(temporary.path(), "agy", "#!/bin/sh\nexit 0\n");
    let omp = executable_script(temporary.path(), "omp", "#!/bin/sh\nexit 0\n");
    let opencode = executable_script(temporary.path(), "opencode", "#!/bin/sh\nexit 0\n");
    let muse = executable_script(temporary.path(), "muse", "#!/bin/sh\nexit 0\n");
    let kimi = executable_script(temporary.path(), "kimi", "#!/bin/sh\nexit 0\n");
    let receipts = temporary.path().join("receipts");

    let catalog = terminal_launch_catalog(
        temporary.path(),
        &home,
        Path::new("/bin/zsh"),
        &claude,
        &codex,
        &grok,
        &qwen,
        &antigravity,
        &omp,
        &opencode,
        &muse,
        &kimi,
        &home.join(".kimi-code"),
        &receipts,
        Path::new("/tmp/iface-test/panes/iface.sock"),
    )
    .expect("terminal launch catalog");
    assert_eq!(
        catalog
            .profiles()
            .into_iter()
            .map(|profile| (profile.id, profile.available))
            .collect::<Vec<_>>(),
        vec![
            (TerminalLaunchProfileId::Shell, true),
            (TerminalLaunchProfileId::Claude, true),
            (TerminalLaunchProfileId::Codex, true),
            (TerminalLaunchProfileId::Grok, true),
            (TerminalLaunchProfileId::Gemini, true),
            (TerminalLaunchProfileId::Omp, true),
            (TerminalLaunchProfileId::Opencode, true),
            (TerminalLaunchProfileId::Muse, true),
            (TerminalLaunchProfileId::Kimi, true),
        ]
    );
    assert!(matches!(
        catalog.resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Qwen,
            }),
            None,
            false,
            None,
        ),
        Ok(ResolvedTerminalLaunch::Program { .. })
    ));

    let resolved = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Grok,
            }),
            None,
            false,
            None,
        )
        .expect("curated Grok launch");
    let ResolvedTerminalLaunch::Program {
        executable,
        arguments,
        environment,
    } = resolved
    else {
        panic!("Grok must resolve to a direct program");
    };
    assert_eq!(
        executable,
        std::fs::canonicalize(grok).expect("canonical Grok executable")
    );
    // `--no-leader` keeps Grok's MCP servers under the pane so the pane
    // server can name its caller; the socket rides the environment for the
    // static config entry Grok reads.
    assert_eq!(arguments, vec![OsString::from("--no-leader")]);
    assert_eq!(environment.len(), 3);
    assert_eq!(environment[0].0, OsString::from("PATH"));
    assert!(
        std::env::split_paths(&environment[0].1).any(|entry| entry == temporary.path()),
        "the fixed executable directory must be present in the launch PATH"
    );
    assert_eq!(
        environment[1],
        (
            OsString::from(PANE_MCP_SOCKET_ENV),
            OsString::from("/tmp/iface-test/panes/iface.sock"),
        )
    );
    assert_eq!(
        environment[2],
        (OsString::from("AGY_CLI_OSC7501"), OsString::from("on"))
    );

    let resolved = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Gemini,
            }),
            None,
            false,
            None,
        )
        .expect("curated Antigravity launch");
    let ResolvedTerminalLaunch::Program {
        executable,
        arguments,
        environment,
    } = resolved
    else {
        panic!("the stable Gemini profile must resolve to Antigravity");
    };
    assert_eq!(
        executable,
        std::fs::canonicalize(antigravity).expect("canonical Antigravity executable")
    );
    assert!(arguments.is_empty());
    assert_eq!(environment.len(), 3);
    assert_eq!(environment[1].0, OsString::from(PANE_MCP_SOCKET_ENV));
    assert_eq!(
        environment[2],
        (OsString::from("AGY_CLI_OSC7501"), OsString::from("on"))
    );
    assert!(
        std::env::split_paths(&environment[0].1).any(|entry| entry == temporary.path()),
        "the Antigravity executable directory must be present in the launch PATH"
    );

    assert_eq!(
        catalog.executable(TerminalLaunchProfileId::Claude),
        Some(claude.as_path())
    );
    assert_eq!(
        catalog.executable(TerminalLaunchProfileId::Codex),
        Some(codex.as_path())
    );
    let ResolvedTerminalLaunch::Program {
        executable,
        arguments,
        ..
    } = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Omp,
            }),
            None,
            false,
            None,
        )
        .expect("OMP launch")
    else {
        panic!("OMP must launch directly");
    };
    assert_eq!(executable, std::fs::canonicalize(omp).unwrap());
    assert!(
        arguments.is_empty(),
        "OMP owns its model and permission settings"
    );
    assert!(!catalog.captures_provider_session(TerminalLaunchProfileId::Omp));
    let ResolvedTerminalLaunch::Program {
        executable,
        arguments,
        environment,
    } = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Kimi,
            }),
            None,
            false,
            Some(CAPTURE_TOKEN),
        )
        .expect("Kimi Code launch")
    else {
        panic!("Kimi Code must launch directly");
    };
    assert_eq!(executable, std::fs::canonicalize(kimi).unwrap());
    assert!(
        arguments.is_empty(),
        "Kimi Code owns its model and permission settings"
    );
    // Its checkpoint plugin is registered once in Kimi's own plugin store,
    // and every Kimi pane carries the private routing its hooks need.
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Kimi));
    let registry: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join(".kimi-code/plugins/installed.json"))
            .expect("Kimi plugin registry"),
    )
    .expect("Kimi plugin registry JSON");
    assert_eq!(registry["plugins"][0]["id"], "interface-checkpoints");
    let current_host = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
    for (name, value) in [
        (
            interface_terminal::native_checkpoint::EXE_ENV,
            current_host.as_os_str(),
        ),
        (
            interface_terminal::native_checkpoint::ROOT_ENV,
            receipts.as_os_str(),
        ),
        (
            interface_terminal::native_checkpoint::TOKEN_ENV,
            std::ffi::OsStr::new(CAPTURE_TOKEN),
        ),
    ] {
        assert!(
            environment
                .iter()
                .any(|(key, found)| key == name && found == value),
            "Kimi pane is missing {name}"
        );
    }
    let ResolvedTerminalLaunch::Program {
        executable,
        arguments,
        ..
    } = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Muse,
            }),
            None,
            false,
            Some("d2d9dfef-c119-4df6-8827-2bad4e79eda6"),
        )
        .expect("Muse Code launch")
    else {
        panic!("Muse Code must launch directly");
    };
    assert_eq!(executable, std::fs::canonicalize(muse).unwrap());
    assert!(
        arguments.len() == 2
            && arguments[0] == "--config"
            && arguments[1].to_string_lossy().starts_with("hooks="),
        "Muse receives only additive lifecycle hooks; its model and permissions stay native"
    );
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Muse));
    // First launches, exact resumes (including restarts), and legacy picker
    // reopens all retain the configured permission mode, even with a cwd peer.
    for session in [
        None,
        Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: None,
            resume: false,
        }),
        Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some("ses_savedOpenCodeChat".to_owned()),
            resume: true,
        }),
        Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: None,
            resume: true,
        }),
    ] {
        for shared in [false, true] {
            let ResolvedTerminalLaunch::Program {
                executable,
                arguments,
                ..
            } = catalog
                .resolve_with_peers(
                    Some(TerminalLaunchRequestV1 {
                        version: 1,
                        profile_id: TerminalLaunchProfileId::Opencode,
                    }),
                    session.as_ref(),
                    shared,
                    Some(CAPTURE_TOKEN),
                )
                .expect("OpenCode launch")
            else {
                panic!("OpenCode must launch directly");
            };
            assert_eq!(executable, std::fs::canonicalize(&opencode).unwrap());
            let mut expected = vec![OsString::from("--auto")];
            if session.as_ref().is_some_and(|session| session.id.is_some()) {
                expected.push(OsString::from("--session=ses_savedOpenCodeChat"));
            }
            assert_eq!(arguments, expected);
        }
    }
    // OpenCode captures are proven from OpenCode's own log, so the pane must
    // never publish a fresh minted handle as provider truth.
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Opencode));
    assert_eq!(catalog.executable(TerminalLaunchProfileId::Shell), None);
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Claude));
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Shell));
    assert!(catalog.captures_provider_session(TerminalLaunchProfileId::Codex));

    for session in [
        None,
        Some(TerminalLaunchSessionV1 {
            version: TERMINAL_LAUNCH_SESSION_VERSION,
            id: Some(CAPTURE_TOKEN.to_owned()),
            resume: true,
        }),
    ] {
        let ResolvedTerminalLaunch::Program { arguments, .. } = catalog
            .resolve_with_peers(
                Some(TerminalLaunchRequestV1 {
                    version: 1,
                    profile_id: TerminalLaunchProfileId::Codex,
                }),
                session.as_ref(),
                false,
                Some(CAPTURE_TOKEN),
            )
            .expect("Codex launch")
        else {
            panic!("Codex must resolve to a direct program");
        };
        assert!(
            arguments.windows(4).any(|pair| {
                pair == [
                    OsString::from("-c"),
                    OsString::from(interface_terminal::CODEX_TERMINAL_TITLE_CONFIG),
                    OsString::from("-c"),
                    OsString::from(interface_terminal::CODEX_FAST_MODE_KEY_CONFIG),
                ]
            }),
            "fresh and resumed Codex panes must announce their chat title"
        );
        assert!(
            arguments.windows(2).any(|pair| {
                pair == [
                    OsString::from("-c"),
                    OsString::from("tui.notification_method=osc9"),
                ]
            }),
            "fresh and resumed Codex panes must use OSC 9 notifications"
        );
    }

    let ResolvedTerminalLaunch::LoginShellWithEnvironment { environment } = catalog
        .resolve_with_peers(None, None, false, Some(CAPTURE_TOKEN))
        .expect("capturing login shell")
    else {
        panic!("the default shell must carry private Claude capture material");
    };
    assert!(environment.iter().any(|(name, value)| {
        name == CLAUDE_MANUAL_SETTINGS_ENV
            && value
                .to_str()
                .is_some_and(|value| value.contains(CAPTURE_TOKEN))
    }));
    assert!(environment.iter().all(|(_, value)| {
        value
            .to_str()
            .is_none_or(|value| !value.contains(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER))
    }));

    let ResolvedTerminalLaunch::Program { arguments, .. } = catalog
        .resolve_with_peers(
            Some(TerminalLaunchRequestV1 {
                version: 1,
                profile_id: TerminalLaunchProfileId::Claude,
            }),
            None,
            false,
            Some(CAPTURE_TOKEN),
        )
        .expect("curated Claude launch")
    else {
        panic!("Claude must resolve to a direct program");
    };
    assert_eq!(arguments.first(), Some(&OsString::from("--mcp-config")));
    assert_eq!(arguments.get(2), Some(&OsString::from("--settings")));
    let settings: serde_json::Value = serde_json::from_str(
        arguments
            .get(3)
            .expect("inline hook settings")
            .to_str()
            .expect("UTF-8 settings"),
    )
    .expect("valid hook settings JSON");
    assert!(settings.get("disableAllHooks").is_none());
    let hooks = settings["hooks"]["SessionStart"]
        .as_array()
        .expect("SessionStart hook array");
    assert_eq!(
        hooks
            .iter()
            .map(|hook| hook["matcher"].as_str().expect("hook matcher"))
            .collect::<Vec<_>>(),
        vec!["startup", "resume", "clear", "compact", "fork"]
    );
    for hook in hooks {
        assert_eq!(hook["hooks"][0]["args"][0], CLAUDE_SESSION_HOOK_MODE);
        assert_eq!(hook["hooks"][0]["args"][1], CAPTURE_TOKEN);
        assert_eq!(hook["hooks"][0]["args"][2].as_str(), receipts.to_str());
    }
    assert_eq!(
        settings["hooks"]["UserPromptSubmit"][0]["hooks"][0],
        hooks[0]["hooks"][0]
    );
    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0],
        hooks[0]["hooks"][0]
    );
    // Every tool call reports work, so self-started turns light the dot.
    assert_eq!(
        settings["hooks"]["PreToolUse"],
        serde_json::json!([{"hooks":[hooks[0]["hooks"][0]]}])
    );
    assert!(settings["hooks"].get("PostToolUse").is_some());
    // A permission dialog or question turns the pane to "needs you".
    assert_eq!(
        settings["hooks"]["PermissionRequest"],
        serde_json::json!([{"hooks":[hooks[0]["hooks"][0]]}])
    );
    assert!(
        settings
            .to_string()
            .contains(crate::delegation_hook::NOTICE_MODE)
    );
    assert!(!settings.to_string().contains(crate::delegation_hook::MODE));
}

#[test]
#[allow(clippy::too_many_lines)] // One argv checked override by override.
fn codex_terminal_hook_template_is_tokenless_and_shell_quoted() {
    let receipts = Path::new("/private/tmp/interface-claude-receipts");
    let socket = Path::new("/private/tmp/iface-test/panes/iface.sock");
    let with_pane_server =
        codex_terminal_capture_arguments(receipts, Some(socket), PaneToolApproval::ReadOnly)
            .expect("Codex pane arguments");
    let arguments = codex_terminal_capture_arguments(receipts, None, PaneToolApproval::ReadOnly)
        .expect("Codex hook arguments");
    assert_eq!(with_pane_server[..10], arguments[..10]);
    assert_eq!(with_pane_server.len(), 28);
    let overrides = with_pane_server
        .chunks_exact(2)
        .map(|pair| {
            assert_eq!(pair[0], "-c");
            pair[1].to_str().unwrap()
        })
        .collect::<Vec<_>>();
    assert!(overrides.contains(&"mcp_servers.iface.command=\"/usr/bin/nc\""));
    assert!(overrides.contains(&"mcp_servers.cheap_agents.command=\"/usr/bin/nc\""));
    assert!(overrides.contains(
        &"mcp_servers.cheap_agents.args=[\"-U\",\"/private/tmp/iface-test/panes/cheap_agents.sock\"]"
    ));
    assert!(
        overrides
            .iter()
            .any(|value| value.starts_with("mcp_servers.cheap_agents.tools=")
                && value.contains("worker_start={approval_mode=\"approve\"}"))
    );
    assert!(
        overrides.contains(
            &"mcp_servers.iface.args=[\"-U\",\"/private/tmp/iface-test/panes/iface.sock\"]"
        )
    );
    for event in ["PreToolUse", "Stop"] {
        assert!(
            !overrides
                .iter()
                .any(|value| value.starts_with(&format!("hooks.{event}=")))
        );
    }
    for event in ["UserPromptSubmit", "PostToolUse"] {
        assert!(
            overrides
                .iter()
                .any(|value| value.starts_with(&format!("hooks.{event}=")))
        );
    }
    assert!(overrides.contains(
        &"mcp_servers.iface.tools={pass_off={approval_mode=\"prompt\"},read_pane={approval_mode=\"approve\"},send_to_pane={approval_mode=\"prompt\"}}"
    ));
    assert!(
        overrides
            .iter()
            .any(|value| value.contains(crate::delegation_hook::NOTICE_MODE))
    );
    let trust = overrides
        .iter()
        .find(|value| value.starts_with("hooks.state="))
        .unwrap();
    assert_eq!(trust.matches("trusted_hash=").count(), 3);
    assert!(!trust.contains("/<session-flags>/config.toml:pre_tool_use:0:0"));
    let executable = std::fs::canonicalize(std::env::current_exe().expect("host executable"))
        .expect("canonical host executable");
    let expected_command = format!(
        "'{}' {CODEX_SESSION_HOOK_MODE} '{}'",
        executable.display(),
        receipts.display()
    );
    assert_eq!(
        arguments,
        vec![
            OsString::from("-c"),
            OsString::from(interface_terminal::CODEX_TERMINAL_TITLE_CONFIG),
            OsString::from("-c"),
            OsString::from(interface_terminal::CODEX_FAST_MODE_KEY_CONFIG),
            OsString::from("-c"),
            OsString::from("tui.notification_method=osc9"),
            OsString::from("-c"),
            OsString::from("features.hooks=true"),
            OsString::from("-c"),
            OsString::from(format!(
                "hooks.SessionStart=[{{hooks=[{{type=\"command\",command=\"{}\"}}]}}]",
                toml_basic_string_escaped(&expected_command)
            )),
            OsString::from("-c"),
            OsString::from(format!(
                "hooks.state={{\"{CODEX_SESSION_HOOK_STATE_KEY}\"={{trusted_hash=\"{}\"}}}}",
                codex_hook_trusted_hash(&expected_command)
            )),
        ]
    );
    // The token never appears in argv: Codex hashes the command for its
    // trust state, so it must be identical for every pane.
    assert!(
        !arguments
            .iter()
            .any(|argument| { argument.to_string_lossy().contains("CAPTURE_TOKEN") })
    );
    assert_eq!(
        shell_single_quoted("/Applications/It's Here/host").expect("quoted"),
        "'/Applications/It'\\''s Here/host'"
    );
    assert_eq!(toml_basic_string_escaped(r#"a\b"c"#), r#"a\\b\"c"#);
    assert!(shell_single_quoted("bad\npath").is_err());
}

/// Pinned against Codex 0.153.3: trusting exactly this hook through its
/// "Hooks need review" prompt wrote this `trusted_hash` to `config.toml`.
#[test]
fn codex_hook_trust_hash_matches_what_codex_persists() {
    assert_eq!(
        codex_hook_trusted_hash(
            "'/Applications/Interface.app/Contents/Resources/bin/interface-host' \
             --terminal-codex-session-hook '/private/tmp/interface-claude-receipts'"
        ),
        "sha256:d9541426b46bfcae7c735be2c5b4344a02aad36a9964bbe746ad4cb969d3b9d0"
    );
    assert_eq!(
        CODEX_SESSION_HOOK_STATE_KEY,
        "/<session-flags>/config.toml:session_start:0:0"
    );
}

#[test]
fn claude_terminal_hook_template_is_enabled_and_tokenized() {
    let receipts = Path::new("/private/tmp/interface-claude-receipts");
    let socket = Path::new("/private/tmp/iface-test/panes/iface.sock");
    let arguments = claude_terminal_capture_arguments(receipts, socket, PaneToolApproval::ReadOnly)
        .expect("Claude hook arguments");
    // `--mcp-config` is variadic, so it comes first and `--settings` closes
    // it; the trailing pass-off prompt then stays a positional.
    assert_eq!(arguments.first(), Some(&OsString::from("--mcp-config")));
    let mcp: serde_json::Value = serde_json::from_str(
        arguments
            .get(1)
            .expect("inline mcp config")
            .to_str()
            .expect("UTF-8 mcp config"),
    )
    .expect("valid mcp config JSON");
    assert_eq!(
        mcp,
        serde_json::json!({"mcpServers": {
            "iface": {
                "type": "stdio", "command": "/usr/bin/nc",
                "args": ["-U", "/private/tmp/iface-test/panes/iface.sock"], "env": {}
            },
            "cheap_agents": {
                "type": "stdio", "command": "/usr/bin/nc",
                "args": ["-U", "/private/tmp/iface-test/panes/cheap_agents.sock"], "env": {}
            }
        }})
    );
    assert_eq!(arguments.get(2), Some(&OsString::from("--settings")));
    assert_eq!(arguments.len(), 4);
    let settings: serde_json::Value = serde_json::from_str(
        arguments
            .get(3)
            .expect("inline hook settings")
            .to_str()
            .expect("UTF-8 settings"),
    )
    .expect("valid hook settings JSON");
    assert!(settings.get("disableAllHooks").is_none());
    assert_eq!(
        settings["permissions"]["allow"],
        serde_json::json!([
            "mcp__iface__read_pane",
            "mcp__cheap_agents__worker_models",
            "mcp__cheap_agents__worker_start",
            "mcp__cheap_agents__worker_collect",
            "mcp__cheap_agents__worker_followup",
            "mcp__cheap_agents__worker_cancel",
            "mcp__cheap_agents__worker_review",
        ])
    );
    let hooks = settings["hooks"]["SessionStart"]
        .as_array()
        .expect("SessionStart hook array");
    assert_eq!(hooks.len(), 5);
    for (hook, matcher) in hooks
        .iter()
        .zip(["startup", "resume", "clear", "compact", "fork"])
    {
        assert_eq!(hook["matcher"], matcher);
        assert_eq!(hook["hooks"][0]["args"][0], CLAUDE_SESSION_HOOK_MODE);
        assert_eq!(
            hook["hooks"][0]["args"][1],
            CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER
        );
        assert_eq!(hook["hooks"][0]["args"][2].as_str(), receipts.to_str());
    }
}

#[test]
fn provider_history_roots_honor_the_frozen_environment_overrides() {
    let home = Path::new("/fixture/home");
    assert_eq!(
        claude_projects_root(home, Some(PathBuf::from("/fixture/claude-config"))),
        PathBuf::from("/fixture/claude-config/projects")
    );
    assert_eq!(
        claude_projects_root(home, Some(PathBuf::new())),
        PathBuf::from("/fixture/home/.claude/projects")
    );
    assert_eq!(
        grok_sessions_root(home, Some(PathBuf::from("/fixture/grok-home"))),
        PathBuf::from("/fixture/grok-home/sessions")
    );
    assert_eq!(
        grok_sessions_root(home, None),
        PathBuf::from("/fixture/home/.grok/sessions")
    );
}

#[test]
fn explicit_data_root_opens_without_contacting_providers() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).expect("test home");
    let active_profiles = vec![AcpProfile {
        slug: "local-agent".into(),
        display_name: "Local Agent".into(),
        executable_path: "/fixtures/local-agent".into(),
        args: vec!["acp".into(), "stdio".into()],
    }];

    let runtime =
        open_test_runtime_at(&root, &active_profiles, &home).expect("runtime at injected root");

    assert_eq!(
        runtime
            .registered_engines()
            .into_iter()
            .map(|engine| engine.to_string())
            .collect::<Vec<_>>(),
        vec!["acp:grok", "acp:local-agent", "acp:qwen", "claude", "codex"]
    );
    assert!(root.is_dir());
    assert!(
        open_test_runtime_at(&root, &active_profiles, &home).is_err(),
        "a second runtime must not bypass the writer leases"
    );
    drop(runtime);
    assert!(
        open_test_runtime_at(&root, &active_profiles, &home).is_ok(),
        "leases release on shutdown"
    );
}

fn chat_unavailable() -> String {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/session-kind-contract.json"
    ))
    .expect("session-kind contract fixture");
    fixture["chatUnavailable"]
        .as_str()
        .expect("chatUnavailable")
        .to_owned()
}

#[test]
fn bootstrap_creates_a_private_chat_folder_and_pins_chat_launches() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).expect("test home");
    let runtime = open_test_runtime_at(&root, &[], &home).expect("runtime at injected root");
    let chat = root.join("chat");
    assert!(chat.is_dir());
    #[cfg(unix)]
    {
        assert_eq!(
            std::fs::metadata(&chat)
                .expect("chat metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let mut options = LaunchOptions {
        cwd: "/tmp/project".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Chat),
        worktree: Some(true),
        ..LaunchOptions::default()
    };
    runtime
        .pin_chat_launch(&mut options)
        .expect("chat home configured");
    assert_eq!(options.cwd, chat.to_string_lossy());
    assert_eq!(options.worktree, None);
    drop(runtime);
}

#[test]
fn bootstrap_disables_chat_when_the_chat_folder_cannot_be_created() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).expect("test home");
    std::fs::create_dir(&root).expect("runtime root");
    #[cfg(unix)]
    {
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("private runtime root");
    }
    std::fs::write(root.join("chat"), b"not a directory").expect("block chat folder");
    let runtime = open_test_runtime_at(&root, &[], &home).expect("code and agent still open");
    let mut options = LaunchOptions {
        cwd: "/tmp/project".into(),
        prompt: "hello".into(),
        kind: Some(SessionKind::Chat),
        ..LaunchOptions::default()
    };
    let error = runtime
        .pin_chat_launch(&mut options)
        .expect_err("chat disabled");
    assert_eq!(error.message, chat_unavailable());
    assert!(error.replay_is_safe());
    assert_eq!(options.cwd, "/tmp/project");
    assert!(
        runtime
            .registered_engines()
            .iter()
            .any(|engine| engine.as_str() == "codex")
    );
    drop(runtime);
}

#[test]
fn shared_host_client_never_opens_the_orchestration_store() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default project root");

    let opened = open_host_services_at_with_orchestration_and_collaboration(
        &root,
        &home,
        "remote-owner-epoch",
        false,
        None,
        None,
        env!("CARGO_PKG_VERSION"),
    )
    .expect("client service graph");

    assert!(opened.runtime.services().orchestration().is_none());
    assert_eq!(opened.orchestration_runtime_epoch, "remote-owner-epoch");
    assert!(!root.join("state").exists());
}

#[tokio::test]
async fn desktop_startup_roster_registers_custom_acp_before_ready() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir_all(home.join("Desktop/Projects")).expect("default project root");

    let first = open_host_services_at(&root, &home).expect("first host graph");
    first
        .desktop
        .dispatch_blocking(
            CHANNELS.set_acp_profiles,
            json!({"profiles": [{
                "slug": "local-agent",
                "displayName": "Local Agent",
                "executablePath": "/fixtures/local-agent",
                "args": ["acp", "stdio"]
            }]}),
        )
        .expect("persist next-startup roster");
    assert!(
        !first
            .runtime
            .registered_engines()
            .iter()
            .any(|engine| engine.as_str() == "acp:local-agent"),
        "the startup roster is immutable for the current process"
    );
    drop(first);

    let reopened = open_host_services_at(&root, &home).expect("reopened host graph");
    assert!(
        reopened
            .runtime
            .registered_engines()
            .iter()
            .any(|engine| engine.as_str() == "acp:local-agent")
    );
    assert_eq!(reopened.desktop.active_acp_profiles().len(), 1);
}

/// Codex hashes hook trust positionally, so the pane's `-c` overrides must
/// keep this exact order and bytes: title, fast-mode key, notifications,
/// hooks, the session hook, the worker hooks, the MCP servers, then trust.
#[test]
fn codex_pane_capture_argv_keeps_its_exact_order() {
    let arguments = codex_terminal_capture_arguments_for(
        Path::new("/Applications/Interface.app/Contents/Resources/bin/interface-host"),
        Path::new("/private/tmp/interface-claude-receipts"),
        Some(Path::new("/private/tmp/iface-test/panes/iface.sock")),
        PaneToolApproval::ReadOnly,
    )
    .expect("Codex pane arguments");
    let expected = [
        r#"tui.terminal_title=["spinner","app-name","fast-mode","model","status","thread"]"#,
        r#"tui.keymap.global.toggle_fast_mode=["shift-f12"]"#,
        "tui.notification_method=osc9",
        "features.hooks=true",
        r#"hooks.SessionStart=[{hooks=[{type="command",command="'/Applications/Interface.app/Contents/Resources/bin/interface-host' --terminal-codex-session-hook '/private/tmp/interface-claude-receipts'"}]}]"#,
        r#"hooks.UserPromptSubmit=[{hooks=[{type="command",command="'/Applications/Interface.app/Contents/Resources/bin/interface-host' --interface-worker-notice codex",timeout=5}]}]"#,
        r#"hooks.PostToolUse=[{hooks=[{type="command",command="'/Applications/Interface.app/Contents/Resources/bin/interface-host' --interface-worker-notice codex",timeout=5}]}]"#,
        r#"mcp_servers.iface.tools={pass_off={approval_mode="prompt"},read_pane={approval_mode="approve"},send_to_pane={approval_mode="prompt"}}"#,
        r#"mcp_servers.cheap_agents.tools={worker_models={approval_mode="approve"},worker_start={approval_mode="approve"},worker_collect={approval_mode="approve"},worker_followup={approval_mode="approve"},worker_cancel={approval_mode="approve"},worker_review={approval_mode="approve"}}"#,
        r#"mcp_servers.iface.command="/usr/bin/nc""#,
        r#"mcp_servers.iface.args=["-U","/private/tmp/iface-test/panes/iface.sock"]"#,
        r#"mcp_servers.cheap_agents.command="/usr/bin/nc""#,
        r#"mcp_servers.cheap_agents.args=["-U","/private/tmp/iface-test/panes/cheap_agents.sock"]"#,
        r#"hooks.state={"/<session-flags>/config.toml:session_start:0:0"={trusted_hash="sha256:d9541426b46bfcae7c735be2c5b4344a02aad36a9964bbe746ad4cb969d3b9d0"},"/<session-flags>/config.toml:user_prompt_submit:0:0"={trusted_hash="sha256:ad77c374923928278d4f0e6c931c5d562c1e73bbc79387eed67e794c82a9cfe7"},"/<session-flags>/config.toml:post_tool_use:0:0"={trusted_hash="sha256:52517b58d858f0dd031a54a1b6f5674995495bd9fc250d5729340ebff698a189"}}"#,
    ];
    assert_eq!(expected[0], interface_terminal::CODEX_TERMINAL_TITLE_CONFIG);
    assert_eq!(expected[1], interface_terminal::CODEX_FAST_MODE_KEY_CONFIG);
    assert_eq!(
        arguments,
        expected
            .iter()
            .flat_map(|value| [OsString::from("-c"), OsString::from(value)])
            .collect::<Vec<_>>()
    );
}

/// A pane tool that types into another pane (`send_to_pane`) or starts one
/// (`pass_off`) asks the owner first, in Claude and in Codex panes, unless
/// the owner switched "Pane tools without asking" on; `read_pane` never asks.
/// Codex gets an explicit `prompt`, so no looser default applies.
#[test]
fn pane_tools_that_act_on_other_panes_ask_unless_the_owner_switched_them_on() {
    let executable = Path::new("/Applications/Interface.app/Contents/Resources/bin/interface-host");
    let receipts = Path::new("/private/tmp/interface-claude-receipts");
    let socket = Path::new("/private/tmp/iface-test/panes/iface.sock");
    for (approval, codex_tools, claude_allowed) in [
        (
            PaneToolApproval::ReadOnly,
            r#"mcp_servers.iface.tools={pass_off={approval_mode="prompt"},read_pane={approval_mode="approve"},send_to_pane={approval_mode="prompt"}}"#,
            vec!["mcp__iface__read_pane"],
        ),
        (
            PaneToolApproval::All,
            r#"mcp_servers.iface.tools={pass_off={approval_mode="approve"},read_pane={approval_mode="approve"},send_to_pane={approval_mode="approve"}}"#,
            vec![
                "mcp__iface__pass_off",
                "mcp__iface__read_pane",
                "mcp__iface__send_to_pane",
            ],
        ),
    ] {
        let codex =
            codex_terminal_capture_arguments_for(executable, receipts, Some(socket), approval)
                .expect("Codex pane arguments");
        assert!(codex.contains(&OsString::from(codex_tools)), "{approval:?}");
        let settings: serde_json::Value = serde_json::from_str(
            claude_terminal_capture_settings_for(executable, receipts, approval)
                .expect("Claude settings")
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let allowed = settings["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool.as_str())
            .filter(|tool| tool.starts_with("mcp__iface__"))
            .collect::<Vec<_>>();
        assert_eq!(allowed, claude_allowed, "{approval:?}");
        // The switch changes only the pane tools: hooks, trust and the
        // workers' tools are the same either way.
        let other = |arguments: &[OsString]| {
            arguments
                .iter()
                .filter(|argument| {
                    !argument
                        .to_string_lossy()
                        .starts_with("mcp_servers.iface.tools=")
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let read_only = codex_terminal_capture_arguments_for(
            executable,
            receipts,
            Some(socket),
            PaneToolApproval::ReadOnly,
        )
        .unwrap();
        assert_eq!(other(&codex), other(&read_only));
    }
    assert_eq!(PaneToolApproval::default(), PaneToolApproval::ReadOnly);
    assert_eq!(
        PaneToolApproval::from_switch(false),
        PaneToolApproval::ReadOnly
    );
    assert_eq!(PaneToolApproval::from_switch(true), PaneToolApproval::All);
}

/// A Codex the owner starts in a shell pane follows the same switch: the
/// host exports it only when it is on, and anything else reads as off.
#[test]
fn a_manual_codex_reads_the_pane_tools_switch_from_its_shell_and_absent_means_ask() {
    assert_eq!(
        PaneToolApproval::from_environment(None),
        PaneToolApproval::ReadOnly
    );
    for value in ["", "0", "true", "yes", " 1"] {
        assert_eq!(
            PaneToolApproval::from_environment(Some(OsStr::new(value))),
            PaneToolApproval::ReadOnly,
            "{value:?}"
        );
    }
    assert_eq!(
        PaneToolApproval::from_environment(Some(OsStr::new("1"))),
        PaneToolApproval::All
    );
    assert!(crate::claude_manual_wrapper::MANUAL_ENVIRONMENT.contains(&PaneToolApproval::ENV));
}

/// Each zsh overlay wraps both CLIs, and a bare `codex` it runs gets the same
/// title, fast-mode key and notification overrides a pane launch passes.
#[test]
fn zsh_overlays_wrap_both_clis_with_the_pane_codex_overrides() {
    for (name, overlay) in [
        (".zshenv", ZSHENV),
        (".zprofile", ZPROFILE),
        (".zshrc", ZSHRC),
    ] {
        assert!(overlay.contains(CLAUDE_MANUAL_WRAPPER_MODE), "{name}");
        assert!(overlay.contains(CODEX_MANUAL_WRAPPER_MODE), "{name}");
        for config in [
            interface_terminal::CODEX_TERMINAL_TITLE_CONFIG,
            interface_terminal::CODEX_FAST_MODE_KEY_CONFIG,
            "tui.notification_method=osc9",
        ] {
            assert!(
                overlay.contains(&format!("-c '{config}'")),
                "{name}: {config}"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn a_session_left_working_by_a_host_crash_opens_idle_without_rewriting_the_cache() {
    use interface_contracts::engine::SessionStatus;
    let temporary = tempfile::tempdir().expect("temporary directory");
    let root = temporary.path().join("Interface");
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).expect("test home");
    std::fs::create_dir(&root).expect("runtime root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).expect("private root");
    // What a host killed mid-turn leaves: its live sessions still "working"
    // (or "waiting" on an approval nobody can answer any more).
    let session = |id: &str, status: &str| {
        json!({
            "engine": "claude", "sessionId": id, "title": id, "cwd": "/tmp",
            "status": status, "cloud": false, "model": "claude-fake-opus", "effort": "low",
            "lastActivity": "2026-11-15T22:46:23.905Z", "firstSeen": "2026-11-15T22:46:18.780Z",
            "archived": false, "origin": "app", "kind": "chat"
        })
    };
    let cache = json!({"sessions": {
        "claude#working-one": session("working-one", "working"),
        "claude#waiting-one": session("waiting-one", "waiting"),
        "claude#failed-one": session("failed-one", "failed"),
    }});
    let cache_path = root.join("registry.json");
    std::fs::write(&cache_path, serde_json::to_vec(&cache).unwrap()).expect("registry cache");
    std::fs::set_permissions(&cache_path, std::fs::Permissions::from_mode(0o600))
        .expect("private cache");
    let before = std::fs::read(&cache_path).unwrap();

    let runtime = open_test_runtime_at(&root, &[], &home).expect("runtime at injected root");
    let statuses = {
        let registry = runtime.services().session_registry();
        let registry = registry.lock().expect("session registry");
        let mut statuses = registry
            .all()
            .into_iter()
            .map(|summary| (summary.session_id.clone(), summary.status))
            .collect::<Vec<_>>();
        statuses.sort_by(|left, right| left.0.cmp(&right.0));
        statuses
    };
    // Nothing runs in a host that just started: no session is working or
    // waiting, so the thread reconcile can close the cut-off run.
    assert_eq!(
        statuses,
        [
            ("failed-one".to_owned(), SessionStatus::Failed),
            ("waiting-one".to_owned(), SessionStatus::Idle),
            ("working-one".to_owned(), SessionStatus::Idle),
        ]
    );
    // Opening still writes no byte of the cache.
    assert_eq!(std::fs::read(&cache_path).unwrap(), before);
}
