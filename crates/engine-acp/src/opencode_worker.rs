//! Bounded, one-shot `OpenCode` Go workers. The CLI owns authentication.
//!
//! This adapter targets `opencode run --format json` as implemented in 1.18.29:
//! <https://github.com/anomalyco/opencode/blob/v1.18.29/packages/opencode/src/cli/cmd/run.ts>.
//! Permissions are CLI policy, not an OS sandbox. Callers must supply a private,
//! credential-filtered snapshot without symlinks and review changes before use.
//! In particular, `OpenCode`'s grep permission matches the query, not file paths.

use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use interface_engine::worker::{self, ProcessGroup, RuntimeDirectoryErrors, cancelled, drain};
use serde_json::{Map, Value, json};
#[cfg(test)]
use std::sync::Arc;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::watch,
};

const MAX_SUMMARY: usize = 16 * 1024;
const MAX_LINE: usize = 256 * 1024;
const MAX_STDOUT: usize = 2 * 1024 * 1024;
const MAX_STDERR: usize = 1024 * 1024;
const MAX_TASK: usize = 64 * 1024;
const WORKER_AGENT: &str = "interface-worker";
/// Every `OpenCode` child a worker or usage read starts gets each of these set
/// to `true`.
pub(super) const OPENCODE_DISABLED: [&str; 9] = [
    "OPENCODE_DISABLE_PROJECT_CONFIG",
    "OPENCODE_DISABLE_CLAUDE_CODE",
    "OPENCODE_DISABLE_EXTERNAL_SKILLS",
    "OPENCODE_DISABLE_DEFAULT_PLUGINS",
    "OPENCODE_DISABLE_LSP_DOWNLOAD",
    "OPENCODE_DISABLE_AUTOUPDATE",
    "OPENCODE_DISABLE_TERMINAL_TITLE",
    "OPENCODE_DISABLE_MODELS_FETCH",
    "OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER",
];
pub use interface_engine::worker::{WorkerOutput, WorkerProgress};

/// Builds the process-local configuration without inspecting user settings.
///
/// # Errors
/// Refuses providers outside Go, non-absolute workspace paths, and nonliteral
/// or sensitive write paths. Filesystem isolation is the caller's responsibility.
pub fn config_content(
    workspace: &Path,
    model: &str,
    write_files: &[String],
) -> Result<String, String> {
    validate_model(model)?;
    let workspace = workspace
        .to_str()
        .filter(|path| Path::new(path).is_absolute() && !has_pattern_bytes(path))
        .ok_or("worker workspace must be an absolute literal UTF-8 path")?;
    if write_files.len() > 64 {
        return Err("worker write scope exceeds 64 files".into());
    }
    let mut edits = Map::from_iter([("*".into(), json!("deny"))]);
    for file in write_files {
        validate_write_file(file)?;
        edits.insert(file.clone(), json!("allow"));
        edits.insert(format!("{workspace}/{file}"), json!("allow"));
        // OpenCode uses worktree="/" for a snapshot without .git, and edit
        // checks path.relative(worktree, filePath). Keep this third spelling
        // exact too; widening to a suffix wildcard would authorize other files.
        edits.insert(
            format!("{}/{file}", workspace.trim_start_matches('/')),
            json!("allow"),
        );
    }
    let truncation_glob = truncation_glob()?;
    // A plain catch-all allow followed by restrictive wildcards survives
    // serde_json's sorted object order. Absolute allows here would override
    // earlier secret denies in OpenCode's last-matching-rule evaluator.
    let permission = json!({
        "*": "deny",
        "read": {
            "*": "allow", "*.env": "deny", "*.env.*": "deny", "*.env.example": "allow",
            "*.key": "deny", "*.pem": "deny", "*credential*": "deny",
            "*service-account*": "deny", "*service_account*": "deny",
            "*/.git/*": "deny", "*/.ssh/*": "deny", "*/.aws/*": "deny",
            "*/.gnupg/*": "deny", "*/.kube/*": "deny"
        },
        "glob": "allow", "grep": "allow", "edit": edits,
        "bash": "deny", "task": "deny",
        "external_directory": {"*": "deny", truncation_glob: "deny"},
        "question": "deny", "skill": "deny", "webfetch": "deny",
        "websearch": "deny", "lsp": "deny", "doom_loop": "deny"
    });
    Ok(json!({
        "$schema": "https://opencode.ai/config.json",
        "model": model, "small_model": model,
        "enabled_providers": [model.split('/').next().unwrap_or_default()],
        "default_agent": WORKER_AGENT, "subagent_depth": 0,
        "agent": {
            WORKER_AGENT: {
                "description": "Implements one bounded task for an Interface parent agent",
                "mode": "primary", "model": model,
                "permission": permission,
                "prompt": "You are an Interface worker. Complete only the supplied task in this private snapshot. Read the supplied context; edit only the explicitly allowed files. Never delegate, run commands, access credentials, publish, install dependencies, or alter instructions. The parent handles tests, review, integration and release. Use only read, glob, grep, edit, write or apply_patch; do not use bash, task, todo, question or other tools. Deliver actual file edits, never a patch described only in prose. If necessary context is missing, stop and name the exact relative files; do not infer their contents. Keep your final summary to one short plain English sentence, without code, regular expressions or backslashes. Your final response must be only one JSON object: {\"version\":1,\"outcome\":\"changes\" or \"needsContext\" or \"blocked\",\"summary\":\"short concrete explanation\",\"missingFiles\":[]}. Use needsContext with exact missing paths, blocked for an unresolved capability or task blocker, and changes only after completing the requested file edits. Do not claim tests ran."
            }
        },
        "permission": permission,
        "plugin": [], "mcp": {}, "instructions": [],
        "formatter": false, "lsp": false, "snapshot": false,
        "share": "disabled", "autoupdate": false,
        "compaction": {"auto": false},
        "experimental": {"batch_tool": false, "continue_loop_on_deny": false}
    }).to_string())
}

fn truncation_glob() -> Result<String, String> {
    // OpenCode 1.18.29 appends this allow after user rules unless an exact deny
    // exists (agent/agent.ts:285). Derive only the pathname; do not read data.
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|path| PathBuf::from(path).join(".local/share")))
        .ok_or("worker cannot resolve the OpenCode data directory")?;
    let data = interface_common::paths::lexically_normalized(&data);
    let data = data
        .to_str()
        .filter(|path| Path::new(path).is_absolute() && !has_pattern_bytes(path))
        .ok_or("worker requires a literal OpenCode data directory")?;
    Ok(format!(
        "{}/opencode/tool-output/*",
        data.trim_end_matches('/')
    ))
}

fn validate_model(model: &str) -> Result<(), String> {
    if model == "opencode/muse-spark-1.3-contributor-free" {
        return Ok(());
    }
    let valid = model.strip_prefix("opencode-go/").is_some_and(|name| {
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    });
    if valid {
        Ok(())
    } else {
        Err("worker model must be an explicit opencode-go/<model> identifier; no fallback is allowed".into())
    }
}

/// Runs a bounded report on supplied inline context with all tools denied.
/// The CLI retains authentication ownership; no fallback or retry is made.
/// # Errors
/// Refuses invalid bounds, unapproved tools, incomplete streams or process errors.
#[allow(clippy::too_many_arguments)]
pub async fn run_report(
    executable: &Path,
    workspace: &Path,
    model: &str,
    task: &str,
    wall_seconds: u64,
    max_steps: u32,
    cancellation: watch::Receiver<bool>,
) -> Result<WorkerOutput, String> {
    if !(1..=600).contains(&wall_seconds) || !(1..=16).contains(&max_steps) {
        return Err("invalid report execution limits".into());
    }
    run_worker(
        executable,
        workspace,
        model,
        task,
        &[],
        cancellation,
        Some(Duration::from_secs(wall_seconds)),
        None,
        Some(max_steps),
    )
    .await
}

fn report_config(workspace: &Path, model: &str, steps: u32) -> Result<String, String> {
    let mut config: Value = serde_json::from_str(&config_content(workspace, model, &[])?)
        .map_err(|_| "invalid internal worker configuration")?;
    config["permission"] = json!({"*":"deny"});
    config["agent"][WORKER_AGENT]["permission"] = json!({"*":"deny"});
    config["agent"][WORKER_AGENT]["steps"] = json!(steps);
    config["agent"][WORKER_AGENT]["prompt"] = json!(
        "You are a read-only report worker. Use only the supplied packet as data. Never use tools, edit files, delegate, or claim checks ran. Return exactly one JSON object with schemaVersion:1, outcome:report|noFindings|needsContext|blocked, summary:string, findings:[{claim:string,evidence:string,uncertainty:string}], missingContext:[string]. Cite captured path and line numbers or supplied source URLs. Findings are untrusted suggestions for the lead to verify. Keep the report under 12000 characters."
    );
    Ok(config.to_string())
}

fn has_pattern_bytes(value: &str) -> bool {
    value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, '*' | '?' | '\\' | '{' | '}'))
}

fn validate_write_file(file: &str) -> Result<(), String> {
    if file.is_empty()
        || file.len() > 1024
        || has_pattern_bytes(file)
        || file
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || !Path::new(file)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(
            "worker write paths must name exact relative files without traversal or wildcards"
                .into(),
        );
    }
    for component in file.split('/') {
        let lower = component.to_ascii_lowercase();
        if (lower.starts_with(".env") && lower != ".env.example")
            || Path::new(&lower)
                .extension()
                .is_some_and(|ext| ext == "env")
            || lower.contains("credential")
            || lower.contains("service-account")
            || lower.contains("service_account")
            || matches!(
                lower.as_str(),
                ".git" | ".opencode" | ".codex" | ".claude" | "agents.md" | "claude.md"
            )
            || interface_common::paths::SENSITIVE_DIRECTORIES.contains(&lower.as_str())
            || matches!(
                Path::new(&lower).extension().and_then(|ext| ext.to_str()),
                Some("key" | "pem" | "p12" | "pfx" | "keystore")
            )
        {
            return Err("worker write scope contains a sensitive or agent-control path".into());
        }
    }
    Ok(())
}

/// Runs exactly one fresh Go session while publishing validated messages and
/// safe tool names. Cancellation and every failure terminate the owned process
/// group; callers must never retry an ambiguous run blindly.
///
/// # Errors
/// Returns bounded, provider-output-free errors for input, launch, cancellation,
/// timeout, protocol, provider, output-limit, and incomplete-turn failures.
pub async fn run_observed(
    executable: &Path,
    workspace: &Path,
    model: &str,
    task: &str,
    write_files: &[String],
    cancellation: watch::Receiver<bool>,
    progress: Option<WorkerProgress>,
) -> Result<WorkerOutput, String> {
    run_worker(
        executable,
        workspace,
        model,
        task,
        write_files,
        cancellation,
        None,
        progress,
        None,
    )
    .await
}
#[allow(clippy::too_many_arguments, clippy::too_many_lines)] // Keep owned process cleanup in one scope.
async fn run_worker(
    executable: &Path,
    workspace: &Path,
    model: &str,
    task: &str,
    write_files: &[String],
    mut cancellation: watch::Receiver<bool>,
    timeout: Option<Duration>,
    progress: Option<WorkerProgress>,
    report_steps: Option<u32>,
) -> Result<WorkerOutput, String> {
    if *cancellation.borrow() {
        return Err("worker cancelled before launch".into());
    }
    if !executable.is_absolute() {
        return Err("worker executable must be an absolute path".into());
    }
    if task.trim().is_empty() || task.len() > MAX_TASK || task.contains('\0') {
        return Err("worker task must contain 1-65536 bytes without NUL".into());
    }
    let workspace = workspace
        .canonicalize()
        .map_err(|_| "worker workspace is unavailable")?;
    if !workspace.is_dir() {
        return Err("worker workspace must be a directory".into());
    }
    let config = if let Some(steps) = report_steps {
        report_config(&workspace, model, steps)?
    } else {
        config_content(&workspace, model, write_files)?
    };
    let runtime = RuntimeDirectory::create(&workspace)?;
    let mut command = worker_command(executable, &workspace, model, &config, &runtime);
    if report_steps.is_some() {
        command.args(["--variant", "medium"]);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "OpenCode worker could not start; verify the installed CLI")?;
    let mut group = ProcessGroup::new(child.id());
    let mut stdin = child.stdin.take().ok_or("worker input pipe unavailable")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("worker output pipe unavailable")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("worker diagnostic pipe unavailable")?;
    let result = {
        let completed = async {
            let input = async {
                stdin
                    .write_all(task.as_bytes())
                    .await
                    .map_err(|_| "worker input delivery is uncertain".to_owned())?;
                stdin
                    .shutdown()
                    .await
                    .map_err(|_| "worker input delivery is uncertain".to_owned())?;
                drop(stdin);
                Ok::<(), String>(())
            };
            let ((), output, ()) = tokio::try_join!(
                input,
                read_events_with_policy(stdout, progress, report_steps),
                drain(stderr, MAX_STDERR)
            )?;
            // Reap only after both pipes close and protocol validation succeeds.
            // Failures therefore kill a still-owned PID, avoiding PID reuse.
            let status = child
                .wait()
                .await
                .map_err(|_| "worker exit status is unavailable")?;
            group.disarm();
            if !status.success() {
                return Err("OpenCode worker failed; check Go connection and model availability in an OpenCode pane".into());
            }
            output.finish()
        };
        tokio::select! {
            result = completed => result,
            () = cancelled(&mut cancellation) => Err("worker cancelled; partial output requires review".into()),
            () = optional_deadline(timeout) => Err("worker timed out; partial output requires review".into()),
        }
    };
    drop(group);
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}

pub(super) fn worker_command(
    executable: &Path,
    workspace: &Path,
    model: &str,
    config: &str,
    runtime: &RuntimeDirectory,
) -> Command {
    let mut command = Command::new(executable);
    command
        .args([
            "run",
            "--pure",
            "--format",
            "json",
            "--model",
            model,
            "--agent",
            WORKER_AGENT,
            "--title",
            "Interface delegated task",
        ])
        .arg("--dir")
        .arg(workspace)
        .current_dir(workspace)
        .env("PWD", workspace)
        .env("XDG_CONFIG_HOME", runtime.path().join("config"))
        .env("OPENCODE_TEST_HOME", runtime.path().join("home"))
        .env("OPENCODE_CONFIG_CONTENT", config)
        .env("OPENCODE_AUTO_SHARE", "false")
        .env("OPENCODE_EXPERIMENTAL", "false")
        .env("OPENCODE_ENABLE_PARALLEL", "false")
        .env("OPENCODE_EXPERIMENTAL_PARALLEL", "false")
        .env("OPENCODE_EXPERIMENTAL_CODE_MODE", "false")
        .env("OPENCODE_EXPERIMENTAL_BACKGROUND_SUBAGENTS", "false")
        .env("INTERFACE_DELEGATE_WORKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for name in [
        "OPENCODE_CONFIG",
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_PERMISSION",
        "OPENCODE_MODELS_PATH",
        "OPENCODE_MODELS_URL",
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_SERVER_USERNAME",
        "INTERFACE_PANE_MCP_SOCKET",
    ] {
        command.env_remove(name);
    }
    for name in OPENCODE_DISABLED {
        command.env(name, "true");
    }
    #[cfg(unix)]
    command.process_group(0);
    command
}

async fn optional_deadline(timeout: Option<Duration>) {
    match timeout {
        Some(duration) => tokio::time::sleep(duration).await,
        None => std::future::pending::<()>().await,
    }
}

/// The shared private runtime directory plus `OpenCode`'s read-only
/// `config/opencode` and its `home`.
pub(super) struct RuntimeDirectory(worker::RuntimeDirectory);

impl RuntimeDirectory {
    pub(super) fn create(workspace: &Path) -> Result<Self, String> {
        let runtime = Self(worker::RuntimeDirectory::create_named(
            workspace,
            ".opencode-runtime",
            &RuntimeDirectoryErrors {
                no_parent: "worker workspace requires a private parent directory",
                clock: "worker runtime clock unavailable",
                create: "worker runtime directory could not be created",
            },
        )?);
        let config = runtime.path().join("config/opencode");
        std::fs::create_dir_all(&config)
            .map_err(|_| "worker configuration directory could not be created")?;
        std::fs::create_dir(runtime.path().join("home"))
            .map_err(|_| "worker home directory could not be created")?;
        std::fs::write(config.join(".gitignore"), "*")
            .map_err(|_| "worker configuration could not be prepared")?;
        // 1.18.29 attempts a default plugin dependency install even with --pure.
        // Npm.install first checks directory writability. Keep this intentionally
        // empty, read-only config directory so no dependency is installed.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o500))
                .map_err(|_| "worker configuration could not be made read-only")?;
        }
        Ok(runtime)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0.0
    }
}

impl Drop for RuntimeDirectory {
    // Runs before the inner directory's own drop removes the tree.
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                self.path().join("config/opencode"),
                std::fs::Permissions::from_mode(0o700),
            );
        }
    }
}

#[derive(Default)]
struct EventOutput {
    report_steps: Option<u32>,
    steps: u32,
    progress: Option<WorkerProgress>,
    usage: crate::opencode_worker_metrics::WorkerUsageAccumulator,
    summary: String,
    session_id: Option<String>,
    completed: bool,
    summary_truncated: bool,
    tool_error_count: usize,
}

impl EventOutput {
    fn accept(&mut self, line: &[u8]) -> Result<(), String> {
        let event: Value = serde_json::from_slice(line)
            .map_err(|_| "worker emitted invalid JSON; execution outcome requires review")?;
        if let Some(session_id) = event.get("sessionID").and_then(Value::as_str) {
            if session_id.is_empty()
                || session_id.len() > 256
                || session_id.chars().any(char::is_control)
            {
                return Err("worker emitted an invalid session identity".into());
            }
            if self
                .session_id
                .as_deref()
                .is_some_and(|current| current != session_id)
            {
                return Err("worker output changed session identity".into());
            }
            self.session_id = Some(session_id.into());
        }
        let part = &event["part"];
        match event.get("type").and_then(Value::as_str) {
            Some("error") => return Err("OpenCode reported a provider error; check Go connection and quota in an OpenCode pane".into()),
            Some("step_start") => {
                self.steps += 1;
                if self.report_steps.is_some_and(|limit| self.steps > limit) { return Err("worker step limit exceeded".into()); }
                self.publish("status", "Working…"); self.completed = false; self.summary.clear(); self.summary_truncated = false;
            }
            Some("step_finish") => { self.completed = part["reason"] == "stop"; self.usage.observe_step(part); }
            Some("text") => {
                let text = part["text"].as_str().ok_or("worker text event is malformed")?;
                self.publish("message", text);
                if !self.summary_truncated {
                    if !self.summary.is_empty() { self.summary_truncated |= append_bounded(&mut self.summary, "\n"); }
                    self.summary_truncated |= append_bounded(&mut self.summary, text);
                }
            }
            Some("tool_use") => {
                if self.report_steps.is_some() { return Err("read-only report worker attempted a tool".into()); }
                let tool = part["tool"].as_str().ok_or("worker tool event is malformed")?;
                if !matches!(tool, "read" | "glob" | "grep" | "edit" | "write" | "patch" | "apply_patch" | "multiedit") {
                    return Err("worker attempted a tool outside its delegated capabilities".into());
                }
                if part["state"]["status"] == "error" { self.tool_error_count += 1; }
                let status = match part["state"]["status"].as_str() {
                    Some("completed") => "completed", Some("error") => "failed",
                    Some("running") => "running", _ => "started",
                };
                self.publish("tool", &format!("{tool} · {status}"));
            }
            Some("reasoning") => {},
            _ => return Err("worker emitted an unsupported event; update the OpenCode adapter before retrying".into()),
        }
        Ok(())
    }

    fn publish(&self, kind: &str, text: &str) {
        // Unidentified events cannot be associated with a worker session yet.
        if self.session_id.is_some() {
            if let Some(progress) = &self.progress {
                progress(kind, text);
            }
        }
    }

    fn finish(mut self) -> Result<WorkerOutput, String> {
        if !self.completed || self.summary.trim().is_empty() || self.session_id.is_none() {
            return Err(
                "worker exited without a confirmed final answer; partial output requires review"
                    .into(),
            );
        }
        if self.summary_truncated {
            let suffix = "\n[summary truncated]";
            let mut end = (MAX_SUMMARY - suffix.len()).min(self.summary.len());
            while !self.summary.is_char_boundary(end) {
                end -= 1;
            }
            self.summary.truncate(end);
            self.summary.push_str(suffix);
        }
        Ok(WorkerOutput {
            usage: self.usage.snapshot(),
            summary: self.summary,
            session_id: self.session_id,
            tool_error_count: self.tool_error_count,
            actual_model: None,
        })
    }
}

fn append_bounded(output: &mut String, text: &str) -> bool {
    let remaining = MAX_SUMMARY.saturating_sub(output.len());
    let mut end = remaining.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&text[..end]);
    end < text.len()
}

async fn read_events_with_policy(
    mut stream: impl AsyncRead + Unpin,
    progress: Option<WorkerProgress>,
    report_steps: Option<u32>,
) -> Result<EventOutput, String> {
    let mut output = EventOutput {
        report_steps,
        progress,
        ..EventOutput::default()
    };
    let mut buffer = [0; 4096];
    let mut line = Vec::new();
    let mut count = 0;
    loop {
        let size = stream
            .read(&mut buffer)
            .await
            .map_err(|_| "worker output pipe failed")?;
        if size == 0 {
            return if line.is_empty() {
                Ok(output)
            } else {
                Err("worker output ended with a truncated event".into())
            };
        }
        count += size;
        if count > MAX_STDOUT {
            return Err("worker output limit exceeded".into());
        }
        for byte in &buffer[..size] {
            if *byte == b'\n' {
                if !line.is_empty() {
                    output.accept(&line)?;
                }
                line.clear();
            } else {
                if line.len() >= MAX_LINE {
                    return Err("worker event size limit exceeded".into());
                }
                line.push(*byte);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    const MODEL: &str = "opencode-go/minimax-m3";
    const FINISH: &str = r"
print(json.dumps({'type':'text','sessionID':'ses_fixture','part':{'text':'Implemented.'}}))
print(json.dumps({'type':'step_finish','sessionID':'ses_fixture','part':{'reason':'stop'}}))
";

    struct Fixture {
        root: TempDir,
        executable: PathBuf,
        workspace: PathBuf,
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let executable = root.path().join("fake-opencode");
            let workspace = root.path().join("snapshot with spaces");
            std::fs::create_dir(&workspace).unwrap();
            std::fs::write(&executable, format!("#!/usr/bin/python3\nimport json, os, sys, time\nprompt = sys.stdin.read()\n{body}\n")).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                root,
                executable,
                workspace,
            }
        }

        async fn run(&self) -> Result<WorkerOutput, String> {
            let (_sender, receiver) = watch::channel(false);
            run_worker(
                &self.executable,
                &self.workspace,
                MODEL,
                "Do the supplied task.",
                &["src/item.rs".into()],
                receiver,
                Some(Duration::from_secs(5)),
                None,
                None,
            )
            .await
        }

        fn assert_runtime_cleaned(&self) {
            assert!(!std::fs::read_dir(self.root.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".opencode-runtime-")
            }));
        }
    }

    #[tokio::test]
    async fn activity_arrives_before_exit_and_excludes_private_tool_diagnostics() {
        let fixture = Fixture::new(&format!(
            "print(json.dumps({{'type':'text','sessionID':'ses_fixture','part':{{'text':'Working live'}}}}), flush=True)\nprint(json.dumps({{'type':'tool_use','sessionID':'ses_fixture','part':{{'tool':'read','state':{{'status':'error','error':'PRIVATE_TOOL_SENTINEL','output':'PRIVATE_OUTPUT'}}}}}}), flush=True)\nwhile not os.path.exists('release'): time.sleep(0.01)\n{FINISH}"
        ));
        let (sender, mut activity) = tokio::sync::mpsc::unbounded_channel();
        let observer: WorkerProgress = Arc::new(move |kind, text| {
            let _ = sender.send((kind.to_owned(), text.to_owned()));
        });
        let (_sender, receiver) = watch::channel(false);
        let run = run_observed(
            &fixture.executable,
            &fixture.workspace,
            MODEL,
            "Task",
            &[],
            receiver,
            Some(observer),
        );
        tokio::pin!(run);
        let observe = async {
            assert_eq!(
                activity.recv().await.unwrap(),
                ("message".into(), "Working live".into())
            );
            assert_eq!(
                activity.recv().await.unwrap(),
                ("tool".into(), "read · failed".into())
            );
            std::fs::write(fixture.workspace.join("release"), "release").unwrap();
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            let (result, ()) = tokio::join!(&mut run, observe);
            assert_eq!(result.unwrap().summary, "Working live\nImplemented.");
        })
        .await
        .unwrap();
        fixture.assert_runtime_cleaned();
    }

    #[test]
    fn rejected_events_never_reach_the_activity_observer() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let copy = captured.clone();
        let mut output = EventOutput {
            progress: Some(Arc::new(move |_, text| {
                copy.lock().unwrap().push(text.to_owned());
            })),
            ..EventOutput::default()
        };
        output
            .accept(br#"{"type":"text","sessionID":"one","part":{"text":"safe"}}"#)
            .unwrap();
        assert!(
            output
                .accept(br#"{"type":"text","sessionID":"two","part":{"text":"FOREIGN"}}"#)
                .is_err()
        );
        assert!(
            output
                .accept(br#"{"type":"tool_use","sessionID":"one","part":{"tool":"bash"}}"#)
                .is_err()
        );
        output
            .accept(br#"{"type":"reasoning","sessionID":"one","part":{"text":"PRIVATE"}}"#)
            .unwrap();
        assert_eq!(*captured.lock().unwrap(), vec!["safe"]);
    }

    #[test]
    fn config_fences_model_tools_and_exact_write_paths() {
        let cfg: Value = serde_json::from_str(
            &config_content(
                Path::new("/private/snapshot"),
                MODEL,
                &["src/item.rs".into(), "new file.md".into()],
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["enabled_providers"], json!(["opencode-go"]));
        assert_eq!(cfg["small_model"], MODEL);
        assert_eq!(cfg["agent"][WORKER_AGENT]["model"], MODEL);
        assert!(cfg["agent"][WORKER_AGENT].get("steps").is_none());
        assert_eq!(cfg["agent"][WORKER_AGENT]["permission"], cfg["permission"]);
        let permission = &cfg["permission"];
        assert_eq!(permission["*"], "deny");
        for tool in [
            "bash",
            "task",
            "question",
            "skill",
            "webfetch",
            "websearch",
            "lsp",
        ] {
            assert_eq!(permission[tool], "deny");
        }
        assert_eq!(permission["external_directory"]["*"], "deny");
        assert_eq!(
            permission["external_directory"][truncation_glob().unwrap()],
            "deny"
        );
        assert_eq!(permission["edit"]["*"], "deny");
        assert_eq!(permission["edit"]["src/item.rs"], "allow");
        assert_eq!(permission["edit"]["/private/snapshot/src/item.rs"], "allow");
        assert_eq!(permission["edit"]["private/snapshot/src/item.rs"], "allow");
        assert_eq!(permission["edit"].as_object().unwrap().len(), 7);
        assert_eq!(permission["read"]["*.env"], "deny");
        assert_eq!(permission["read"]["*.env.example"], "allow");
        for option in ["formatter", "lsp", "snapshot", "autoupdate"] {
            assert_eq!(cfg[option], false);
        }
        assert_eq!(cfg["share"], "disabled");
        assert_eq!(cfg["subagent_depth"], 0);
    }

    #[test]
    fn invalid_models_and_scope_never_reach_a_cli() {
        for model in [
            "openai/gpt-5",
            "opencode/minimax-m3",
            "opencode-go/",
            "opencode-go/m3 --auto",
            "opencode-go/a/b",
        ] {
            assert!(config_content(Path::new("/private/snapshot"), model, &[]).is_err());
        }
        for file in [
            "../item.rs",
            "/tmp/item.rs",
            "src/*.rs",
            "src/a?.rs",
            "src/./a.rs",
            "src//a.rs",
            ".env",
            "key.pem",
            "credentials/file.json",
            ".git/config",
            ".opencode/tools/run.ts",
            "AGENTS.md",
        ] {
            assert!(
                config_content(Path::new("/private/snapshot"), MODEL, &[file.into()]).is_err(),
                "{file}"
            );
        }
        assert!(config_content(Path::new("/private/*"), MODEL, &[]).is_err());
    }

    #[tokio::test]
    async fn fixture_receives_literal_stdin_scoped_config_and_no_shell_options() {
        let body = format!(
            r"
assert sys.argv[1:10] == ['run','--pure','--format','json','--model','opencode-go/minimax-m3','--agent','interface-worker','--title']
assert '--auto' not in sys.argv
assert sys.argv[-2] == '--dir'
assert sys.argv[-1] == os.getcwd()
assert os.environ['PWD'] == os.getcwd()
assert 'INTERFACE_PANE_MCP_SOCKET' not in os.environ
assert 'OPENCODE_PERMISSION' not in os.environ
assert os.environ['OPENCODE_DISABLE_PROJECT_CONFIG'] == 'true'
assert os.environ['OPENCODE_DISABLE_CLAUDE_CODE'] == 'true'
assert os.environ['OPENCODE_DISABLE_DEFAULT_PLUGINS'] == 'true'
assert os.environ['OPENCODE_TEST_HOME'] != os.environ['HOME']
cfg = json.loads(os.environ['OPENCODE_CONFIG_CONTENT'])
assert 'steps' not in cfg['agent']['interface-worker']
assert cfg['permission']['edit']['src/item.rs'] == 'allow'
assert cfg['permission']['edit'][os.getcwd() + '/src/item.rs'] == 'allow'
assert cfg['permission']['edit'][os.getcwd().lstrip('/') + '/src/item.rs'] == 'allow'
assert cfg['permission']['external_directory']['*'] == 'deny'
assert cfg['permission']['external_directory'][os.path.join(os.environ.get('XDG_DATA_HOME', os.environ['HOME'] + '/.local/share'), 'opencode/tool-output/*')] == 'deny'
assert os.stat(os.environ['XDG_CONFIG_HOME'] + '/opencode').st_mode & 0o777 == 0o500
assert prompt == 'Literal $HOME and $(touch should-not-exist) `whoami` --auto\nsecond line'
{FINISH}
"
        );
        let fixture = Fixture::new(&body);
        let (_sender, receiver) = watch::channel(false);
        let output = run_observed(
            &fixture.executable,
            &fixture.workspace,
            MODEL,
            "Literal $HOME and $(touch should-not-exist) `whoami` --auto\nsecond line",
            &["src/item.rs".into()],
            receiver,
            None,
        )
        .await
        .unwrap();
        assert_eq!(output.summary, "Implemented.");
        assert_eq!(output.session_id.as_deref(), Some("ses_fixture"));
        assert!(output.actual_model.is_none());
        assert_eq!(output.tool_error_count, 0);
        assert!(!fixture.workspace.join("should-not-exist").exists());
        fixture.assert_runtime_cleaned();
    }

    #[tokio::test]
    async fn provider_error_is_failure_even_with_zero_exit_and_diagnostics_are_private() {
        let fixture = Fixture::new(
            r"
sys.stderr.write('PRIVATE_DIAGNOSTIC_SENTINEL')
print(json.dumps({'type':'error','error':{'name':'APIError','data':{'message':'PRIVATE_PROVIDER_SENTINEL'}}}))
",
        );
        let error = fixture.run().await.unwrap_err();
        assert!(error.contains("provider error"));
        assert!(!error.contains("PRIVATE_"));
        fixture.assert_runtime_cleaned();
    }

    #[tokio::test]
    async fn completed_text_does_not_hide_nonzero_exit() {
        let fixture = Fixture::new(&format!("{FINISH}\nsys.exit(4)"));
        assert!(fixture.run().await.unwrap_err().contains("worker failed"));
    }

    #[tokio::test]
    async fn tool_errors_are_counted_even_after_a_successful_repair() {
        let fixture = Fixture::new(&format!(
            "print(json.dumps({{'type':'tool_use','sessionID':'ses_fixture','part':{{'tool':'edit','state':{{'status':'error','error':'PRIVATE_TOOL_SENTINEL'}}}}}}))\nprint(json.dumps({{'type':'tool_use','sessionID':'ses_fixture','part':{{'tool':'edit','state':{{'status':'completed'}}}}}}))\n{FINISH}"
        ));
        let output = fixture.run().await.unwrap();
        assert_eq!(output.tool_error_count, 1);
        assert!(!output.summary.contains("PRIVATE_TOOL_SENTINEL"));
    }

    #[tokio::test]
    async fn incomplete_or_inconsistent_turns_are_not_success() {
        for body in [
            "pass".to_owned(),
            "print(json.dumps({'type':'text','sessionID':'a','part':{'text':'not done'}}))".into(),
            FINISH.replace("'stop'", "'tool-calls'"),
            FINISH.replace("'stop'", "'length'"),
            format!(
                "{FINISH}\nprint(json.dumps({{'type':'step_start','sessionID':'ses_fixture','part':{{}}}}))"
            ),
            format!(
                "{FINISH}\nprint(json.dumps({{'type':'text','sessionID':'another','part':{{'text':'wrong session'}}}}))"
            ),
        ] {
            let fixture = Fixture::new(&body);
            assert!(fixture.run().await.is_err());
            fixture.assert_runtime_cleaned();
        }
    }

    #[tokio::test]
    async fn forbidden_tool_and_invalid_or_truncated_json_fail_closed() {
        for body in [
            "print('not json')",
            "sys.stdout.write('{\"type\":\"text\"}')",
            "print(json.dumps({'type':'tool_use','sessionID':'s','part':{'tool':'bash'}}))",
        ] {
            let fixture = Fixture::new(body);
            assert!(fixture.run().await.is_err());
        }
    }

    #[tokio::test]
    async fn oversized_lines_total_output_and_stderr_are_bounded() {
        for (body, expected) in [
            (
                format!("sys.stdout.write('x' * {})", MAX_LINE + 1),
                "event size limit",
            ),
            (
                "for i in range(40):\n print(json.dumps({'type':'reasoning','part':{'text':'x' * 70000}}))".to_owned(),
                "output limit",
            ),
            (
                format!("sys.stderr.write('x' * {})\n{FINISH}", MAX_STDERR + 1),
                "diagnostic output limit",
            ),
        ] {
            let fixture = Fixture::new(&body);
            assert!(
                fixture.run().await.unwrap_err().contains(expected),
                "{expected}"
            );
            fixture.assert_runtime_cleaned();
        }
    }

    #[tokio::test]
    async fn long_unicode_summary_is_explicitly_truncated_at_a_character_boundary() {
        let fixture = Fixture::new(&FINISH.replace("'Implemented.'", "'語' * 7000"));
        let output = fixture.run().await.unwrap();
        assert!(output.summary.len() <= MAX_SUMMARY);
        assert!(output.summary.ends_with("[summary truncated]"));
        assert!(output.summary.starts_with("語語"));
    }

    #[tokio::test]
    async fn cancellation_kills_and_reaps_the_cli() {
        let fixture = Fixture::new("open('started', 'w').write(str(os.getpid()))\ntime.sleep(30)");
        let (sender, receiver) = watch::channel(false);
        let executable = fixture.executable.clone();
        let workspace = fixture.workspace.clone();
        let running = tokio::spawn(async move {
            run_observed(&executable, &workspace, MODEL, "Task", &[], receiver, None).await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !fixture.workspace.join("started").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        sender.send(true).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(3), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(error.contains("cancelled"));
        let pid = std::fs::read_to_string(fixture.workspace.join("started")).unwrap();
        let status = std::process::Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success());
        fixture.assert_runtime_cleaned();
    }

    #[tokio::test]
    async fn timeout_terminates_descendants_that_keep_pipes_open() {
        let fixture = Fixture::new(
            "if os.fork() == 0:\n time.sleep(1.5)\n open('leaked-child', 'w').write('leak')\n os._exit(0)\ntime.sleep(30)",
        );
        let (_sender, receiver) = watch::channel(false);
        let result = run_worker(
            &fixture.executable,
            &fixture.workspace,
            MODEL,
            "Task",
            &[],
            receiver,
            Some(Duration::from_millis(400)),
            None,
            None,
        )
        .await;
        assert!(result.unwrap_err().contains("timed out"));
        tokio::time::sleep(Duration::from_millis(1600)).await;
        assert!(!fixture.workspace.join("leaked-child").exists());
        fixture.assert_runtime_cleaned();
    }

    #[tokio::test]
    async fn cancellation_before_spawn_does_not_create_a_runtime() {
        let fixture = Fixture::new(FINISH);
        let (_sender, receiver) = watch::channel(true);
        assert!(
            run_observed(
                &fixture.executable,
                &fixture.workspace,
                MODEL,
                "Task",
                &[],
                receiver,
                None
            )
            .await
            .unwrap_err()
            .contains("before launch")
        );
        fixture.assert_runtime_cleaned();
    }
}
