//! CLI-specific setup for native checkpoint observations. Provider settings
//! and permission policies remain owned by their CLIs.
use crate::bounded_process::{Capture, CapturedOutput, run_bounded};
use chrono::{SecondsFormat, Utc};
use interface_storage::{
    PrivateDirectory, WriterLock, atomic_write, read_bounded, snapshot_target,
};
use interface_terminal::{CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER, native_checkpoint as native};
use serde_json::{Value, json};
use std::{
    error::Error,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// The Muse lifecycle events Interface observes, with each plugin hook's
/// stable id. One list feeds the plugin bundle, the `--config` fallback and
/// their tests, so the two routes can never drift apart.
const MUSE_PLUGIN_HOOKS: [(&str, &str); 11] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "user-prompt-submit"),
    ("PreToolUse", "pre-tool-use"),
    ("PreLLMCall", "pre-llm-call"),
    ("PermissionRequest", "permission-request"),
    ("SubagentStart", "subagent-start"),
    ("SubagentStop", "subagent-stop"),
    ("Stop", "stop"),
    ("StopFailure", "stop-failure"),
    ("Interrupt", "interrupt"),
    ("SessionEnd", "session-end"),
];
/// Bundle path per `MUSE_PLUGIN_HOOKS` entry: the validator requires one
/// distinct script file per hook id.
const MUSE_PLUGIN_SCRIPT_PATHS: [&str; 11] = [
    "hooks/session-start.sh",
    "hooks/user-prompt-submit.sh",
    "hooks/pre-tool-use.sh",
    "hooks/pre-llm-call.sh",
    "hooks/permission-request.sh",
    "hooks/subagent-start.sh",
    "hooks/subagent-stop.sh",
    "hooks/stop.sh",
    "hooks/stop-failure.sh",
    "hooks/interrupt.sh",
    "hooks/session-end.sh",
];
const MUSE_PLUGIN_ID: &str = "interface-checkpoints";
/// Runtime-root file naming this channel's host executable. Plugin hook
/// scripts (one install serves every channel) read it to find the host
/// that owns their pane; the receipts root beside it is where that host
/// resolves the pane by lineage.
const HOOK_HOST_FILE: &str = ".interface-muse-hook-host";

fn quote(path: &str) -> Result<String, Box<dyn Error>> {
    if path.chars().any(char::is_control) {
        return Err("invalid native hook path".into());
    }
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}

/// Fallback override, used only when the checkpoint plugin is unusable:
/// append observers to the user's hooks, preserving every handler and its
/// order. `--config` is run-scoped, so these observers die with the first
/// in-process session replacement; the plugin route above is the norm.
pub(crate) fn muse_arguments(
    home: &Path,
    executable: &Path,
    receipts: &Path,
) -> Result<Vec<OsString>, Box<dyn Error>> {
    let config =
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
    let path = config.join("muse/settings.json");
    let mut hooks = if path.exists() {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 1024 * 1024 {
            return Err("Muse settings exceed the inspection limit".into());
        }
        serde_json::from_slice::<Value>(&bytes)?
            .get("hooks")
            .cloned()
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    let command = format!(
        "{} {} muse",
        quote(executable.to_str().ok_or("invalid host path")?)?,
        native::HOOK_MODE
    );
    let root = quote(receipts.to_str().ok_or("invalid receipt path")?)?;
    append_muse_hooks(&mut hooks, &command, &root)?;
    Ok(vec!["--config".into(), format!("hooks={hooks}").into()])
}

fn append_muse_hooks(hooks: &mut Value, command: &str, root: &str) -> Result<(), Box<dyn Error>> {
    let map = hooks
        .as_object_mut()
        .ok_or("Muse hooks are not an object")?;
    for (event, _) in MUSE_PLUGIN_HOOKS {
        let mut hook = json!({
            "type": "command",
            "command": format!("{command} {event} {CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER} {root}"),
            "timeout": 5
        });
        if event == "Interrupt" {
            hook["async"] = json!(true);
        }
        let groups = map
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("Muse hook event is not an array")?;
        groups.push(json!({"hooks":[hook]}));
    }
    Ok(())
}

/// One plugin hook's script. Muse filters a plugin hook's environment to
/// its own variables (no Interface token can ride along), so the script
/// routes by the two channels' runtime roots under `$HOME` and the host
/// resolves the pane by process lineage. stdin is buffered once because
/// both channels get a probe and the foreign one must stay a no-op.
fn muse_hook_script(event: &str) -> String {
    format!(
        "#!/bin/sh\n\
         # Interface pane observer. Inactive outside Interface: nothing\n\
         # routes unless a channel's host names itself and this pane is in\n\
         # its receipts by lineage.\n\
         payload=$(cat)\n\
         for channel in \"Interface\" \"Iface Dev\"; do\n\
         \x20 support=\"$HOME/Library/Application Support/$channel\"\n\
         \x20 host=$(cat \"$support/{HOOK_HOST_FILE}\" 2>/dev/null) || continue\n\
         \x20 [ -n \"$host\" ] && [ -x \"$host\" ] || continue\n\
         \x20 printf '%s' \"$payload\" | \"$host\" {mode} muse {event} {lineage} \"$support/claude-terminal-session-receipts\" >/dev/null 2>&1\n\
         done\n\
         exit 0\n",
        mode = native::HOOK_MODE,
        lineage = native::LINEAGE_TOKEN,
    )
}

/// The plugin bundle as (path under the bundle root, bytes). Pure so the
/// bundle Interface ships is the bundle its tests inspect.
fn muse_plugin_bundle() -> Vec<(&'static str, Vec<u8>)> {
    let hooks: Vec<Value> = MUSE_PLUGIN_HOOKS
        .iter()
        .map(|(event, id)| {
            let mut hook = json!({
                "id": id,
                "event": event,
                "command": ["sh", format!("hooks/{id}.sh")],
                "timeoutMs": 5000
            });
            if *event == "Interrupt" {
                hook["async"] = json!(true);
            }
            hook
        })
        .collect();
    let manifest = json!({
        "schemaVersion": 1,
        "name": MUSE_PLUGIN_ID,
        "displayName": "Interface Checkpoints",
        "version": "1.0.0",
        "description": "Observe Interface panes for cooperative update checkpoints. Inactive outside Interface.",
        "compat": {"source": "native", "manifestDir": ".muse-plugin"},
        "capabilities": {
            "skills": [],
            "commands": [],
            "hooks": hooks,
            "mcpServers": [],
            "reminders": []
        }
    });
    let mut bundle = vec![(
        ".muse-plugin/plugin.json",
        serde_json::to_vec(&manifest).expect("plugin manifest serializes"),
    )];
    for ((event, _), path) in MUSE_PLUGIN_HOOKS.iter().zip(MUSE_PLUGIN_SCRIPT_PATHS) {
        bundle.push((path, muse_hook_script(event).into_bytes()));
    }
    bundle
}

fn muse_command(
    executable: &Path,
    arguments: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let (status, bytes) = run_bounded(
        &mut command,
        timeout,
        Duration::from_millis(50),
        Capture {
            output: CapturedOutput::Stdout,
            max_bytes: 256 * 1024,
            overflow_fails: true,
        },
    )
    .map_err(|_| "Muse checkpoint plugin operation failed")?;
    if !status.success() {
        return Err("Muse checkpoint plugin operation failed".into());
    }
    Ok(bytes?)
}

fn listed_muse_plugin(list: &Value) -> Option<&Value> {
    list.get("plugins")?
        .as_array()?
        .iter()
        .find(|entry| entry["record"]["id"].as_str() == Some(MUSE_PLUGIN_ID))
}

/// Whether the installed plugin is enabled with every hook approved and
/// active. The list output's review warning is printed for every
/// third-party plugin, approved or not, so readiness comes from
/// `inspect`'s per-capability status instead.
fn muse_plugin_active(executable: &Path) -> Result<bool, Box<dyn Error>> {
    let inspected = muse_command(
        executable,
        &["plugins", "inspect", MUSE_PLUGIN_ID, "--json"],
        Duration::from_secs(5),
    )?;
    let inspected: Value = serde_json::from_slice(&inspected)?;
    let Some(capabilities) = inspected["runtime_capabilities"].as_array() else {
        return Ok(false);
    };
    let hooks: Vec<&Value> = capabilities
        .iter()
        .filter(|capability| {
            capability["candidate"]["stable_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(&format!("plugin:{MUSE_PLUGIN_ID}:hook:")))
        })
        .collect();
    Ok(inspected["record"]["enabled"].as_bool() == Some(true)
        && inspected["active"].as_bool() == Some(true)
        && hooks.len() == MUSE_PLUGIN_HOOKS.len()
        && hooks
            .iter()
            .all(|capability| capability["status"].as_str() == Some("trusted_enabled")))
}

fn muse_plugin_is_ours(entry: &Value, bundle: &Path) -> bool {
    let recorded = entry["record"]["source"]["path"].as_str();
    let canonical = std::fs::canonicalize(bundle)
        .ok()
        .and_then(|path| path.to_str().map(str::to_owned));
    recorded.is_some() && recorded == canonical.as_deref()
}

fn approve_muse_plugin(executable: &Path) -> Result<(), Box<dyn Error>> {
    let approved = muse_command(
        executable,
        &["plugins", "approve", MUSE_PLUGIN_ID, "--json"],
        Duration::from_secs(5),
    )?;
    let approved: Value = serde_json::from_slice(&approved)?;
    let capabilities = approved["runtime_capabilities"]
        .as_array()
        .ok_or("invalid Muse plugin approval")?;
    let complete = capabilities.len() == MUSE_PLUGIN_HOOKS.len()
        && capabilities.iter().all(|capability| {
            capability["enabled"].as_bool() == Some(true)
                && capability["stable_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with(&format!("plugin:{MUSE_PLUGIN_ID}:hook:")))
        });
    if !complete {
        return Err("Muse checkpoint plugin approval was incomplete".into());
    }
    Ok(())
}

/// Record which host executable serves this channel, for the plugin hooks.
/// Called on every startup so an app update re-points the routing at once.
pub(crate) fn write_muse_hook_host(
    runtime: &Path,
    executable: &Path,
) -> Result<(), Box<dyn Error>> {
    let path = executable.to_str().ok_or("invalid host path")?;
    if path.chars().any(char::is_control) {
        return Err("invalid host path".into());
    }
    let directory = PrivateDirectory::open_or_create(runtime)?;
    let writer = WriterLock::acquire(&directory, ".muse-hook-host.lock")?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        let expected = snapshot_target(&directory, HOOK_HOST_FILE, 4096)?;
        atomic_write(
            &directory,
            &writer,
            HOOK_HOST_FILE,
            path.as_bytes(),
            &expected,
            4096,
        )?;
        directory.assert_unchanged()?;
        Ok(())
    })();
    let released = writer.release(&directory);
    result?;
    released?;
    Ok(())
}

/// Muse's `--config` hooks are run-scoped: an in-process `/clear` starts a
/// fresh session whose config reload drops them, so the pane's stop is
/// never observed again (probed on Muse 1.4.13, 2026-10-26: first session
/// fires every hook, the post-clear session none, while settings and
/// plugin hooks fire in both). Plugins are the hook source Muse reloads
/// for every session, so Interface registers a small namespaced observer
/// once, like Antigravity's. The installed scripts do nothing outside
/// Interface panes. Dev and production share the one registration and
/// route by their own routing files. An existing registration — disabled
/// or owner-managed — is left untouched. `Ok(false)` means the plugin
/// route is not usable and the caller keeps the `--config` fallback.
pub(crate) fn install_muse_plugin(
    runtime: &Path,
    executable: &Path,
) -> Result<bool, Box<dyn Error>> {
    let directory = PrivateDirectory::open_or_create(runtime.join("muse-checkpoints"))?;
    if read_bounded(&directory, "installed-v1", 1)?.is_some() {
        return Ok(true);
    }
    if !executable.is_file() {
        return Err("Muse Code is unavailable".into());
    }
    let writer = WriterLock::acquire(&directory, "setup.lock")?;
    let result = (|| -> Result<bool, Box<dyn Error>> {
        if read_bounded(&directory, "installed-v1", 1)?.is_some() {
            return Ok(true);
        }
        let bundle = directory.open_or_create_child("bundle")?;
        let listed = muse_command(
            executable,
            &["plugins", "list", "--json"],
            Duration::from_secs(5),
        )?;
        let listed: Value = serde_json::from_slice(&listed)?;
        if let Some(entry) = listed_muse_plugin(&listed) {
            // Usually the other channel's registration: record it, so this
            // channel stops re-checking on every start, as the installer does.
            if muse_plugin_active(executable)? {
                let expected = snapshot_target(&directory, "installed-v1", 1)?;
                atomic_write(&directory, &writer, "installed-v1", b"1", &expected, 1)?;
                return Ok(true);
            }
            // Only a half-finished run of our own installer may be
            // completed here; approving someone else's bundle under this
            // id would trust hooks Interface never wrote.
            if !(muse_plugin_is_ours(entry, bundle.path())
                && entry["record"]["enabled"].as_bool() == Some(true))
            {
                return Ok(false);
            }
            approve_muse_plugin(executable)?;
            let expected = snapshot_target(&directory, "installed-v1", 1)?;
            atomic_write(&directory, &writer, "installed-v1", b"1", &expected, 1)?;
            return Ok(true);
        }
        for (path, bytes) in muse_plugin_bundle() {
            let (parent, name) = path
                .rsplit_once('/')
                .ok_or("invalid Muse plugin bundle path")?;
            let parent = bundle.open_or_create_child(parent)?;
            // The lease lives in the setup directory so the installed
            // bundle keeps only the files Muse validates. It cannot be
            // the setup lock itself: this installer already holds that.
            let lease = WriterLock::acquire_for(&directory, "bundle.lock", &parent)?;
            let expected = snapshot_target(&parent, name, 16384)?;
            let written = atomic_write(&parent, &lease, name, &bytes, &expected, 16384);
            let released = lease.release(&parent);
            written?;
            released?;
        }
        let bundle_path = bundle
            .path()
            .to_str()
            .ok_or("invalid Muse plugin bundle path")?;
        muse_command(
            executable,
            &[
                "plugins",
                "install",
                bundle_path,
                "--scope",
                "user",
                "--json",
            ],
            Duration::from_secs(10),
        )?;
        approve_muse_plugin(executable)?;
        let expected = snapshot_target(&directory, "installed-v1", 1)?;
        atomic_write(&directory, &writer, "installed-v1", b"1", &expected, 1)?;
        Ok(true)
    })();
    let release = writer.release(&directory);
    let ready = result?;
    release?;
    Ok(ready)
}

/// Antigravity has no process-local hooks flag. Its native plugin installer
/// registers a small, namespaced observer once. The installed commands do
/// nothing outside launches carrying Interface's private environment; Dev and
/// production each route to their own binary and receipt directory. No user
/// hooks/config file is replaced. A disabled/uninstalled plugin stays disabled.
pub(crate) fn install_agy(runtime: &Path, executable: &Path) -> Result<(), Box<dyn Error>> {
    let directory = PrivateDirectory::open_or_create(runtime.join("agy-checkpoints"))?;
    if read_bounded(&directory, "installed-v1", 1)?.is_some() {
        return Ok(());
    }
    if !executable.is_file() {
        return Err("Antigravity is unavailable".into());
    }
    let writer = WriterLock::acquire(&directory, "setup.lock")?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        if read_bounded(&directory, "installed-v1", 1)?.is_some() {
            return Ok(());
        }
        // Never replace an existing owner-managed/disabled plugin or enable it
        // again when a second Interface profile starts.
        let existing = agy_command(executable, &["plugin", "list", "--json"])?;
        let existing: Value = serde_json::from_slice(&existing)?;
        let existing = existing
            .as_array()
            .ok_or("invalid Antigravity plugin listing")?;
        if existing
            .iter()
            .any(|entry| entry.get("name").and_then(Value::as_str) == Some("interface-checkpoints"))
        {
            // Recorded like an install, so this channel does not list the
            // plugins again on every start (as Kimi's installer does).
            let expected = snapshot_target(&directory, "installed-v1", 1)?;
            atomic_write(&directory, &writer, "installed-v1", b"1", &expected, 1)?;
            return Ok(());
        }
        let manifest = serde_json::to_vec(
            &json!({"name":"interface-checkpoints","version":"1.0.0","description":"Observe Interface panes for cooperative update checkpoints. Inactive outside Interface."}),
        )?;
        let mut events = serde_json::Map::new();
        for event in ["SessionStart", "PreInvocation", "Stop"] {
            // Shell parameter expansion is quoted; nothing supplied by a
            // model is interpreted as shell program text.
            let command = format!(
                "if [ -n \"${{{}}}\" ] && [ -n \"${{{}}}\" ] && [ -n \"${{{}}}\" ]; then \"${{{}}}\" {} agy {event} \"${{{}}}\" \"${{{}}}\"; fi",
                native::EXE_ENV,
                native::TOKEN_ENV,
                native::ROOT_ENV,
                native::EXE_ENV,
                native::HOOK_MODE,
                native::TOKEN_ENV,
                native::ROOT_ENV
            );
            events.insert(event.into(), json!([{"command":command,"timeout":5}]));
        }
        let hooks = serde_json::to_vec(&json!({"interface-checkpoints":events}))?;
        for (name, bytes) in [
            ("plugin.json", manifest.as_slice()),
            ("hooks.json", hooks.as_slice()),
        ] {
            let expected = snapshot_target(&directory, name, 16384)?;
            atomic_write(&directory, &writer, name, bytes, &expected, 16384)?;
        }
        let mut command = Command::new(executable);
        command
            .args(["plugin", "install"])
            .arg(directory.path())
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        let (status, output) = run_bounded(
            &mut command,
            Duration::from_secs(10),
            Duration::from_millis(50),
            Capture {
                output: CapturedOutput::Stdout,
                max_bytes: 8192,
                overflow_fails: true,
            },
        )
        .map_err(|_| "Antigravity checkpoint plugin installation failed")?;
        output?;
        if !status.success() {
            return Err("Antigravity checkpoint plugin installation failed".into());
        }
        let expected = snapshot_target(&directory, "installed-v1", 1)?;
        atomic_write(&directory, &writer, "installed-v1", b"1", &expected, 1)?;
        Ok(())
    })();
    let release = writer.release(&directory);
    result?;
    release?;
    Ok(())
}

const KIMI_PLUGIN_ID: &str = "interface-checkpoints";
/// Kimi Code reads `plugins/installed.json` whole; anything larger is not
/// a registry Interface should rewrite.
const KIMI_REGISTRY_LIMIT: u64 = 1024 * 1024;

/// The Kimi plugin manifest (`kimi.plugin.json`), pure so the manifest
/// Interface installs is the one its tests inspect. Kimi runs each hook
/// through `sh -c` in the pane's own environment, so the command acts only
/// inside an Interface pane carrying its host's private variables; Dev and
/// production each route to their own binary and receipts. Shell parameter
/// expansion is quoted, stdout (Kimi's hook protocol) stays empty and the
/// exit is always 0, so a hook can never block, answer or add to a turn.
fn kimi_plugin_manifest() -> Value {
    let hooks: Vec<Value> = native::KIMI_HOOK_EVENTS
        .iter()
        .map(|event| {
            let command = format!(
                "if [ -n \"${{{exe}}}\" ] && [ -n \"${{{token}}}\" ] && [ -n \"${{{root}}}\" ]; \
                 then \"${{{exe}}}\" {mode} kimi {event} \"${{{token}}}\" \"${{{root}}}\" \
                 >/dev/null 2>&1; fi; exit 0",
                exe = native::EXE_ENV,
                token = native::TOKEN_ENV,
                root = native::ROOT_ENV,
                mode = native::HOOK_MODE,
            );
            json!({"event": event, "command": command, "timeout": 5})
        })
        .collect();
    json!({
        "name": KIMI_PLUGIN_ID,
        "version": "1.0.0",
        "description": "Observe Interface panes for session resume and activity. Inactive outside Interface.",
        "interface": {
            "displayName": "Interface Checkpoints",
            "shortDescription": "Interface pane resume and activity. Inactive outside Interface."
        },
        "hooks": hooks
    })
}

fn read_kimi_registry(path: &Path) -> Result<Option<Vec<u8>>, Box<dyn Error>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(KIMI_REGISTRY_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > KIMI_REGISTRY_LIMIT {
        return Err("Kimi plugin registry exceeds the inspection limit".into());
    }
    Ok(Some(bytes))
}

/// Write `name` in `directory` the way Kimi writes its own plugin files:
/// whole, through a sibling temporary file renamed into place.
fn write_kimi_file(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    builder.create(directory)?;
    let temporary = directory.join(format!("{name}.interface-{}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let written = (|| -> std::io::Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, directory.join(name))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    Ok(written?)
}

/// Kimi Code has no plugin-install command: its `/plugins` panel copies a
/// plugin to `<KIMI_CODE_HOME>/plugins/managed/<id>` and records it in
/// `plugins/installed.json`. Interface does the same once for its small,
/// namespaced observer, which reports each pane's session id and turn
/// activity and does nothing outside Interface. Kimi rewrites the registry
/// whole and takes no lock, so the registry is replaced only if it is
/// unchanged since it was read. An existing record under this id, or one the
/// owner later disables or removes, is never touched again.
pub(crate) fn install_kimi_plugin(
    runtime: &Path,
    kimi_home: &Path,
    executable: &Path,
) -> Result<(), Box<dyn Error>> {
    let directory = PrivateDirectory::open_or_create(runtime.join("kimi-checkpoints"))?;
    if read_bounded(&directory, "installed-v1", 1)?.is_some() {
        return Ok(());
    }
    if !executable.is_file() {
        return Err("Kimi Code is unavailable".into());
    }
    let writer = WriterLock::acquire(&directory, "setup.lock")?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        if read_bounded(&directory, "installed-v1", 1)?.is_some() {
            return Ok(());
        }
        let plugins = kimi_home.join("plugins");
        let registry = plugins.join("installed.json");
        let before = read_kimi_registry(&registry)?;
        let mut installed = match &before {
            Some(bytes) => serde_json::from_slice::<Value>(bytes)?,
            None => json!({"version": 1, "plugins": []}),
        };
        let entries = installed
            .get_mut("plugins")
            .and_then(Value::as_array_mut)
            .ok_or("invalid Kimi plugin registry")?;
        if !entries
            .iter()
            .any(|entry| entry.get("id").and_then(Value::as_str) == Some(KIMI_PLUGIN_ID))
        {
            let root = plugins.join("managed").join(KIMI_PLUGIN_ID);
            write_kimi_file(
                &root,
                "kimi.plugin.json",
                &serde_json::to_vec_pretty(&kimi_plugin_manifest())?,
            )?;
            // Kimi records the managed copy by its real path.
            let root = std::fs::canonicalize(&root)?;
            let root = root.to_str().ok_or("invalid Kimi plugin path")?;
            let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            entries.push(json!({
                "id": KIMI_PLUGIN_ID,
                "root": root,
                "source": "local-path",
                "enabled": true,
                "installedAt": now,
                "updatedAt": now,
                "originalSource": root
            }));
            let bytes = serde_json::to_vec_pretty(&installed)?;
            if read_kimi_registry(&registry)? != before {
                return Err("Kimi plugin registry changed during setup".into());
            }
            write_kimi_file(&plugins, "installed.json", &bytes)?;
        }
        let expected = snapshot_target(&directory, "installed-v1", 1)?;
        atomic_write(&directory, &writer, "installed-v1", b"1", &expected, 1)?;
        Ok(())
    })();
    let release = writer.release(&directory);
    result?;
    release?;
    Ok(())
}

fn agy_command(executable: &Path, arguments: &[&str]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut command = Command::new(executable);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let (status, bytes) = run_bounded(
        &mut command,
        Duration::from_secs(5),
        Duration::from_millis(50),
        Capture {
            output: CapturedOutput::Stdout,
            max_bytes: 256 * 1024,
            overflow_fails: true,
        },
    )
    .map_err(|_| "Antigravity plugin inspection failed")?;
    if !status.success() {
        return Err("Antigravity plugin inspection failed".into());
    }
    Ok(bytes?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn muse_observers_preserve_existing_hooks_and_do_not_change_permissions() {
        let previous = json!({"hooks":[{"type":"command","command":"owner-check"}]});
        let mut hooks = json!({"Stop":[previous.clone()],"state":{"owner":true}});
        append_muse_hooks(&mut hooks, "'/host' --hook muse", "'/receipts'").unwrap();
        assert_eq!(hooks["Stop"][0], previous);
        assert_eq!(hooks["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(hooks["state"], json!({"owner":true}));
        assert!(!hooks.to_string().contains("permissionDecision"));
        assert!(
            hooks["Stop"][1]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(CLAUDE_SESSION_CAPTURE_TOKEN_PLACEHOLDER)
        );
        for event in ["StopFailure", "Interrupt"] {
            assert_eq!(hooks[event].as_array().unwrap().len(), 1);
        }
        assert_eq!(hooks["Interrupt"][0]["hooks"][0]["async"], json!(true));
    }

    // Kimi Code 2.3.1 loads `kimi.plugin.json` hooks (event, matcher,
    // command, timeout; strict) for every record in `plugins/installed.json`.
    #[test]
    fn kimi_plugin_registers_every_observed_event_and_keeps_owner_plugins() {
        let manifest = kimi_plugin_manifest();
        assert_eq!(manifest["name"], json!(KIMI_PLUGIN_ID));
        let hooks = manifest["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), native::KIMI_HOOK_EVENTS.len());
        for (hook, event) in hooks.iter().zip(native::KIMI_HOOK_EVENTS) {
            let keys: Vec<&String> = hook.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["command", "event", "timeout"]);
            assert_eq!(hook["event"], json!(event));
            assert_eq!(hook["timeout"], json!(5));
            let command = hook["command"].as_str().unwrap();
            assert!(command.contains(&format!("{} kimi {event} ", native::HOOK_MODE)));
            for variable in [native::EXE_ENV, native::TOKEN_ENV, native::ROOT_ENV] {
                assert!(command.contains(&format!("[ -n \"${{{variable}}}\" ]")));
            }
            assert!(command.contains(">/dev/null 2>&1"));
            assert!(command.ends_with("exit 0"));
        }

        let temp = tempfile::tempdir().unwrap();
        // The host's runtime root always exists before setup runs.
        let runtime = temp.path().join("runtime");
        PrivateDirectory::open_or_create(&runtime).unwrap();
        let kimi_home = temp.path().join("kimi-home");
        let executable = temp.path().join("kimi");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        let owner = json!({"id":"owner-tools","root":"/tmp/owner-tools","source":"local-path","enabled":false,"installedAt":"2026-10-01T00:00:00.000Z","updatedAt":"2026-10-01T00:00:00.000Z"});
        std::fs::create_dir_all(kimi_home.join("plugins")).unwrap();
        std::fs::write(
            kimi_home.join("plugins/installed.json"),
            serde_json::to_vec(&json!({"version":1,"plugins":[owner.clone()]})).unwrap(),
        )
        .unwrap();
        install_kimi_plugin(&runtime, &kimi_home, &executable).unwrap();
        let registry: Value = serde_json::from_slice(
            &std::fs::read(kimi_home.join("plugins/installed.json")).unwrap(),
        )
        .unwrap();
        let entries = registry["plugins"].as_array().unwrap();
        assert_eq!(entries[0], owner);
        assert_eq!(entries[1]["id"], json!(KIMI_PLUGIN_ID));
        assert_eq!(entries[1]["enabled"], json!(true));
        let root = PathBuf::from(entries[1]["root"].as_str().unwrap());
        let installed: Value =
            serde_json::from_slice(&std::fs::read(root.join("kimi.plugin.json")).unwrap()).unwrap();
        assert_eq!(installed, manifest);

        // Once installed, an owner's later removal stays removed.
        std::fs::write(
            kimi_home.join("plugins/installed.json"),
            serde_json::to_vec(&json!({"version":1,"plugins":[owner]})).unwrap(),
        )
        .unwrap();
        install_kimi_plugin(&runtime, &kimi_home, &executable).unwrap();
        let registry: Value = serde_json::from_slice(
            &std::fs::read(kimi_home.join("plugins/installed.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(registry["plugins"].as_array().unwrap().len(), 1);

        // A registry Kimi could not read is never rewritten.
        let broken = temp.path().join("broken-home");
        std::fs::create_dir_all(broken.join("plugins")).unwrap();
        std::fs::write(broken.join("plugins/installed.json"), b"{\"plugins\":").unwrap();
        let fresh_runtime = temp.path().join("runtime-2");
        PrivateDirectory::open_or_create(&fresh_runtime).unwrap();
        assert!(install_kimi_plugin(&fresh_runtime, &broken, &executable).is_err());
        assert_eq!(
            std::fs::read(broken.join("plugins/installed.json")).unwrap(),
            b"{\"plugins\":"
        );
    }

    // Dev and production share one user-wide registration, so the channel
    // that did not install finds it present. It records that like an
    // install and does not ask the CLI again on the next start.
    #[cfg(unix)]
    #[test]
    fn plugins_registered_by_the_other_channel_are_checked_once() {
        use std::{fmt::Write, os::unix::fs::PermissionsExt};
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        PrivateDirectory::open_or_create(&runtime).unwrap();
        let log = temp.path().join("calls.log");
        let script = |name: &str, replies: &[(&str, Value)]| {
            let mut body = format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncase \"$*\" in\n",
                log.display()
            );
            for (arguments, reply) in replies {
                let reply_path = temp
                    .path()
                    .join(format!("{name}-{}.json", arguments.replace(' ', "_")));
                std::fs::write(&reply_path, serde_json::to_vec(reply).unwrap()).unwrap();
                writeln!(body, "  '{arguments}') cat '{}' ;;", reply_path.display()).unwrap();
            }
            body.push_str("  *) exit 1 ;;\nesac\n");
            let path = temp.path().join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let calls = || std::fs::read_to_string(&log).unwrap_or_default();

        let muse = script(
            "muse",
            &[
                (
                    "plugins list --json",
                    json!({"plugins":[{"record":{"id":MUSE_PLUGIN_ID,"enabled":true}}]}),
                ),
                (
                    "plugins inspect interface-checkpoints --json",
                    json!({
                        "record":{"enabled":true},
                        "active":true,
                        "runtime_capabilities": MUSE_PLUGIN_HOOKS
                            .iter()
                            .map(|(event, _)| json!({
                                "candidate":{"stable_id":format!("plugin:{MUSE_PLUGIN_ID}:hook:{event}")},
                                "status":"trusted_enabled"
                            }))
                            .collect::<Vec<_>>()
                    }),
                ),
            ],
        );
        assert!(install_muse_plugin(&runtime, &muse).unwrap());
        assert_eq!(
            calls(),
            "plugins list --json\nplugins inspect interface-checkpoints --json\n"
        );
        assert!(runtime.join("muse-checkpoints/installed-v1").is_file());
        assert!(install_muse_plugin(&runtime, &muse).unwrap());
        assert_eq!(
            calls(),
            "plugins list --json\nplugins inspect interface-checkpoints --json\n"
        );

        std::fs::remove_file(&log).unwrap();
        let agy = script(
            "agy",
            &[(
                "plugin list --json",
                json!([{"name":"interface-checkpoints","enabled":true}]),
            )],
        );
        install_agy(&runtime, &agy).unwrap();
        assert_eq!(calls(), "plugin list --json\n");
        assert!(runtime.join("agy-checkpoints/installed-v1").is_file());
        install_agy(&runtime, &agy).unwrap();
        assert_eq!(calls(), "plugin list --json\n");
        // An existing registration is never installed over.
        assert!(!runtime.join("agy-checkpoints/plugin.json").exists());
    }

    // The plugin is the only Muse hook source reloaded on an in-process
    // session replacement (`/clear`): run-scoped `--config` hooks are
    // dropped by the fresh session (Muse 1.4.13, probed 2026-10-26).
    // Every event must route, by lineage, to whichever Interface channel
    // owns the pane, and no-op outside Interface.
    #[test]
    fn muse_plugin_bundle_routes_every_event_by_lineage() {
        let bundle = muse_plugin_bundle();
        let find = |path: &str| {
            bundle
                .iter()
                .find(|(candidate, _)| *candidate == path)
                .map_or_else(
                    || panic!("bundle is missing {path}"),
                    |(_, bytes)| String::from_utf8(bytes.clone()).unwrap(),
                )
        };
        let manifest: Value = serde_json::from_str(&find(".muse-plugin/plugin.json")).unwrap();
        assert_eq!(manifest["name"], json!(MUSE_PLUGIN_ID));
        let hooks = manifest["capabilities"]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), MUSE_PLUGIN_HOOKS.len());
        let mut ids = std::collections::BTreeSet::new();
        for hook in hooks {
            let id = hook["id"].as_str().unwrap();
            assert!(ids.insert(id), "duplicate hook id {id}");
            let event = hook["event"].as_str().unwrap();
            assert!(
                MUSE_PLUGIN_HOOKS.iter().any(|(known, _)| *known == event),
                "unexpected {event}"
            );
            assert_eq!(hook["timeoutMs"], json!(5000));
            if event == "Interrupt" {
                assert_eq!(hook["async"], json!(true));
            }
            let script_path = hook["command"][1].as_str().unwrap();
            let script = find(script_path);
            assert!(script.contains(native::HOOK_MODE), "{script_path}");
            assert!(script.contains(&format!("muse {event}")), "{script_path}");
            assert!(script.contains(native::LINEAGE_TOKEN), "{script_path}");
            assert!(script.contains(HOOK_HOST_FILE), "{script_path}");
            assert!(script.contains("exit 0"), "{script_path}");
        }
        // The routing file is a plain executable path a shell `cat` can read.
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        write_muse_hook_host(&runtime, Path::new("/Applications/Interface.app/host")).unwrap();
        assert_eq!(
            std::fs::read_to_string(runtime.join(HOOK_HOST_FILE)).unwrap(),
            "/Applications/Interface.app/host"
        );
    }
}
