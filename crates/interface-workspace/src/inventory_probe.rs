//! Explicit MCP health check, isolated from provider sessions and tool calls.
use interface_common::process::terminate_process_group;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn is_credential_env_var(name: &str) -> bool {
    const PREFIXES: [&str; 15] = [
        "INTERFACE_",
        "IFACE_",
        "ANTHROPIC_",
        "OPENAI_",
        "XAI_",
        "GEMINI_",
        "GOOGLE_API",
        "CLAUDE",
        "CODEX_",
        "GROK_",
        "QWEN_",
        "OPENCODE",
        "AWS_SECRET",
        "GH_",
        "GITHUB_",
    ];
    const SUFFIXES: [&str; 6] = [
        "_API_KEY",
        "_SECRET",
        "_SECRET_KEY",
        "_PASSWORD",
        "_PASSWD",
        "_TOKEN",
    ];
    let upper = name.to_ascii_uppercase();
    PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
        || SUFFIXES.iter().any(|suffix| upper.ends_with(suffix))
}

fn is_dangerous_probe_env_key(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("DYLD_")
        || upper.starts_with("LD_")
        || upper.starts_with("PYTHON")
        || matches!(
            upper.as_str(),
            "NODE_OPTIONS" | "BASH_ENV" | "ENV" | "ZDOTDIR" | "PATH"
        )
}

fn is_relative_executable(program: &str) -> bool {
    program == "."
        || program == ".."
        || program.starts_with("./")
        || program.starts_with("../")
        || (program.contains(['/', '\\']) && !Path::new(program).is_absolute())
}

fn probe_command(server: &Value, cwd: &Path) -> Result<Command, &'static str> {
    let mut argv = match server.get("command") {
        Some(Value::String(command)) => vec![command.clone()],
        Some(Value::Array(argv)) => argv
            .iter()
            .map(|arg| arg.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid server command.")?,
        _ => return Err("No stdio command is configured."),
    };
    if let Some(args) = server.get("args") {
        argv.extend(
            args.as_array()
                .ok_or("Invalid server arguments.")?
                .iter()
                .map(|arg| arg.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or("Invalid server arguments.")?,
        );
    }
    if argv.is_empty()
        || argv.len() > 128
        || argv
            .iter()
            .any(|arg| arg.is_empty() || arg.len() > 4096 || arg.contains('\0'))
    {
        return Err("Server command exceeds the health check's bounds.");
    }
    if is_relative_executable(&argv[0]) {
        return Err("Server command cannot use a relative path.");
    }
    let mut command = Command::new(&argv[0]);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(is_credential_env_var) {
            command.env_remove(name);
        }
    }
    command.env_remove("INTERFACE_PANE_MCP_SOCKET");
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    if let Some(env) = server.get("env").or_else(|| server.get("environment")) {
        let env = env.as_object().ok_or("Invalid server environment.")?;
        if env.len() > 128 {
            return Err("Server environment exceeds the health check's bounds.");
        }
        for (key, value) in env {
            let value = value.as_str().ok_or("Invalid server environment.")?;
            if key.is_empty()
                || key.len() > 256
                || key.contains(['=', '\0'])
                || is_dangerous_probe_env_key(key)
                || value.len() > 16 * 1024
                || value.contains('\0')
            {
                return Err("Invalid server environment.");
            }
            command.env(key, value);
        }
    }
    Ok(command)
}

pub(super) fn stdio_probe(server: &Value, cwd: &Path) -> Result<(), &'static str> {
    let mut command = probe_command(server, cwd)?;
    let mut child = command
        .spawn()
        .map_err(|_| "The configured server could not be started.")?;
    let result = (|| {
        let mut input = child
            .stdin
            .take()
            .ok_or("The server's input is unavailable.")?;
        let mut output = child
            .stdout
            .take()
            .ok_or("The server's output is unavailable.")?;
        rustix::fs::fcntl_setfl(&output, rustix::fs::OFlags::NONBLOCK)
            .map_err(|_| "Could not bound the server probe.")?;
        // These fixed requests contain no project content, credentials or tool calls.
        writeln!(input, "{}", json!({"jsonrpc":"2.0", "id":1,"method":"initialize","params":{
            "protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"Interface health check","version":"1"}}}))
            .map_err(|_| "The server closed before initialization.")?;
        let started = Instant::now();
        let mut pending = Vec::new();
        let mut total = 0_usize;
        let mut initialized = false;
        let mut buffer = [0_u8; 8192];
        while started.elapsed() < Duration::from_secs(10) {
            match output.read(&mut buffer) {
                Ok(0) => return Err("The server closed before completing the health check."),
                Ok(count) => {
                    total += count;
                    if total > 1024 * 1024 {
                        return Err("Server output exceeded the health check's limit.");
                    }
                    pending.extend_from_slice(&buffer[..count]);
                    while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                        let line: Vec<_> = pending.drain(..=end).collect();
                        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
                            continue;
                        };
                        if message.get("id") == Some(&json!(if initialized { 2 } else { 1 })) {
                            if message.get("error").is_some() {
                                return Err("The server refused initialize or ping.");
                            }
                            if message.get("result").and_then(Value::as_object).is_none() {
                                return Err("The server returned an invalid MCP response.");
                            }
                            if initialized {
                                return Ok(());
                            }
                            if message
                                .pointer("/result/protocolVersion")
                                .and_then(Value::as_str)
                                != Some("2024-11-05")
                                || message
                                    .pointer("/result/capabilities")
                                    .and_then(Value::as_object)
                                    .is_none()
                            {
                                return Err(
                                    "The server returned an invalid initialization response.",
                                );
                            }
                            initialized = true;
                            writeln!(
                                input,
                                "{}\n{}",
                                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                                json!({"jsonrpc":"2.0","id":2,"method":"ping"})
                            )
                            .map_err(|_| "The server closed before ping.")?;
                        }
                    }
                    if pending.len() > 256 * 1024 {
                        return Err("Server response exceeded the health check's limit.");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return Err("Could not read the server's health response."),
            }
        }
        Err("The server did not answer initialize and ping within 10 seconds.")
    })();
    terminate_process_group(child);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_command_rejects_relative_executables_and_dangerous_env_keys() {
        let cwd = Path::new("/tmp");
        for relative in ["./evil", "../evil", "bin/evil", ".", ".."] {
            let err = probe_command(&json!({"command": relative}), cwd).unwrap_err();
            assert!(err.contains("relative"), "{relative}: {err}");
        }

        for key in [
            "DYLD_INSERT_LIBRARIES",
            "dyld_library_path",
            "LD_PRELOAD",
            "NODE_OPTIONS",
            "PYTHONPATH",
            "PYTHONHOME",
            "BASH_ENV",
            "ENV",
            "ZDOTDIR",
            "PATH",
        ] {
            let err = probe_command(
                &json!({"command": "/bin/echo", "env": {key: "injected"}}),
                cwd,
            )
            .unwrap_err();
            assert_eq!(err, "Invalid server environment.", "key {key}");
        }

        let built = probe_command(
            &json!({"command": "/bin/echo", "env": {"SAFE_CUSTOM_FLAG": "1"}}),
            cwd,
        )
        .expect("safe absolute command");
        let envs = built
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            envs.iter()
                .any(|(k, v)| k == "INTERFACE_PANE_MCP_SOCKET" && v.is_none()),
            "INTERFACE_PANE_MCP_SOCKET must be explicitly removed"
        );
    }
}
