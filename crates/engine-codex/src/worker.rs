//! Codex `exec` content proposals. The CLI remains the authentication boundary.
//! Flags verified with codex-cli 0.153.4; unsupported controls fail at startup.
use interface_common::sync::lock;
use interface_engine::{
    FileVersion,
    worker::{
        self, ContentOutput, Events, RuntimeDirectory, WorkerOutput, WorkerProgress, WorkerUsage,
    },
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
use tokio::{process::Command, sync::watch};

/// Each executable's parsed `codex debug models --bundled` answer, a function
/// of that file alone: kept while its path resolves to the same file, asked
/// again after an upgrade. A failed capture or parse is never kept.
type CatalogCache = Mutex<HashMap<PathBuf, (FileVersion, Arc<Value>)>>;
static BUNDLED_CATALOGS: LazyLock<CatalogCache> = LazyLock::new(Mutex::default);
/// More executables than any install has; past it the cache starts over.
const KEPT_CATALOGS: usize = 64;

/// # Errors
/// Refuses unsupported IDs, provider errors, tool activity or incomplete output.
/// Explicit unsupported effort is refused before starting a provider.
#[allow(clippy::too_many_arguments)]
pub async fn run_with_effort(
    executable: &Path,
    workspace: &Path,
    model: &str,
    task: &str,
    cancellation: watch::Receiver<bool>,
    progress: Option<WorkerProgress>,
    effort: Option<&str>,
) -> Result<ContentOutput, String> {
    if effort.is_some_and(|value| !matches!(value, "low" | "medium" | "high" | "xhigh")) {
        return Err("Unsupported content-worker effort; no provider started".into());
    }
    let model = worker::model_name(model, "codex/")?;
    let runtime = RuntimeDirectory::create(workspace)?;
    let bundled = bundled_catalog(executable, &runtime.0, &cancellation).await?;
    let catalog = restricted_catalog(&bundled, model)?;
    let catalog_path = runtime.0.join("model-catalog.json");
    std::fs::write(&catalog_path, catalog.to_string())
        .map_err(|_| "Codex worker catalog unavailable")?;
    let schema = runtime.0.join("response-schema.json");
    std::fs::write(&schema, worker::schema()).map_err(|_| "Codex worker schema unavailable")?;
    let mut command = worker_command(executable, &runtime.0, model, &schema);
    if let Some(effort) = effort {
        command.args(["-c", &format!("model_reasoning_effort={effort:?}")]);
    }
    command.args([
        "-c",
        &format!(
            "model_catalog_json={}",
            serde_json::to_string(&catalog_path).map_err(|_| "Codex catalog path invalid")?
        ),
    ]);
    worker::run(
        command,
        task,
        cancellation,
        progress,
        CodexEvents::default(),
    )
    .await
}

// Newer models force Code Mode and v2 subagents even when their feature flags
// are false. Keep native identity/instructions/limits, but remove those tool
// requirements and patch capability in this private catalog. Unknown models
// fail before a model call; there is no metadata or provider fallback.
async fn bundled_catalog(
    executable: &Path,
    cwd: &Path,
    cancellation: &watch::Receiver<bool>,
) -> Result<Arc<Value>, String> {
    let version = FileVersion::of(executable);
    if let Some(version) = version
        && let Some((kept, catalog)) = lock(&BUNDLED_CATALOGS).get(executable)
        && *kept == version
    {
        return Ok(Arc::clone(catalog));
    }
    let mut metadata = Command::new(executable);
    metadata
        .args(["debug", "models", "--bundled"])
        .current_dir(cwd);
    let bytes = worker::capture(metadata, cancellation.clone()).await?;
    let catalog = Arc::new(parse_catalog(&bytes)?);
    if let Some(version) = version {
        let mut kept = lock(&BUNDLED_CATALOGS);
        if kept.len() >= KEPT_CATALOGS && !kept.contains_key(executable) {
            kept.clear();
        }
        kept.insert(executable.to_path_buf(), (version, Arc::clone(&catalog)));
    }
    Ok(catalog)
}

fn parse_catalog(bytes: &[u8]) -> Result<Value, String> {
    Ok(serde_json::from_slice(bytes).map_err(|_| "Codex bundled model catalog is invalid")?)
}

fn restricted_catalog(catalog: &Value, model: &str) -> Result<Value, String> {
    let mut entry = catalog["models"].as_array().and_then(|models| models.iter().find(|m| m["slug"] == model)).cloned().ok_or("Codex model is not in the installed CLI catalog; update Codex or choose an explicit supported model. No fallback was started")?;
    entry["tool_mode"] = Value::Null;
    entry["apply_patch_tool_type"] = Value::Null;
    entry["multi_agent_version"] = Value::String("v1".into());
    Ok(serde_json::json!({"models":[entry]}))
}

fn worker_command(executable: &Path, cwd: &Path, model: &str, schema: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .args([
            "exec",
            "--ignore-user-config",
            "--strict-config",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "--json",
            "--color",
            "never",
            "--model",
            model,
            "--output-schema",
        ])
        .arg(schema)
        .current_dir(cwd)
        .env("PWD", cwd)
        .env_remove("CODEX_THREAD_ID")
        .env_remove("CODEX_INTERNAL_ORIGINATOR_OVERRIDE");
    for config in [
        "approval_policy=\"never\"",
        "web_search=\"disabled\"",
        "project_doc_max_bytes=0",
        "check_for_update_on_startup=false",
        "features.skip_host_skill_discovery=true",
        "suppress_unstable_features_warning=true",
    ] {
        command.args(["-c", config]);
    }
    command.args([
        "-c",
        &format!(
            "developer_instructions={}",
            serde_json::to_string(worker::INSTRUCTIONS).expect("literal instruction string")
        ),
    ]);
    for feature in [
        "shell_tool",
        "unified_exec",
        "apps",
        "plugins",
        "memories",
        "multi_agent",
        "multi_agent_v2",
        "browser_use",
        "computer_use",
        "image_generation",
        "code_mode",
        "code_mode_host",
        "hooks",
        "shell_snapshot",
        "skill_search",
        "tool_suggest",
        "view_image",
        "workspace_dependencies",
    ] {
        command.args(["--disable", feature]);
    }
    command.arg("-");
    command
}

#[derive(Default)]
struct CodexEvents {
    session: Option<String>,
    started: bool,
    text: Option<String>,
    result: Option<ContentOutput>,
}
impl Events for CodexEvents {
    fn observe(&mut self, value: &Value, progress: Option<&WorkerProgress>) -> Result<(), String> {
        if self.result.is_some() {
            return Err("Codex emitted events after completion".into());
        }
        match value["type"].as_str() {
            Some("thread.started") if self.session.is_none() => {
                let id = value["thread_id"].as_str().filter(|s| !s.is_empty() && s.len() <= 128).ok_or("Codex thread identity missing")?;
                self.session = Some(id.into());
                if let Some(p) = progress { p("status", "Codex is preparing a proposal"); }
            }
            Some("turn.started") if self.session.is_some() && !self.started => self.started = true,
            Some("item.started" | "item.updated" | "item.completed") if self.started => {
                let item = &value["item"];
                match item["type"].as_str() {
                    Some("reasoning") => {}, // Never expose private reasoning.
                    Some("agent_message") => {
                        if value["type"] == "item.completed" {
                            let text = item["text"].as_str().ok_or("Codex message text missing")?;
                            if let Some(p) = progress { p("message", text); }
                            self.text = Some(text.into());
                        }
                    }
                    _ => return Err("Codex reported an error or attempted activity outside its content-only worker scope".into()),
                }
            }
            Some("turn.completed") if self.started => {
                let report: Value = serde_json::from_str(self.text.as_deref().ok_or("Codex final proposal missing")?).map_err(|_| "Codex final proposal is not JSON")?;
                let usage = &value["usage"];
                self.result = Some(worker::parse_report(&report, WorkerOutput {
                    summary: String::new(), session_id: self.session.clone(), tool_error_count: 0, actual_model: None,
                    usage: WorkerUsage { input_tokens: usage["input_tokens"].as_u64(), output_tokens: usage["output_tokens"].as_u64(), cache_read_tokens: usage["cached_input_tokens"].as_u64(),
                        cache_write_tokens: usage["cache_write_input_tokens"].as_u64(),
                        reasoning_tokens: usage["reasoning_output_tokens"].as_u64(), ..WorkerUsage::default() },
                })?);
            }
            _ => return Err("Codex worker failed or emitted an unsupported event; no fallback or replay was attempted".into()),
        }
        Ok(())
    }
    fn finish(self) -> Result<ContentOutput, String> {
        self.result
            .ok_or_else(|| "Codex ended without a successful complete result".into())
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
