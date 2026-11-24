//! Built-in Qwen Code lifecycle for Interface's owner-managed MLX backend.

use std::{
    ffi::{OsStr, OsString},
    fs::OpenOptions,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_engine::{ExecutableResolution, resolve_executable};
use serde_json::{Value, json};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    process::{Child, Command},
    sync::Mutex as AsyncMutex,
    time::{sleep, timeout},
};

pub const QWEN_MODEL_ID: &str = "mlx-community/Qwen3.8-27B-4bit";
pub const QWEN_LOCAL_BASE_URL: &str = "http://127.0.0.1:8080/v1";
pub const QWEN_CONTEXT_WINDOW: u64 = 65_536;

const QWEN_MODELS_ADDRESS: (&str, u16) = ("127.0.0.1", 8080);
const QWEN_MODELS_PATH: &str = "/v1/models";
const DEFAULT_START_ATTEMPTS: usize = 120;
const DEFAULT_START_INTERVAL: Duration = Duration::from_millis(250);
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HEALTH_RESPONSE_BYTES: u64 = 1024 * 1024;

static NEXT_SETTINGS_ID: AtomicU64 = AtomicU64::new(1);

pub const QWEN_DISABLED_TOOLS: &[&str] = &[
    "zoom_image",
    "todo_write",
    "save_memory",
    "agent",
    "skill",
    "exit_plan_mode",
    "enter_plan_mode",
    "web_fetch",
    "web_search",
    "image_gen",
    "lsp",
    "ask_user_question",
    "cron_create",
    "cron_list",
    "cron_delete",
    "loop_wakeup",
    "create_sub_session",
    "list_agents",
    "task_stop",
    "task_create",
    "task_update",
    "task_list",
    "team_create",
    "team_delete",
    "team_plan_approval",
    "send_message",
    "structured_output",
    "monitor",
    "notebook_edit",
    "read_mcp_resource",
    "enter_worktree",
    "exit_worktree",
    "workflow",
    "artifact",
    "record_artifact",
    "get_goal",
    "update_goal",
    "display_image",
];

/// Exact Qwen Code system settings used by Interface 0.1.169.
#[must_use]
pub fn qwen_system_settings() -> Value {
    json!({
        "$version": 2,
        "model": {
            "name": QWEN_MODEL_ID,
            "baseUrl": QWEN_LOCAL_BASE_URL,
            "skipStartupContext": true
        },
        "modelProviders": {"openai": [{
            "id": QWEN_MODEL_ID,
            "name": "Qwen 3.8 27B (Interface local)",
            "description": "Interface-owned local MLX provider",
            "baseUrl": QWEN_LOCAL_BASE_URL,
            "envKey": "OPENAI_API_KEY",
            "generationConfig": {
                "timeout": 600_000,
                "maxRetries": 1,
                "contextWindowSize": QWEN_CONTEXT_WINDOW,
                "samplingParams": {
                    "temperature": 0.7,
                    "top_p": 0.95,
                    "max_tokens": 8192
                }
            }
        }]},
        "tools": {
            "disabled": QWEN_DISABLED_TOOLS,
            "toolSearch": {"enabled": true, "threshold": 10},
            "computerUse": {"enabled": false},
            "truncateToolOutputThreshold": 12_000,
            "truncateToolOutputLines": 300,
            "toolOutputBatchBudget": 20_000
        },
        "skills": {
            "disabledLevels": ["project", "user", "extension", "bundled"]
        },
        "memory": {
            "enableManagedAutoMemory": false,
            "enableManagedAutoDream": false,
            "enableAutoSkill": false,
            "enableTeamMemory": false,
            "enableTeamMemorySync": false
        },
        "context": {
            "autoCompactThreshold": 0.75,
            "clearContextOnIdle": {
                "toolResultsThresholdMinutes": 15,
                "toolResultsNumToKeep": 3,
                "toolResultsTotalCharsThreshold": 60_000
            }
        }
    })
}

#[derive(Clone, Debug)]
pub struct QwenBackendRuntimeOptions {
    pub settings_path: Option<PathBuf>,
    pub server_executable: Option<PathBuf>,
    /// Cached login-shell path to `qwen-mlx-server`. Never discovered here.
    pub login_shell_path: Option<PathBuf>,
    pub start_attempts: usize,
    pub start_interval: Duration,
    pub health_timeout: Duration,
    pub stop_timeout: Duration,
}

impl Default for QwenBackendRuntimeOptions {
    fn default() -> Self {
        Self {
            settings_path: None,
            server_executable: None,
            login_shell_path: None,
            start_attempts: DEFAULT_START_ATTEMPTS,
            start_interval: DEFAULT_START_INTERVAL,
            health_timeout: DEFAULT_HEALTH_TIMEOUT,
            stop_timeout: DEFAULT_STOP_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum QwenBackendError {
    #[error("Qwen backend runtime has been disposed.")]
    Disposed,
    #[error("Qwen backend startup was cancelled.")]
    Cancelled,
    #[error(
        "Qwen's local backend is offline and `qwen-mlx-server` was not found. Install the Interface Qwen MLX wrapper or set QWEN_MLX_SERVER_EXECUTABLE."
    )]
    MissingExecutable,
    #[error("Qwen cannot start because port 8080 belongs to a different service.")]
    PortOccupied,
    #[error("Qwen cannot start because port 8080 serves a different model.")]
    WrongModel,
    #[error("qwen-mlx-server exited before it became ready (code {0}).")]
    ChildExited(String),
    #[error("qwen-mlx-server did not advertise Qwen 3.8 27B within 30 seconds.")]
    StartupTimeout,
    #[error("Qwen's private system settings could not be created.")]
    SettingsUnavailable,
    #[error("Qwen's private system settings could not be removed.")]
    SettingsCleanup,
    #[error("qwen-mlx-server could not be started.")]
    SpawnUnavailable,
    #[error("qwen-mlx-server could not be stopped cleanly.")]
    ShutdownFailed,
}

impl QwenBackendError {
    #[must_use]
    pub(crate) const fn retryable(&self) -> bool {
        matches!(
            self,
            Self::MissingExecutable
                | Self::SettingsUnavailable
                | Self::SettingsCleanup
                | Self::SpawnUnavailable
                | Self::ShutdownFailed
        )
    }
}

#[async_trait]
pub trait QwenBackendLifecycle: Send + Sync {
    fn settings_path(&self) -> &Path;
    async fn prepare_probe(&self) -> Result<(), QwenBackendError>;
    async fn prepare(&self) -> Result<(), QwenBackendError>;
    async fn dispose(&self) -> Result<(), QwenBackendError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum QwenBackendHealth {
    Ready,
    Offline,
    Occupied,
}

#[async_trait]
trait QwenHealthProbe: Send + Sync {
    async fn probe(&self) -> QwenBackendHealth;
}

struct SystemQwenHealthProbe {
    timeout: Duration,
}

#[async_trait]
impl QwenHealthProbe for SystemQwenHealthProbe {
    async fn probe(&self) -> QwenBackendHealth {
        probe_qwen_backend(self.timeout).await
    }
}

#[derive(Default)]
struct QwenBackendState {
    settings_written: bool,
    owned_child: Option<Child>,
    cleaned: bool,
}

pub struct QwenBackendRuntime {
    options: QwenBackendRuntimeOptions,
    settings_path: PathBuf,
    probe: Arc<dyn QwenHealthProbe>,
    operation: AsyncMutex<()>,
    state: Mutex<QwenBackendState>,
    disposed: AtomicBool,
    prepare_generation: AtomicU64,
    last_prepare: Mutex<Option<Result<(), QwenBackendError>>>,
}

impl QwenBackendRuntime {
    #[must_use]
    pub fn new(options: QwenBackendRuntimeOptions) -> Self {
        let settings_path = options
            .settings_path
            .clone()
            .unwrap_or_else(unique_settings_path);
        let probe: Arc<dyn QwenHealthProbe> = Arc::new(SystemQwenHealthProbe {
            timeout: options.health_timeout,
        });
        Self::with_probe(options, settings_path, probe)
    }

    fn with_probe(
        options: QwenBackendRuntimeOptions,
        settings_path: PathBuf,
        probe: Arc<dyn QwenHealthProbe>,
    ) -> Self {
        Self {
            options,
            settings_path,
            probe,
            operation: AsyncMutex::new(()),
            state: Mutex::new(QwenBackendState::default()),
            disposed: AtomicBool::new(false),
            prepare_generation: AtomicU64::new(0),
            last_prepare: Mutex::new(None),
        }
    }

    async fn ensure_settings(&self) -> Result<(), QwenBackendError> {
        if lock(&self.state).settings_written {
            return Ok(());
        }
        let mut settings = serde_json::to_string_pretty(&qwen_system_settings())
            .map_err(|_| QwenBackendError::SettingsUnavailable)?;
        settings.push('\n');
        write_private_settings(self.settings_path.clone(), settings).await?;
        lock(&self.state).settings_written = true;
        Ok(())
    }

    async fn start_once(&self) -> Result<(), QwenBackendError> {
        self.ensure_settings().await?;
        match self.probe.probe().await {
            QwenBackendHealth::Ready => return Ok(()),
            QwenBackendHealth::Occupied => return Err(QwenBackendError::PortOccupied),
            QwenBackendHealth::Offline => {}
        }
        let executable = if let Some(executable) = &self.options.server_executable {
            executable.clone()
        } else {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            resolve_qwen_mlx_server_executable(
                std::env::var_os("QWEN_MLX_SERVER_EXECUTABLE")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from),
                self.options.login_shell_path.clone(),
                home.as_deref(),
                Path::exists,
            )
            .ok_or(QwenBackendError::MissingExecutable)?
        };
        let mut child = spawn_qwen_backend(&executable)?;
        for _ in 0..self.options.start_attempts {
            if self.disposed.load(Ordering::Acquire) {
                let _ = stop_owned_child(&mut child, self.options.stop_timeout).await;
                return Err(QwenBackendError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(QwenBackendError::ChildExited(
                        status
                            .code()
                            .map_or_else(|| "null".into(), |code| code.to_string()),
                    ));
                }
                Ok(None) => {}
                Err(_) => {
                    let _ = stop_owned_child(&mut child, self.options.stop_timeout).await;
                    return Err(QwenBackendError::SpawnUnavailable);
                }
            }
            sleep(self.options.start_interval).await;
            match self.probe.probe().await {
                QwenBackendHealth::Ready => {
                    let previous = lock(&self.state).owned_child.replace(child);
                    if let Some(mut previous) = previous {
                        let _ = stop_owned_child(&mut previous, self.options.stop_timeout).await;
                    }
                    return Ok(());
                }
                QwenBackendHealth::Occupied => {
                    let _ = stop_owned_child(&mut child, self.options.stop_timeout).await;
                    return Err(QwenBackendError::WrongModel);
                }
                QwenBackendHealth::Offline => {}
            }
        }
        let _ = stop_owned_child(&mut child, self.options.stop_timeout).await;
        Err(QwenBackendError::StartupTimeout)
    }
}

#[async_trait]
impl QwenBackendLifecycle for QwenBackendRuntime {
    fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    async fn prepare_probe(&self) -> Result<(), QwenBackendError> {
        let _operation = self.operation.lock().await;
        if self.disposed.load(Ordering::Acquire) {
            return Err(QwenBackendError::Disposed);
        }
        self.ensure_settings().await
    }

    async fn prepare(&self) -> Result<(), QwenBackendError> {
        let observed_generation = self.prepare_generation.load(Ordering::Acquire);
        let _operation = self.operation.lock().await;
        if self.disposed.load(Ordering::Acquire) {
            return Err(QwenBackendError::Disposed);
        }
        if self.prepare_generation.load(Ordering::Acquire) != observed_generation {
            return lock(&self.last_prepare)
                .clone()
                .unwrap_or(Err(QwenBackendError::SpawnUnavailable));
        }
        let result = self.start_once().await;
        *lock(&self.last_prepare) = Some(result.clone());
        self.prepare_generation.fetch_add(1, Ordering::AcqRel);
        result
    }

    async fn dispose(&self) -> Result<(), QwenBackendError> {
        self.disposed.store(true, Ordering::Release);
        let _operation = self.operation.lock().await;
        if lock(&self.state).cleaned {
            return Ok(());
        }
        let mut child = lock(&self.state).owned_child.take();
        let settings_written = lock(&self.state).settings_written;
        let stop_result = if let Some(child) = child.as_mut() {
            stop_owned_child(child, self.options.stop_timeout).await
        } else {
            Ok(())
        };
        let remove_result = if settings_written {
            remove_private_settings(&self.settings_path).await
        } else {
            Ok(())
        };
        if remove_result.is_ok() {
            let mut state = lock(&self.state);
            state.settings_written = false;
            state.cleaned = true;
        }
        stop_result.and(remove_result)
    }
}

#[must_use]
pub(crate) fn qwen_process_environment(settings_path: &Path) -> Vec<(OsString, OsString)> {
    vec![
        (OsString::from("OPENAI_API_KEY"), OsString::from("local")),
        (
            OsString::from("OPENAI_BASE_URL"),
            OsString::from(QWEN_LOCAL_BASE_URL),
        ),
        (
            OsString::from("OPENAI_MODEL"),
            OsString::from(QWEN_MODEL_ID),
        ),
        (
            OsString::from("QWEN_CODE_SYSTEM_SETTINGS_PATH"),
            settings_path.as_os_str().to_owned(),
        ),
    ]
}

async fn write_private_settings(path: PathBuf, contents: String) -> Result<(), QwenBackendError> {
    tokio::task::spawn_blocking(move || {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options
            .open(&path)
            .map_err(|_| QwenBackendError::SettingsUnavailable)?;
        let result = (|| {
            file.write_all(contents.as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(|_| QwenBackendError::SettingsUnavailable)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|_| QwenBackendError::SettingsUnavailable)?;
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|_| QwenBackendError::SettingsUnavailable)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o777 != 0o600
            {
                return Err(QwenBackendError::SettingsUnavailable);
            }
            Ok(())
        })();
        drop(file);
        if result.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        result
    })
    .await
    .map_err(|_| QwenBackendError::SettingsUnavailable)?
}

async fn remove_private_settings(path: &Path) -> Result<(), QwenBackendError> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(QwenBackendError::SettingsCleanup),
    }
}

fn spawn_qwen_backend(executable: &Path) -> Result<Child, QwenBackendError> {
    let mut command = Command::new(executable);
    command
        .env("OPENAI_API_KEY", "local")
        .env("OPENAI_BASE_URL", QWEN_LOCAL_BASE_URL)
        .env("OPENAI_MODEL", QWEN_MODEL_ID)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    command
        .spawn()
        .map_err(|_| QwenBackendError::SpawnUnavailable)
}

async fn stop_owned_child(
    child: &mut Child,
    stop_timeout: Duration,
) -> Result<(), QwenBackendError> {
    match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(_) => return Err(QwenBackendError::ShutdownFailed),
    }
    let Some(pid) = child.id() else {
        return Ok(());
    };
    let term_status = Command::new("/bin/kill")
        .args([OsStr::new("-TERM"), OsStr::new(&pid.to_string())])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await;
    match term_status {
        Ok(status) if status.success() => {}
        Ok(_) => {
            if child.try_wait().ok().flatten().is_some() {
                return Ok(());
            }
            return Err(QwenBackendError::ShutdownFailed);
        }
        Err(_) => return Err(QwenBackendError::ShutdownFailed),
    }
    match timeout(stop_timeout, child.wait()).await {
        Ok(Ok(_)) => return Ok(()),
        Ok(Err(_)) => return Err(QwenBackendError::ShutdownFailed),
        Err(_) => {}
    }
    if child.start_kill().is_err() {
        return match child.try_wait() {
            Ok(Some(_)) => Ok(()),
            Ok(None) | Err(_) => Err(QwenBackendError::ShutdownFailed),
        };
    }
    match timeout(stop_timeout, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(QwenBackendError::ShutdownFailed),
    }
}

fn resolve_qwen_mlx_server_executable(
    explicit_override: Option<PathBuf>,
    login_shell_path: Option<PathBuf>,
    home: Option<&Path>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let known_locations = home
        .map(|home| home.join(".local/bin/qwen-mlx-server"))
        .into_iter()
        .chain([
            PathBuf::from("/opt/homebrew/bin/qwen-mlx-server"),
            PathBuf::from("/usr/local/bin/qwen-mlx-server"),
        ])
        .collect();
    resolve_executable(
        &ExecutableResolution {
            explicit_override,
            login_shell_path,
            known_locations,
            require_absolute: true,
        },
        exists,
    )
    .map(|resolved| resolved.path)
}

async fn probe_qwen_backend(health_timeout: Duration) -> QwenBackendHealth {
    let response = timeout(health_timeout, async {
        let mut stream = TcpStream::connect(QWEN_MODELS_ADDRESS).await?;
        stream
            .write_all(
                format!(
                    "GET {QWEN_MODELS_PATH} HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        let mut bytes = Vec::new();
        stream
            .take(MAX_HEALTH_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .await?;
        Ok::<Vec<u8>, io::Error>(bytes)
    })
    .await;
    let Ok(Ok(response)) = response else {
        return QwenBackendHealth::Offline;
    };
    if response.len() as u64 > MAX_HEALTH_RESPONSE_BYTES {
        return QwenBackendHealth::Offline;
    }
    classify_qwen_health_response(&response)
}

fn classify_qwen_health_response(response: &[u8]) -> QwenBackendHealth {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        return QwenBackendHealth::Offline;
    };
    let Ok(headers) = std::str::from_utf8(&response[..header_end]) else {
        return QwenBackendHealth::Offline;
    };
    let Some(status) = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
    else {
        return QwenBackendHealth::Offline;
    };
    if !(200..300).contains(&status) {
        return QwenBackendHealth::Occupied;
    }
    let Ok(body) = serde_json::from_slice::<Value>(&response[header_end + 4..]) else {
        return QwenBackendHealth::Offline;
    };
    let Some(models) = body.get("data").and_then(Value::as_array) else {
        return QwenBackendHealth::Occupied;
    };
    if models
        .iter()
        .any(|model| model.get("id").and_then(Value::as_str) == Some(QWEN_MODEL_ID))
    {
        QwenBackendHealth::Ready
    } else {
        QwenBackendHealth::Occupied
    }
}

fn unique_settings_path() -> PathBuf {
    let ordinal = NEXT_SETTINGS_ID.fetch_add(1, Ordering::Relaxed);
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "iface-qwen-{}-{epoch:x}-{ordinal:x}.json",
        std::process::id()
    ))
}

#[cfg(test)]
#[path = "qwen_tests.rs"]
mod tests;
