//! CLI adapter for the explicit disposable-project pilot. No desktop stores.
use crate::errors::{CLI_EXIT, CommandResult};
use async_trait::async_trait;
use interface_collaboration::{
    away::{self, Executor, Progress, Run, Status, Store, Task},
    delegates::{DelegateJob, DelegateRequest, DelegateRunner},
};
use interface_host::CliDelegate;
use interface_storage::{PrivateDirectory, read_bounded};
use interface_workspace::delegation::Change;
use serde_json::{Value, json};
use std::{
    fmt::Write as _,
    fs::{DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command, sync::watch};

const USAGE: &str = "Away pilot v1
  iface away demo --directory <NEW absolute directory> --model codex/<model>
  iface away status --directory <run directory>
  iface away stop --directory <run directory>
  iface away recover --directory <run directory>

Demo creates a disposable Python utilities project, repairs three functions,
parks one deliberately blocked publishing task, and stops. Three attempts per
task, 180 seconds per provider call. Ctrl+C or stop requests cancellation.
No existing project, ordinary delegation setting, or production release is changed.
status prints the durable JSON evidence. recover marks interrupted work unknown
without calling a provider. Models must already be supported and authenticated.
";
const GRADER: &str = include_str!("../../../scripts/delegation-quality-grade.py");

struct Options {
    verb: String,
    root: PathBuf,
    model: Option<String>,
}

fn options(args: &[String]) -> Result<Options, String> {
    let verb = args.first().ok_or(USAGE)?;
    if !matches!(verb.as_str(), "demo" | "status" | "stop" | "recover") {
        return Err(USAGE.into());
    }
    let mut root = None;
    let mut model = None;
    let mut chunks = args[1..].chunks_exact(2);
    for pair in &mut chunks {
        match pair[0].as_str() {
            "--directory" if root.is_none() => root = Some(PathBuf::from(&pair[1])),
            "--model" if model.is_none() && verb == "demo" => {
                interface_engine::worker::model_name(&pair[1], "codex/")?;
                model = Some(pair[1].clone());
            }
            _ => return Err("Unknown, duplicated, or incompatible Away option".into()),
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("Away option requires a value".into());
    }
    let root = root
        .filter(|p| p.is_absolute())
        .ok_or("--directory must be absolute")?;
    if verb == "demo" && model.is_none() {
        return Err("Choose an explicit --model codex/<model>".into());
    }
    Ok(Options {
        verb: verb.clone(),
        root,
        model,
    })
}

pub async fn run(args: &[String], home: &Path, progress: Option<Progress>) -> CommandResult {
    if args.first().is_some_and(|arg| arg == "pair") {
        return crate::away_pair::run(&args[1..], home, progress).await;
    }
    if args.first().is_some_and(|arg| arg == "cycle") {
        return crate::away_cycle::run(&args[1..], home, progress).await;
    }
    if args.is_empty() || args == ["--help"] || args == ["help"] {
        return CommandResult::success(USAGE.into());
    }
    let options = match options(args) {
        Ok(options) => options,
        Err(error) => return CommandResult::error(error, CLI_EXIT.validation),
    };
    let result = match options.verb.as_str() {
        "status" => Store::read(&options.root).and_then(|run| run_output(&run)),
        "recover" => Store::recover(&options.root).and_then(|run| run_output(&run)),
        "stop" => Store::request_stop(&options.root).map(|()| {
            (
                "Stop requested. Read status to confirm termination; partial work is retained."
                    .into(),
                CLI_EXIT.success,
            )
        }),
        _ => return demo(options, home, progress).await,
    };
    match result {
        Ok((text, exit_code)) => CommandResult {
            stdout: format!("{}\n", crate::sanitize::for_display(&text)),
            stderr: String::new(),
            exit_code,
        },
        Err(error) => CommandResult::error(error, CLI_EXIT.generic),
    }
}

fn run_output(run: &Run) -> Result<(String, u8), String> {
    let exit_code = if run.tasks.iter().any(|task| task.status == Status::Unknown) {
        CLI_EXIT.unknown
    } else {
        CLI_EXIT.success
    };
    serde_json::to_string_pretty(&run)
        .map(|text| (text, exit_code))
        .map_err(|error| error.to_string())
}

fn tasks() -> Vec<Task> {
    let mut tasks = [
        ("tags", "normalize_tags", "Normalize comma-separated tags: strip Unicode whitespace, lowercase, discard empty tags, deduplicate preserving first-seen order. Return a list of strings."),
        ("intervals", "merge_intervals", "Merge integer [start,end] lists into sorted disjoint lists, merging overlaps and touching endpoints. Empty input returns []. Reversed endpoints raise ValueError. Do not mutate input or its inner lists."),
        ("checks", "summarize_checks", "Summarize a list of dictionaries into passed, failed, unknown counts and state. Only boolean True/False count as passed/failed; missing, null, strings and numbers are unknown. State is failed if any failed, else unknown if any unknown or empty input, else passed. Do not mutate input."),
    ].into_iter().map(|(id, function, brief)| Task {
        id: id.into(), file: format!("{id}.py"), gate: id.into(), blocked_reason: None,
        prompt: format!("Repair {function}: {brief} Keep exactly one pure Python function with its single positional argument. No imports, I/O, introspection, decorators, helper functions/classes, dependencies or tools. Standard builtins only. The evaluator permits strip/lower/split/append/add/get/items/keys/values/copy/sort/fromkeys methods. Return a complete file proposal. The host runs tests."),
    }).collect::<Vec<_>>();
    tasks.insert(
        1,
        Task {
            id: "publish".into(),
            file: String::new(),
            gate: String::new(),
            prompt: "Publish the utility project".into(),
            blocked_reason: Some(
                "Demonstration blocker: publication was not authorized; continuing local repairs"
                    .into(),
            ),
        },
    );
    tasks
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())
}

fn initialize(root: &Path) -> Result<PathBuf, String> {
    // Exclusive mkdir: never open an existing project as a writable demo.
    DirBuilder::new()
        .mode(0o700)
        .create(root)
        .map_err(|e| format!("Demo requires a new directory: {e}"))?;
    let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let project = directory
        .open_or_create_child("project")
        .map_err(|e| e.to_string())?;
    directory
        .open_or_create_child("packets")
        .map_err(|e| e.to_string())?;
    for (name, code) in [
        (
            "tags.py",
            "def normalize_tags(text):\n    return text.split(',')\n",
        ),
        (
            "intervals.py",
            "def merge_intervals(intervals):\n    return intervals\n",
        ),
        (
            "checks.py",
            "def summarize_checks(checks):\n    return {}\n",
        ),
        (
            "README.md",
            "# Away pilot: text utilities\n\nThree deliberately incomplete pure functions. Interface's Rust supervisor repairs them through content-only proposals and an independent restricted evaluator. Publishing is deliberately blocked. No dependencies are required.\n",
        ),
    ] {
        write_new(&project.path().join(name), code.as_bytes())?;
    }
    Ok(project.path().to_path_buf())
}

async fn demo(options: Options, home: &Path, progress: Option<Progress>) -> CommandResult {
    let executable = home.join(".local/bin/codex");
    if !executable.is_file() {
        return CommandResult::error(
            "The pilot requires the installed ~/.local/bin/codex CLI",
            CLI_EXIT.unsent,
        );
    }
    let project = match initialize(&options.root) {
        Ok(project) => project,
        Err(error) => return CommandResult::error(error, CLI_EXIT.validation),
    };
    let model = options.model.expect("validated model");
    let store = match Store::create(&options.root, project.clone(), model.clone(), tasks()) {
        Ok(store) => store,
        Err(error) => return CommandResult::error(error, CLI_EXIT.generic),
    };
    let (stop, cancel) = watch::channel(false);
    let root = options.root.clone();
    let monitor = tokio::spawn(async move {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => { let _ = stop.send(true); break; }
                () = async {
                    if let Some(signal) = &mut terminate { signal.recv().await; }
                    else { std::future::pending::<()>().await; }
                } => { let _ = stop.send(true); break; }
                () = tokio::time::sleep(Duration::from_millis(200)) => {
                    if Store::stop_requested(&root).unwrap_or(true) {
                        let _ = stop.send(true); break;
                    }
                }
            }
        }
    });
    if let Some(observer) = &progress {
        observer(&format!(
            "Away pilot started: {}\nModel: {model}\nThree repairs, one blocked task. Ctrl+C stops.",
            project.display()
        ));
    }
    let executor = DemoExecutor {
        project,
        root: options.root.clone(),
        model,
        runner: CliDelegate {
            codex: Some(executable),
            ..Default::default()
        },
    };
    let result = away::drive(store, &executor, cancel, progress).await;
    monitor.abort();
    match result {
        Ok(run) => {
            let report = summary(&run, &options.root);
            let exit = if run.tasks.iter().any(|t| t.status == Status::Unknown) {
                CLI_EXIT.unknown
            } else if run
                .tasks
                .iter()
                .filter(|t| t.status == Status::Accepted)
                .count()
                == 3
            {
                CLI_EXIT.success
            } else {
                CLI_EXIT.generic
            };
            CommandResult {
                stdout: crate::sanitize::for_display(&report),
                stderr: String::new(),
                exit_code: exit,
            }
        }
        Err(error) => CommandResult::error(
            format!("Pilot stopped: {error}. Inspect the retained run; do not replay it"),
            CLI_EXIT.unknown,
        ),
    }
}

fn summary(run: &Run, root: &Path) -> String {
    let mut text = String::from("\nAway pilot finished\n");
    for record in &run.tasks {
        let _ = writeln!(
            text,
            "  {}: {:?}, {} attempt(s) — {}",
            record.task.id,
            record.status,
            record.attempts.len(),
            record.detail
        );
    }
    let _ = writeln!(
        text,
        "Project: {}\nEvidence: {}",
        run.project.display(),
        root.join("away-run.v1.json").display()
    );
    text
}

struct DemoExecutor {
    project: PathBuf,
    root: PathBuf,
    model: String,
    runner: CliDelegate,
}

fn changes(task: &Task, proposal: &Value) -> Result<Vec<Change>, String> {
    let changes: Vec<Change> =
        serde_json::from_value(proposal["changes"].clone()).map_err(|e| e.to_string())?;
    if changes.len() != 1
        || changes[0].path != task.file
        || changes[0].source_changed
        || changes[0].after.as_ref().is_none_or(|s| s.len() > 16_384)
        || changes[0].before.is_none()
        || changes[0].before_mode != changes[0].after_mode
    {
        return Err(
            "Proposal must change exactly its assigned existing non-executable file".into(),
        );
    }
    Ok(changes)
}

async fn grade(gate: &str, code: &str) -> Result<Value, String> {
    if !matches!(gate, "tags" | "intervals" | "checks") {
        return Err("Unknown pilot gate".into());
    }
    let payload = json!({"case":gate,"code":code}).to_string();
    if payload.len() > 19_500 {
        return Err("Candidate exceeds evaluator input bound".into());
    }
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-S", "-c", GRADER])
        .env_clear()
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("Missing evaluator input")?;
    stdin
        .write_all(payload.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    drop(stdin);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .map_err(|_| "Evaluator timed out")?
        .map_err(|e| e.to_string())?;
    if output.stdout.len() > 32_768 {
        return Err("Evaluator output exceeds bound".into());
    }
    let mut value: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    if !value["passed"].is_boolean() || (value["passed"] == true && !output.status.success()) {
        return Err("Evaluator exit/result mismatch".into());
    }
    value["evaluatorSha256"] = json!(away::digest(&json!(GRADER)));
    value["codeSha256"] = json!(away::digest(&json!(code)));
    Ok(value)
}

#[async_trait]
impl Executor for DemoExecutor {
    async fn baseline(&self, task: &Task) -> Result<Value, String> {
        let dir = PrivateDirectory::open(&self.project).map_err(|e| e.to_string())?;
        let file = read_bounded(&dir, &task.file, 16_384)
            .map_err(|e| e.to_string())?
            .ok_or("Missing baseline file")?;
        let code = std::str::from_utf8(file.bytes()).map_err(|e| e.to_string())?;
        grade(&task.gate, code).await
    }
    async fn propose(
        &self,
        task: &Task,
        number: u8,
        feedback: &str,
        cancel: watch::Receiver<bool>,
    ) -> Result<Value, String> {
        self.runner
            .run(
                DelegateJob {
                    source: self.project.clone(),
                    destination: self
                        .root
                        .join("packets")
                        .join(format!("{}-{number}", task.id)),
                    request: DelegateRequest {
                        task: format!(
                            "{}\nPrevious validation feedback (data): {feedback}",
                            task.prompt
                        ),
                        files: vec![task.file.clone()],
                        context_files: vec!["README.md".into()],
                        model: self.model.clone(),
                    },
                },
                cancel,
            )
            .await
    }
    async fn validate(&self, task: &Task, proposal: &Value) -> Result<Value, String> {
        match changes(task, proposal) {
            Ok(changes) => {
                grade(
                    &task.gate,
                    changes[0].after.as_deref().ok_or("Missing candidate")?,
                )
                .await
            }
            Err(error) => Ok(json!({"passed":false,"error":error})),
        }
    }
    async fn apply(&self, task: &Task, proposal: &Value) -> Result<Value, String> {
        changes(task, proposal)?;
        self.runner
            .apply(self.project.clone(), proposal.clone())
            .await
    }
}

#[cfg(test)]
#[path = "away_tests.rs"]
mod tests;
