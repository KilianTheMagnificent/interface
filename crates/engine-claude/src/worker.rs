//! Claude Code print-mode content proposals, authenticated by the installed CLI.
//! CLI 2.1.263 safe mode disables customization without disabling subscription auth.
use interface_engine::worker::{
    self, ContentOutput, Events, RuntimeDirectory, WorkerOutput, WorkerProgress, WorkerUsage,
};
use serde_json::Value;
use std::path::Path;
use tokio::{process::Command, sync::watch};

/// # Errors
/// Refuses unsupported IDs, inherited tools, provider errors or incomplete output.
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
    let model = worker::model_name(model, "claude/")?;
    let runtime = RuntimeDirectory::create(workspace)?;
    let mut command = worker_command(executable, &runtime.0, model);
    if let Some(effort) = effort {
        command.args(["--effort", effort]);
    }
    worker::run(
        command,
        task,
        cancellation,
        progress,
        ClaudeEvents::default(),
    )
    .await
}
fn worker_command(executable: &Path, cwd: &Path, model: &str) -> Command {
    let mut command = Command::new(executable);
    command
        .args([
            "--print",
            "--verbose",
            "--output-format",
            "stream-json",
            "--no-session-persistence",
            "--safe-mode",
            "--settings",
            r#"{"enabledPlugins":{"agents-md@builtin":false,"telemetry@builtin":false}}"#,
            "--restricted",
            "--tools",
            "",
            "--disallowedTools",
            "mcp__*",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--disable-slash-commands",
            "--no-chrome",
            "--permission-mode",
            "dontAsk",
            "--model",
            model,
            "--system-prompt",
            &format!("{} Use the native StructuredOutput tool only to deliver this final report; no other tools are allowed.",worker::INSTRUCTIONS),
            "--json-schema",
            &worker::schema(),
        ])
        .current_dir(cwd)
        .env("PWD", cwd);
    for marker in crate::transport::CLAUDE_NESTING_MARKERS {
        command.env_remove(marker);
    }
    command
}
#[derive(Default)]
struct ClaudeEvents {
    session: Option<String>,
    model: Option<String>,
    result: Option<ContentOutput>,
    structured: Option<Value>,
    tool_id: Option<String>,
    tool_complete: bool,
    tool_error_count: usize,
}
impl Events for ClaudeEvents {
    fn observe(&mut self, value: &Value, progress: Option<&WorkerProgress>) -> Result<(), String> {
        if self.result.is_some() {
            return Err("Claude emitted events after completion".into());
        }
        let id = value["session_id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or("Claude session identity missing")?;
        if value["type"] == "system" && value["subtype"] == "init" && self.session.is_none() {
            if value["tools"]
                .as_array()
                .is_none_or(|tools| tools.iter().any(|tool| tool != "StructuredOutput"))
            {
                return Err("Claude worker inherited tools outside its scope".into());
            }
            for field in ["mcp_servers", "plugins", "skills"] {
                if value
                    .get(field)
                    .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
                {
                    return Err(
                        "Claude worker inherited tools or customization outside its scope".into(),
                    );
                }
            }
            self.session = Some(id.into());
            self.model = value["model"].as_str().map(str::to_owned);
            if let Some(p) = progress {
                p("status", "Claude is preparing a proposal");
            }
            return Ok(());
        }
        if self.session.as_deref() != Some(id) {
            return Err("Claude session identity changed or initialization is missing".into());
        }
        match value["type"].as_str() {
            Some("assistant") => self.assistant(value, progress)?,
            Some("system") if value["subtype"] == "thinking_tokens" => {}
            Some("rate_limit_event") => {}
            Some("user")
                if (self.tool_id.is_some() || self.tool_error_count > 0)
                    && !self.tool_complete
                    && value["parent_tool_use_id"].is_null() =>
            {
                let parts = value["message"]["content"]
                    .as_array()
                    .ok_or("Claude structured output acknowledgement missing")?;
                if parts.len() != 1
                    || parts[0]["type"] != "tool_result"
                    || parts[0]["tool_use_id"].as_str() != self.tool_id.as_deref()
                {
                    return Err("Claude emitted an unexpected tool result".into());
                }
                if parts[0]["is_error"] == true {
                    // StructuredOutput only validates an in-memory report. Let
                    // the CLI correct a rejected draft in this same turn; no
                    // file, provider request or other effect is replayed here.
                    self.tool_error_count += 1;
                    self.tool_id = None;
                    self.structured = None;
                } else {
                    self.tool_complete = true;
                }
            }
            Some("result") => {
                if (self.tool_id.is_some() || self.tool_error_count > 0)
                    && (!self.tool_complete
                        || self.structured.as_ref() != Some(&value["structured_output"]))
                {
                    return Err("Claude structured output was incomplete or changed".into());
                }

                if value["subtype"] != "success"
                    || value["is_error"] != false
                    || value["permission_denials"]
                        .as_array()
                        .is_some_and(|v| !v.is_empty())
                {
                    return Err("Claude worker did not complete successfully; no fallback or replay was attempted".into());
                }
                let usage = &value["usage"];
                self.result = Some(worker::parse_report(
                    &value["structured_output"],
                    WorkerOutput {
                        summary: String::new(),
                        session_id: self.session.clone(),
                        tool_error_count: self.tool_error_count,
                        actual_model: self.model.clone(),
                        usage: WorkerUsage {
                            input_tokens: usage["input_tokens"].as_u64(),
                            output_tokens: usage["output_tokens"].as_u64(),
                            cache_read_tokens: usage["cache_read_input_tokens"].as_u64(),
                            cache_write_tokens: usage["cache_creation_input_tokens"].as_u64(),
                            cost_usd: value["total_cost_usd"]
                                .as_f64()
                                .filter(|v| v.is_finite() && *v >= 0.0),
                            reasoning_tokens: None,
                        },
                    },
                )?);
            }
            _ => return Err("Claude emitted an unsupported worker event".into()),
        }
        Ok(())
    }
    fn finish(self) -> Result<ContentOutput, String> {
        self.result
            .ok_or_else(|| "Claude ended without a successful complete result".into())
    }
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;

impl ClaudeEvents {
    fn assistant(
        &mut self,
        value: &Value,
        progress: Option<&WorkerProgress>,
    ) -> Result<(), String> {
        if !value["parent_tool_use_id"].is_null() || value.get("error").is_some() {
            return Err("Claude reported an error or subagent activity".into());
        }
        if let Some(model) = value["message"]["model"].as_str() {
            self.model = Some(model.into());
        }
        for part in value["message"]["content"]
            .as_array()
            .ok_or("Claude message content missing")?
        {
            match part["type"].as_str() {
                Some("thinking" | "redacted_thinking") => {}
                Some("text") => {
                    if let (Some(p), Some(text)) = (progress, part["text"].as_str()) {
                        p("message", text);
                    }
                }
                Some("tool_use")
                    if part["name"] == "StructuredOutput" && self.tool_id.is_none() =>
                {
                    self.tool_id = Some(
                        part["id"]
                            .as_str()
                            .filter(|id| !id.is_empty() && id.len() <= 128)
                            .ok_or("Claude structured output identity missing")?
                            .into(),
                    );
                    self.structured = Some(part["input"].clone());
                }
                _ => {
                    return Err(
                        "Claude attempted a tool outside its content-only worker scope".into(),
                    );
                }
            }
        }

        Ok(())
    }
}
