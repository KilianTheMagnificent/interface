//! Host adapter for the existing tool-free Claude/Codex content workers.
use crate::agents::AgentHost;
use interface_engine::worker::ContentOutput;
use interface_storage::PrivateDirectory;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

/// Every agent-store call below commits or reads under the store lock, which a
/// commit holds across its fsyncs: run them on the blocking pool, never on an
/// async worker the event pumps and the stdio writer share.
async fn store<T: Send + 'static>(
    host: &AgentHost,
    operation: impl FnOnce(&interface_desktop_services::DesktopService) -> T + Send + 'static,
) -> Result<T, String> {
    crate::agents::blocking(&host.desktop, operation)
        .await
        .ok_or_else(|| "Agent task store work did not finish; outcome unknown.".to_owned())
}

pub(crate) async fn consult(
    host: Arc<AgentHost>,
    agent: &str,
    task: &str,
    input: &Value,
) -> Result<String, String> {
    let request = input["requestId"]
        .as_str()
        .ok_or("Missing requestId.")?
        .to_owned();
    let reserved = {
        let (agent, task, request, input) = (
            agent.to_owned(),
            task.to_owned(),
            request.clone(),
            input.clone(),
        );
        store(&host, move |desktop| {
            desktop.reserve_agent_tool(&agent, &task, &request, "consult_start", &input)
        })
        .await?
        .map_err(|e| e.message)?
    };
    if let Some(result) = reserved {
        return Ok(result);
    }
    let result = execute(
        &host,
        agent,
        task,
        input["brief"].as_str().ok_or("Missing brief.")?,
    )
    .await;
    let text = result.unwrap_or_else(|error| json!({"error":error,"retryable":false}).to_string());
    {
        let (task, text) = (task.to_owned(), text.clone());
        store(&host, move |desktop| {
            desktop.settle_agent_tool(&task, &request, &text)
        })
        .await?
        .map_err(|e| e.message)?;
    }
    Ok(text)
}
async fn execute(host: &AgentHost, agent: &str, task: &str, brief: &str) -> Result<String, String> {
    let (child, packet, remaining_ms) = {
        let (agent, task, brief) = (agent.to_owned(), task.to_owned(), brief.to_owned());
        store(host, move |desktop| {
            desktop.prepare_agent_consultation(&agent, &task, &brief)
        })
        .await?
        .map_err(|e| e.message)?
    };
    let began = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_millis(remaining_ms),
        run(host, agent, task, &child.model, &packet, remaining_ms),
    )
    .await
    .unwrap_or_else(|_| Err("Consultation deadline reached; no request will be replayed.".into()));
    let completed = match result {
        Ok(output) => {
            let tokens = output
                .output
                .usage
                .input_tokens
                .zip(output.output.usage.output_tokens)
                .map(|(a, b)| a.saturating_add(b));
            let elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
            let (agent, task, child_id) = (agent.to_owned(), task.to_owned(), child.id.clone());
            store(host, move |desktop| {
                desktop.finish_agent_consultation(
                    &agent,
                    &task,
                    &child_id,
                    &output.output.summary,
                    output
                        .files
                        .into_iter()
                        .map(|file| (file.path, file.after))
                        .collect(),
                    tokens,
                    elapsed_ms,
                )
            })
            .await
            .and_then(|finished| finished.map_err(|e| e.message))
        }
        Err(error) => Err(error),
    };
    if let Err(error) = &completed {
        let (task, child_id, error) = (task.to_owned(), child.id.clone(), error.clone());
        store(host, move |desktop| {
            desktop.fail_agent_consultation(&task, &child_id, &error)
        })
        .await?
        .map_err(|e| e.message)?;
    }
    completed
}
async fn run(
    host: &AgentHost,
    agent: &str,
    task: &str,
    model: &str,
    packet: &str,
    remaining_ms: u64,
) -> Result<ContentOutput, String> {
    let provider = model.split('/').next().ok_or("Missing provider.")?;
    // Probes only the owner's chosen provider; the others are never launched.
    let executable = host
        .ready_executable(provider)
        .await
        .filter(|path| Path::new(path).is_absolute())
        .ok_or("Consultation provider is unavailable; no alternate provider was used.")?;
    let scratch = Scratch(
        PrivateDirectory::open_or_create(
            std::env::temp_dir().join(format!("iface-consult-{}", uuid::Uuid::new_v4())),
        )
        .map_err(|_| "Private consultation runtime unavailable.")?,
    );
    let workspace = scratch.0.path().join("packet");
    let (cancel, cancelled) = watch::channel(false);
    let check = || {
        let (agent, task) = (agent.to_owned(), task.to_owned());
        store(host, move |desktop| {
            desktop
                .check_agent_consultation(&agent, &task)
                .map_err(|e| e.message)
        })
    };
    check().await??;
    let work = run_provider(
        Path::new(&executable),
        &workspace,
        provider,
        model,
        packet,
        cancelled,
    );
    tokio::pin!(work);
    let mut poll = tokio::time::interval(Duration::from_millis(250));
    let deadline = tokio::time::sleep(Duration::from_millis(remaining_ms));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = &mut work => return result,
            () = &mut deadline => { let _ = cancel.send(true); return work.await.and(Err("Consultation deadline reached.".into())); },
            _ = poll.tick() => {
                if check().await.and_then(|checked| checked).is_err() {
                    let _ = cancel.send(true);
                    return work.await.and(Err("Parent stopped or exhausted its budget; consultation cancelled.".into()));
                }
            }
        }
    }
}
async fn run_provider(
    executable: &Path,
    workspace: &Path,
    provider: &str,
    model: &str,
    packet: &str,
    cancelled: watch::Receiver<bool>,
) -> Result<ContentOutput, String> {
    match provider {
        "claude" => {
            engine_claude::worker::run_with_effort(
                executable,
                workspace,
                model,
                packet,
                cancelled,
                None,
                Some("low"),
            )
            .await
        }
        "codex" => {
            engine_codex::worker::run_with_effort(
                executable,
                workspace,
                model,
                packet,
                cancelled,
                None,
                Some("low"),
            )
            .await
        }
        _ => Err("Unsupported consultation provider.".into()),
    }
}
struct Scratch(PrivateDirectory);
impl Drop for Scratch {
    fn drop(&mut self) {
        let path: PathBuf = self.0.path().into();
        let _ = std::fs::remove_dir_all(path);
    }
}
