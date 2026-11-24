//! Local CLI transport and launcher for the optional workers service.
use crate::errors::{CLI_EXIT, CommandResult};
use interface_collaboration::workers::{Policy, Service};
use interface_host::report_workers::OpenCodeRunner;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

mod launch;
#[cfg(test)]
mod tests;
mod transport;
const USAGE: &str = "iface workers serve --root <private-dir> --project <absolute-dir> --opencode <absolute-cli> [--allow-contributor-data] [--allow-file <relative-file>]\niface workers launch --root <private-dir> --client <claude|codex> --executable <absolute-cli> [--model <exact-model>] [--effort <level>] [--prompt <text>]\niface workers mcp --root <private-dir>\niface workers notice --root <private-dir> --client <claude|codex>\n\niface workers grant --root <host-data>/panes/report-workers --project <absolute-dir> --allow-contributor-data [--allow-file <relative-file>]\niface workers revoke --root <host-data>/panes/report-workers --project <absolute-dir>\n\nStart the broker separately for CLI launchers. Native Interface panes use their own shared broker. Sharing is denied unless the owner explicitly supplies --allow-contributor-data; only named files may be captured. Launch binds a new lead conversation. Existing terminal jobs and provider configuration are unchanged. No install or restart.\n";

pub async fn run(args: &[String]) -> CommandResult {
    if args.is_empty() || args == ["--help"] {
        return CommandResult::success(USAGE.into());
    }
    match run_inner(args).await {
        Ok(text) => CommandResult::success(text),
        Err(error) => CommandResult::error(error, CLI_EXIT.generic),
    }
}
async fn run_inner(args: &[String]) -> Result<String, String> {
    let options = Options::parse(args)?;
    match options.verb.as_str() {
        "grant" | "revoke" => {
            if options.verb == "grant" && !options.allow_data {
                return Err("grant requires explicit --allow-contributor-data".into());
            }
            interface_collaboration::workers::grants::write(
                &options.root,
                Policy {
                    source: options.path("--project")?,
                    files: options.files,
                    contributor_data_allowed: options.verb == "grant",
                },
            )?;
            Ok("Source-sharing grant updated for future task admissions. No model call was made.\n".into())
        }
        "serve" => {
            let source = options
                .path("--project")?
                .canonicalize()
                .map_err(|e| e.to_string())?;
            let runner = Arc::new(OpenCodeRunner {
                executable: options.path("--opencode")?,
            });
            let service = Service::open(
                &options.root,
                Policy {
                    source,
                    files: options.files,
                    contributor_data_allowed: options.allow_data,
                },
                runner,
            )?;
            transport::serve(&options.root, service).await?;
            Ok(String::new())
        }
        "mcp" => {
            transport::bridge(&options.root).await?;
            Ok(String::new())
        }
        "notice" => {
            options.client()?;
            let value = transport::request(&options.root, json!({"method":"notice"})).await?;
            let message = value["message"].as_str().unwrap_or_default();
            if message.is_empty() {
                return Ok("{}\n".into());
            }
            let event = options
                .values
                .get("--event")
                .map_or("PostToolUse", String::as_str);
            if !matches!(event, "PostToolUse" | "UserPromptSubmit" | "SessionStart") {
                return Err("invalid hook event".into());
            }
            Ok(format!(
                "{}\n",
                json!({"hookSpecificOutput":{"hookEventName":event,"additionalContext":message}})
            ))
        }
        "launch" => launch::run(&options).await,
        _ => Err(USAGE.into()),
    }
}
struct Options {
    verb: String,
    root: PathBuf,
    values: BTreeMap<String, String>,
    files: BTreeSet<String>,
    allow_data: bool,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut result = Self {
            verb: args[0].clone(),
            root: PathBuf::new(),
            values: BTreeMap::new(),
            files: BTreeSet::new(),
            allow_data: false,
        };
        let mut index = 1;
        while index < args.len() {
            let key = &args[index];
            if key == "--allow-contributor-data" {
                result.allow_data = true;
                index += 1;
                continue;
            }
            if !matches!(
                key.as_str(),
                "--root"
                    | "--project"
                    | "--opencode"
                    | "--allow-file"
                    | "--client"
                    | "--executable"
                    | "--model"
                    | "--prompt"
                    | "--event"
                    | "--effort"
            ) {
                return Err(format!("unknown worker option: {key}"));
            }
            let value = args
                .get(index + 1)
                .filter(|s| !s.is_empty())
                .ok_or("worker option needs a value")?;
            if key == "--allow-file" {
                result.files.insert(value.clone());
            } else if result.values.insert(key.clone(), value.clone()).is_some() {
                return Err("duplicate worker option".into());
            }
            index += 2;
        }
        result.root = result.path("--root")?;
        if !matches!(result.verb.as_str(), "grant" | "revoke")
            && result.root.to_string_lossy().len() > 80
        {
            return Err(
                "worker root too long for a private Unix socket; use a short absolute path".into(),
            );
        }
        if !matches!(result.verb.as_str(), "serve" | "grant" | "revoke")
            && (result.allow_data
                || !result.files.is_empty()
                || result.values.contains_key("--project")
                || result.values.contains_key("--opencode"))
        {
            return Err("sharing and source scope are broker-startup options only".into());
        }
        Ok(result)
    }
    fn path(&self, key: &str) -> Result<PathBuf, String> {
        self.values
            .get(key)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .ok_or_else(|| format!("{key} must be an absolute path"))
    }
    fn client(&self) -> Result<&str, String> {
        self.values
            .get("--client")
            .map(String::as_str)
            .filter(|v| matches!(*v, "claude" | "codex"))
            .ok_or_else(|| "--client must be claude or codex".into())
    }
}
