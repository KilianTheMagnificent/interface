use super::{Options, transport};
use interface_collaboration::workers::{MCP_SERVER_NAME, TOOL_NAMES};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, process::Stdio};
use tokio::process::Command;

pub(super) async fn run(options: &Options) -> Result<String, String> {
    finish(options, build_command(options)?).await
}
fn build_command(options: &Options) -> Result<Command, String> {
    let client = options.client()?;
    let executable = options.path("--executable")?;
    let iface = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let model = options.values.get("--model");
    if model.is_some_and(|s| s.is_empty() || s.len() > 128 || s.chars().any(char::is_control)) {
        return Err("--model must name an exact lead model".into());
    }
    let effort = options
        .values
        .get("--effort")
        .map(String::as_str)
        .or_else(|| options.values.contains_key("--prompt").then_some("low"));
    if effort.is_some_and(|s| !matches!(s, "minimal" | "low" | "medium" | "high" | "xhigh" | "max"))
    {
        return Err("invalid lead effort".into());
    }
    let prompt = options.values.get("--prompt");
    if prompt.is_some_and(|p| p.len() > 32_768) {
        return Err("lead prompt exceeds 32 KiB".into());
    }
    let mut command = Command::new(executable);
    if client == "claude" {
        if let Some(model) = model {
            command.args(["--model", model]);
        }
        if let Some(effort) = effort {
            command.args(["--effort", effort]);
        }
        command.arg("--mcp-config")
            .arg(json!({"mcpServers":{MCP_SERVER_NAME:{"command":iface,"args":["workers","mcp","--root",options.root]}}}).to_string())
            .arg("--settings").arg(claude_hooks(&iface, &options.root)?.to_string());
        if let Some(prompt) = prompt {
            command
                .args([
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--restricted",
                    "--strict-mcp-config",
                    "--no-session-persistence",
                    "--tools",
                    "",
                    "--allowedTools",
                    &format!("mcp__{MCP_SERVER_NAME}"),
                    "--permission-mode",
                    "dontAsk",
                    "--disable-slash-commands",
                ])
                .arg(prompt);
        }
    } else {
        if prompt.is_some() {
            command.args([
                "exec",
                "--json",
                "--ephemeral",
                "--ignore-user-config",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
            ]);
        }
        if let Some(model) = model {
            command.args(["--model", model]);
        }
        if let Some(effort) = effort {
            command
                .arg("-c")
                .arg(format!("model_reasoning_effort={}", json!(effort)));
        }
        for option in codex_config(&iface, &options.root)? {
            command.arg("-c").arg(option);
        }
        if let Some(prompt) = prompt {
            for option in [
                "features.shell_tool=false",
                "features.multi_agent=false",
                "features.apps=false",
                "features.skills=false",
                "features.plugins=false",
                "web_search=\"disabled\"",
                "project_doc_max_bytes=0",
            ] {
                command.arg("-c").arg(option);
            }
            command.arg(prompt);
        }
    }
    Ok(command)
}
async fn finish(options: &Options, mut command: Command) -> Result<String, String> {
    let client = options.client()?;
    let model = options.values.get("--model");
    let binding =
        transport::request(&options.root, json!({"method":"bind","client":client})).await?;
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "lead CLI could not start; no retry performed")?;
    let status = child.wait().await.map_err(|_| "lead exit unknown")?;
    if !status.success() {
        return Err(format!(
            "lead exited unsuccessfully ({status}); worker evidence is retained for {}",
            binding["ownerConversationRef"]
        ));
    }
    Ok(format!(
        "{}\n",
        json!({"type":"worker_lead_finished","client":client,"requestedModel":model,"ownerConversationRef":binding["ownerConversationRef"]})
    ))
}

fn quote(value: &str) -> Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("invalid hook command path".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}
fn hook_command(iface: &Path, root: &Path, client: &str, event: &str) -> Result<String, String> {
    Ok(format!(
        "{} workers notice --root {} --client {client} --event {event}",
        quote(iface.to_str().ok_or("non-UTF8 executable")?)?,
        quote(root.to_str().ok_or("non-UTF8 root")?)?
    ))
}
fn claude_hooks(iface: &Path, root: &Path) -> Result<Value, String> {
    let mut hooks = json!({});
    for event in ["SessionStart", "UserPromptSubmit", "PostToolUse"] {
        hooks[event] = json!([{"hooks":[{"type":"command","command":hook_command(iface,root,"claude",event)?,"timeout":5}]}]);
    }
    Ok(json!({"hooks":hooks}))
}
fn codex_config(iface: &Path, root: &Path) -> Result<Vec<String>, String> {
    let mut options = vec![
        format!("mcp_servers.{MCP_SERVER_NAME}.command={}", json!(iface)),
        format!(
            "mcp_servers.{MCP_SERVER_NAME}.args={}",
            json!([
                "workers",
                "mcp",
                "--root",
                root.to_str().ok_or("non-UTF8 root")?
            ])
        ),
        format!("mcp_servers.{MCP_SERVER_NAME}.tool_timeout_sec=40"),
        "features.hooks=true".into(),
    ];
    let mut trust = Vec::new();
    // The owner explicitly opted into this launcher. Approve only its six
    // fixed report tools, as Claude's scoped --allowedTools does. This is not
    // a global permission/sandbox override; broker grants still gate exports.
    for tool in TOOL_NAMES {
        options.push(format!(
            "mcp_servers.{MCP_SERVER_NAME}.tools.{tool}.approval_mode=\"approve\""
        ));
    }
    for (event, identity) in [
        ("SessionStart", "session_start"),
        ("UserPromptSubmit", "user_prompt_submit"),
        ("PostToolUse", "post_tool_use"),
    ] {
        let command = hook_command(iface, root, "codex", event)?;
        options.push(format!(
            "hooks.{event}=[{{hooks=[{{type=\"command\",command={},timeout=5}}]}}]",
            json!(command)
        ));
        // Same exact-definition, process-local trust contract as the existing
        // Interface Codex launcher. No global bypass and no user config writes.
        let definition = json!({"event_name":identity,"hooks":[{"async":false,"command":command,"timeout":5,"type":"command"}]});
        let hash = format!(
            "sha256:{:x}",
            Sha256::digest(definition.to_string().as_bytes())
        );
        trust.push(format!(
            "\"/<session-flags>/config.toml:{identity}:0:0\"={{trusted_hash=\"{hash}\"}}"
        ));
    }
    options.push(format!("hooks.state={{{}}}", trust.join(",")));
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn everyday_launches_preserve_lead_settings_and_test_runs_are_isolated() {
        for client in ["claude", "codex"] {
            let args: Vec<String> = [
                "launch",
                "--root",
                "/tmp/workers",
                "--client",
                client,
                "--executable",
                "/bin/echo",
            ]
            .into_iter()
            .map(String::from)
            .collect();
            let options = Options::parse(&args).unwrap();
            let command = build_command(&options).unwrap();
            let emitted: Vec<_> = command
                .as_std()
                .get_args()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            assert!(!emitted.iter().any(|v| v == "--effort"
                || v.contains("model_reasoning_effort")
                || v == "--strict-mcp-config"
                || v == "--ignore-user-config"
                || v == "--model"));
            assert!(emitted.iter().any(|v| v.contains("cheap_agents")));
            assert!(!emitted.iter().any(|v| v.contains("interface_workers")));
            if client == "claude" {
                let config = emitted
                    .windows(2)
                    .find(|pair| pair[0] == "--mcp-config")
                    .unwrap();
                let config: Value = serde_json::from_str(&config[1]).unwrap();
                assert!(config["mcpServers"]["cheap_agents"].is_object());
                assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
            } else {
                assert!(
                    emitted
                        .iter()
                        .any(|v| v.starts_with("mcp_servers.cheap_agents.command="))
                );
                assert!(emitted.iter().any(|v| v
                    == "mcp_servers.cheap_agents.tools.worker_start.approval_mode=\"approve\""));
            }
            let mut args = args;
            args.extend([
                "--prompt".into(),
                "fixture".into(),
                "--model".into(),
                if client == "claude" {
                    "claude-opus-5".into()
                } else {
                    "gpt-5.6-sol".into()
                },
            ]);
            let command = build_command(&Options::parse(&args).unwrap()).unwrap();
            let emitted: Vec<_> = command
                .as_std()
                .get_args()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            assert!(
                emitted
                    .iter()
                    .any(|v| v == "low" || v == "model_reasoning_effort=\"low\"")
            );
            assert!(
                emitted
                    .iter()
                    .any(|v| v == "--strict-mcp-config" || v == "--ignore-user-config")
            );
        }
    }

    #[test]
    fn client_hooks_only_add_notices_and_quote_paths() {
        let iface = Path::new("/tmp/it's an iface");
        let root = Path::new("/tmp/workers");
        let hooks = claude_hooks(iface, root).unwrap();
        assert_eq!(hooks["hooks"].as_object().unwrap().len(), 3);
        assert!(hooks["hooks"].get("Stop").is_none());
        assert!(hooks.to_string().contains("notice"));
        let config = codex_config(iface, root).unwrap();
        assert!(config.iter().any(|s| s.starts_with("hooks.state=")));
        assert!(!config.iter().any(|s| s.contains("bypass")));
        assert_eq!(quote("a'b").unwrap(), "'a'\\''b'");
    }
}
