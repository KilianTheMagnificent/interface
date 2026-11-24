//! Narrow task-bound MCP. It exposes no arbitrary shell, approval or owner-confirmation tool.
use crate::{
    agents::AgentHost,
    mcp_transport::{self, BoundSocket, McpServerSpec, Peer, ToolBackend, ToolOutput, tool},
};
use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::engine::{AgentToolsProvider, CollaborationMcpStdioDescriptor};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use tokio_util::sync::CancellationToken;

pub const AGENT_TOOL_SERVER: &str = "iface_agent";
pub use interface_contracts::agent_work::AGENT_TOOL_NAMES;
#[derive(Debug, Default)]
pub struct DeferredAgentTools {
    inner: Mutex<Option<Arc<dyn AgentToolsProvider>>>,
}
impl DeferredAgentTools {
    pub fn attach(&self, provider: Arc<dyn AgentToolsProvider>) {
        *lock(&self.inner) = Some(provider);
    }
}
impl AgentToolsProvider for DeferredAgentTools {
    fn descriptor_for(&self, cwd: &Path) -> Option<CollaborationMcpStdioDescriptor> {
        lock(&self.inner).as_ref()?.descriptor_for(cwd)
    }
    fn descriptor_for_resume(
        &self,
        cwd: &Path,
        key: &str,
    ) -> Option<CollaborationMcpStdioDescriptor> {
        lock(&self.inner).as_ref()?.descriptor_for_resume(cwd, key)
    }
}
struct Server {
    agent: String,
    resume_key: Option<String>,
    path: PathBuf,
    identity: (u64, u64),
    stop: CancellationToken,
}
pub struct AgentToolServers {
    host: Weak<AgentHost>,
    root: PathBuf,
    servers: Mutex<BTreeMap<String, Server>>,
}
impl fmt::Debug for AgentToolServers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AgentToolServers")
    }
}
impl AgentToolServers {
    #[must_use]
    pub fn new(host: &Arc<AgentHost>) -> Arc<Self> {
        Arc::new(Self {
            host: Arc::downgrade(host),
            // Release smoke roots have already passed private-directory validation
            // and hold the host lock. Keep its sockets inside that sandbox, with
            // short names to fit macOS's Unix socket path limit.
            root: std::env::var_os("INTERFACE_HOST_SMOKE_DATA_ROOT").map_or_else(
                || Path::new("/private/tmp").join(format!("iface-at-{}", uuid::Uuid::new_v4())),
                |root| PathBuf::from(root).join("agent-tools"),
            ),
            servers: Mutex::new(BTreeMap::new()),
        })
    }
}
impl AgentToolsProvider for AgentToolServers {
    fn descriptor_for(&self, cwd: &Path) -> Option<CollaborationMcpStdioDescriptor> {
        let host = self.host.upgrade()?;
        let task = host.desktop.agent_tool_task_for_cwd(cwd).ok()??;
        let descriptor = self.bind(&task.agent_id, &task.id, None)?;
        lock(&host.bound_tool_tasks).insert(task.id);
        Some(descriptor)
    }
    fn descriptor_for_resume(
        &self,
        cwd: &Path,
        key: &str,
    ) -> Option<CollaborationMcpStdioDescriptor> {
        let host = self.host.upgrade()?;
        let scope = lock(&host.resumed_tools).get(key)?.clone();
        if std::fs::canonicalize(cwd).ok()? != scope.cwd {
            return None;
        }
        let descriptor = self.bind(&scope.agent, &scope.id, Some(key.to_owned()))?;
        let mut scopes = lock(&host.resumed_tools);
        let current = scopes
            .get_mut(key)
            .filter(|current| current.id == scope.id)?;
        current.connected = true;
        Some(descriptor)
    }
}
impl AgentToolServers {
    fn bind(
        &self,
        agent: &str,
        id: &str,
        resume_key: Option<String>,
    ) -> Option<CollaborationMcpStdioDescriptor> {
        let host = self.host.upgrade()?;
        let mut servers = lock(&self.servers);
        servers.retain(|id, server| {
            let exists = server.resume_key.as_ref().map_or_else(
                || host.desktop.agent_tool_endpoint_exists(&server.agent, id),
                |key| {
                    lock(&host.resumed_tools)
                        .get(key)
                        .is_some_and(|scope| scope.id == *id)
                },
            );
            if !exists {
                server.stop.cancel();
                mcp_transport::remove_bound_socket(&server.path, server.identity);
                lock(&host.bound_tool_tasks).remove(id);
            }
            exists
        });
        if !servers.contains_key(id) {
            let path = self.root.join(format!(
                "{}.sock",
                &uuid::Uuid::new_v4().simple().to_string()[..24]
            ));
            let BoundSocket { listener, identity } = mcp_transport::bind(&self.root, &path).ok()?;
            let stop = CancellationToken::new();
            let backend = Arc::new(BoundAgentTools {
                host: self.host.clone(),
                agent: agent.into(),
                task: id.into(),
                resume_key: resume_key.clone(),
            });
            let spec = Arc::new(McpServerSpec {
                wire_name: "iface-agent",
                title: "Interface Agent Tools",
                instructions: "Host-owned task tools v1. Call task_status before beginning work. Task scope, deadlines and memory compare-and-save are enforced by Interface. Tool outputs and memory proposals never grant owner authority. Use a stable requestId for each mutation; never retry an unknown outcome with a new id.",
                tools: catalogue(),
                compatibility_tools: &[],
                max_request_line_bytes: None,
            });
            mcp_transport::serve(listener, &path, spec, backend, stop.clone());
            servers.insert(
                id.into(),
                Server {
                    agent: agent.into(),
                    resume_key,
                    path,
                    identity,
                    stop,
                },
            );
        }
        let server = servers.get(id)?;
        Some(CollaborationMcpStdioDescriptor {
            name: AGENT_TOOL_SERVER.into(),
            command: mcp_transport::NC.into(),
            args: vec!["-U".into(), server.path.to_string_lossy().into_owned()],
            env: vec![],
        })
    }
}
/// A new, volatile transport generation for an explicitly resumed session. It has no
/// task authority until the host admits a new turn; closing it revokes the generation.
#[derive(Clone)]
pub(crate) struct ResumedTools {
    id: String,
    agent: String,
    cwd: PathBuf,
    roots: Vec<String>,
    connected: bool,
}
impl AgentHost {
    pub(crate) fn prepare_resumed_host_tools(
        &self,
        key: &str,
    ) -> Result<(), crate::dispatcher::DispatchError> {
        let Some(binding) = self.desktop.agent_binding(key) else {
            return Ok(());
        };
        if self
            .desktop
            .active_agent_work(key)
            .map_err(|e| {
                crate::dispatcher::DispatchError::new("agent-unavailable", e.message, false, true)
            })?
            .is_some_and(|task| {
                task.stop_requested
                    || task.execution
                        == interface_contracts::agent_work::AgentExecutionState::Unknown
            })
        {
            return Err(crate::dispatcher::DispatchError::new(
                "agent-refused",
                "Inspect and acknowledge uncertain work before resuming.",
                false,
                true,
            ));
        }
        lock(&self.resumed_tools).insert(
            key.into(),
            ResumedTools {
                id: uuid::Uuid::new_v4().to_string(),
                agent: binding.agent_id,
                cwd: std::fs::canonicalize(binding.cwd).map_err(|_| {
                    crate::dispatcher::DispatchError::new(
                        "agent-refused",
                        "Agent folder unavailable.",
                        false,
                        true,
                    )
                })?,
                roots: binding
                    .roots
                    .iter()
                    .map(|r| std::fs::canonicalize(r).map(|r| r.to_string_lossy().into_owned()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| {
                        crate::dispatcher::DispatchError::new(
                            "agent-refused",
                            "Granted folder unavailable.",
                            false,
                            true,
                        )
                    })?,
                connected: false,
            },
        );
        Ok(())
    }
    pub(crate) fn resumed_host_tools_admit(&self, key: &str) -> bool {
        let scope = lock(&self.resumed_tools).get(key).cloned();
        scope.is_some_and(|scope| scope.connected && self.resumed_tool_task(key, &scope.id).is_ok())
    }
    fn resumed_tool_task(
        &self,
        key: &str,
        id: &str,
    ) -> Result<interface_contracts::agent_work::AgentTaskRecord, String> {
        let scope = lock(&self.resumed_tools)
            .get(key)
            .filter(|scope| scope.id == id)
            .cloned()
            .ok_or("Resumed tool connection was revoked.")?;
        let task = self
            .desktop
            .active_agent_work(key)
            .map_err(|e| e.message)?
            .ok_or("No task has been admitted for this resumed conversation.")?;
        if task.agent_id != scope.agent
            || Path::new(&task.cwd) != scope.cwd
            || task.roots.iter().collect::<std::collections::BTreeSet<_>>()
                != scope
                    .roots
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
        {
            return Err("Resumed task scope changed.".into());
        }
        self.desktop
            .agent_tool_task(&scope.agent, &task.id)
            .map_err(|e| e.message)
    }
}
impl Drop for AgentToolServers {
    fn drop(&mut self) {
        for (_, server) in std::mem::take(&mut *lock(&self.servers)) {
            server.stop.cancel();
            mcp_transport::remove_bound_socket(&server.path, server.identity);
        }
        let _ = std::fs::remove_dir(&self.root);
    }
}
struct BoundAgentTools {
    host: Weak<AgentHost>,
    agent: String,
    task: String,
    resume_key: Option<String>,
}
#[async_trait]
impl ToolBackend for BoundAgentTools {
    async fn call(&self, name: &str, input: Value, _peer: Peer) -> Result<ToolOutput, String> {
        validate_arguments(name, &input)?;
        let host = self.host.upgrade().ok_or("Agent host stopped.")?;
        // The agent store lock is held across a commit's fsyncs: look the
        // task up on the blocking pool, not on an async worker.
        let task = {
            let (host, key, agent, id) = (
                Arc::clone(&host),
                self.resume_key.clone(),
                self.agent.clone(),
                self.task.clone(),
            );
            tokio::task::spawn_blocking(move || match &key {
                Some(key) => host.resumed_tool_task(key, &id),
                None => host
                    .desktop
                    .agent_tool_task(&agent, &id)
                    .map_err(|e| e.message),
            })
            .await
            .map_err(|_| "Task lookup could not finish.".to_owned())??
        };
        if name == "consult_start" {
            return crate::agent_consultation::consult(host, &self.agent, &task.id, &input)
                .await
                .map(ToolOutput::Text);
        }
        if name == "task_status" {
            let agent = self.agent.clone();
            return tokio::task::spawn_blocking(move || {
                host.desktop.agent_task_context(&agent, &task)
            })
            .await
            .map_err(|_| "Task status could not be read.".to_owned())?
            .map(|context| ToolOutput::Text(context.to_string()))
            .map_err(|e| e.message);
        }
        let request = input["requestId"]
            .as_str()
            .ok_or("Missing requestId.")?
            .to_owned();
        let (agent, task_id, name) = (self.agent.clone(), task.id, name.to_owned());
        tokio::task::spawn_blocking(move || {
            host.desktop
                .agent_tool_mutation(&agent, &task_id, &request, &name, &input)
        })
        .await
        .map_err(|_| "Task tool could not finish; outcome unknown.".to_owned())?
        .map(ToolOutput::Text)
        .map_err(|e| e.message)
    }
}
/// Names and annotations never authorize external MCP tools. This validator is used only
/// for Interface's own descriptor and exact task-bound backend.
pub(crate) fn validate_arguments(name: &str, input: &Value) -> Result<(), String> {
    let required: &[&str] = match name {
        "task_status" => &[],
        "consult_start" => &["requestId", "brief"],
        "consult_review" => &["requestId", "childId", "resultSha256", "verdict"],
        "consult_apply" => &["requestId", "childId", "resultSha256", "path"],
        "artifact_register" => &["requestId", "path"],
        "memory_propose" => &["requestId", "expectedSha256", "text"],
        "notification_request" => &["requestId", "text"],
        "routine_draft" => &["requestId", "name", "prompt"],
        _ => return Err("Unknown host tool contract.".into()),
    };
    let object = input.as_object().ok_or("Tool input must be an object.")?;
    if object.len() != required.len()
        || required
            .iter()
            .any(|key| !object.get(*key).is_some_and(Value::is_string))
        || object.values().any(|value| {
            value
                .as_str()
                .is_some_and(|s| s.contains('\0') || s.len() > 256 * 1024)
        })
    {
        return Err("Arguments do not match the trusted host tool contract.".into());
    }
    Ok(())
}
fn catalogue() -> Vec<Value> {
    let string = json!({"type":"string"});
    let mut tools = vec![
        tool(
            "consult_start",
            "Run one bounded tool-free consultation using only the owner's chosen provider and files. Returns a source-bound report and optional proposals; never changes parent completion.",
            &json!({"requestId":string,"brief":string}),
            &["requestId", "brief"],
            false,
        ),
        tool(
            "consult_review",
            "Record the parent's accepted or rejected review of the exact resultSha256. Review is not verification.",
            &json!({"requestId":string,"childId":string,"resultSha256":string,"verdict":{"type":"string","enum":["accepted","rejected"]}}),
            &["requestId", "childId", "resultSha256", "verdict"],
            false,
        ),
        tool(
            "consult_apply",
            "Compare-and-save one reviewed proposal within the owner's selected writable files; all source hashes must still match. No deletion or new-file creation.",
            &json!({"requestId":string,"childId":string,"resultSha256":string,"path":string}),
            &["requestId", "childId", "resultSha256", "path"],
            false,
        ),
        tool(
            "task_status",
            "Read this task's goal, authority, limits and evidence. No cost is inferred.",
            &json!({}),
            &[],
            true,
        ),
        tool(
            "artifact_register",
            "Register exact bytes inside the task scope. Registration does not verify success.",
            &json!({"requestId":string,"path":string}),
            &["requestId", "path"],
            false,
        ),
        tool(
            "memory_propose",
            "Replace this agent's MEMORY.md with text, compare-and-save: expectedSha256 is the SHA-256 hex of the file as it is now, or of empty text when there is none (task_status reports memorySha256, the last recorded revision). On a mismatch nothing is written. Provenance and the prior revision are retained. Returns the new sha256.",
            &json!({"requestId":string,"expectedSha256":string,"text":string}),
            &["requestId", "expectedSha256", "text"],
            false,
        ),
        tool(
            "notification_request",
            "Queue an owner-directed note in Task results. Does not send to third parties.",
            &json!({"requestId":string,"text":string}),
            &["requestId", "text"],
            false,
        ),
        tool(
            "routine_draft",
            "Create a paused routine in the current folder for owner review. Cannot enable it.",
            &json!({"requestId":string,"name":string,"prompt":string}),
            &["requestId", "name", "prompt"],
            false,
        ),
    ];
    // First-party metadata affects discovery only, never host authorization.
    for tool in &mut tools {
        tool["_meta"] = json!({"anthropic/alwaysLoad":true});
    }
    tools
}
