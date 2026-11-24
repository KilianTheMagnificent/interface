use std::{
    collections::VecDeque,
    fs,
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};

use tempfile::tempdir;

use super::*;

struct SequenceProbe {
    calls: AtomicUsize,
    values: Mutex<VecDeque<QwenBackendHealth>>,
    fallback: QwenBackendHealth,
}

#[async_trait]
impl QwenHealthProbe for SequenceProbe {
    async fn probe(&self) -> QwenBackendHealth {
        self.calls.fetch_add(1, Ordering::AcqRel);
        lock(&self.values).pop_front().unwrap_or(self.fallback)
    }
}

fn runtime_with_probe(
    options: QwenBackendRuntimeOptions,
    probe: Arc<SequenceProbe>,
) -> QwenBackendRuntime {
    let settings_path = options.settings_path.clone().expect("settings path");
    QwenBackendRuntime::with_probe(options, settings_path, probe)
}

#[test]
fn frozen_settings_keep_the_local_model_and_cache_stable_tool_policy() {
    let settings = qwen_system_settings();
    assert_eq!(settings["$version"], 2);
    assert_eq!(settings["model"]["name"], QWEN_MODEL_ID);
    assert_eq!(settings["model"]["baseUrl"], QWEN_LOCAL_BASE_URL);
    assert_eq!(settings["model"]["skipStartupContext"], true);
    assert_eq!(
        settings["modelProviders"]["openai"][0]["generationConfig"]["contextWindowSize"],
        QWEN_CONTEXT_WINDOW
    );
    assert_eq!(
        settings["modelProviders"]["openai"][0]["generationConfig"]["samplingParams"]["max_tokens"],
        8192
    );
    assert_eq!(
        settings["tools"]["toolSearch"],
        json!({"enabled": true, "threshold": 10})
    );
    assert_eq!(settings["memory"]["enableManagedAutoMemory"], false);
}

#[tokio::test]
async fn probe_preparation_writes_one_exclusive_0600_profile_without_health_or_spawn() {
    let directory = tempdir().expect("temporary directory");
    let settings_path = directory.path().join("qwen-settings.json");
    let probe = Arc::new(SequenceProbe {
        calls: AtomicUsize::new(0),
        values: Mutex::new(VecDeque::new()),
        fallback: QwenBackendHealth::Offline,
    });
    let runtime = runtime_with_probe(
        QwenBackendRuntimeOptions {
            settings_path: Some(settings_path.clone()),
            server_executable: Some(directory.path().join("must-not-spawn")),
            ..QwenBackendRuntimeOptions::default()
        },
        Arc::clone(&probe),
    );

    let (first, second) = tokio::join!(runtime.prepare_probe(), runtime.prepare_probe());
    first.expect("first probe preparation");
    second.expect("second probe preparation");
    assert_eq!(probe.calls.load(Ordering::Acquire), 0);
    assert_eq!(
        fs::metadata(&settings_path)
            .expect("settings metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let decoded: Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).expect("settings content"))
            .expect("settings JSON");
    assert_eq!(decoded, qwen_system_settings());
    runtime.dispose().await.expect("dispose");
    assert!(!settings_path.exists());
}

#[tokio::test]
async fn matching_external_backend_is_reused_and_concurrent_prepare_is_deduplicated() {
    let directory = tempdir().expect("temporary directory");
    let probe = Arc::new(SequenceProbe {
        calls: AtomicUsize::new(0),
        values: Mutex::new(VecDeque::from([QwenBackendHealth::Ready])),
        fallback: QwenBackendHealth::Ready,
    });
    let runtime = Arc::new(runtime_with_probe(
        QwenBackendRuntimeOptions {
            settings_path: Some(directory.path().join("settings.json")),
            server_executable: Some(directory.path().join("missing-server")),
            ..QwenBackendRuntimeOptions::default()
        },
        Arc::clone(&probe),
    ));

    let (first, second) = tokio::join!(runtime.prepare(), runtime.prepare());
    first.expect("first prepare");
    second.expect("deduplicated prepare");
    assert_eq!(probe.calls.load(Ordering::Acquire), 1);
    runtime.dispose().await.expect("dispose");
}

#[tokio::test]
async fn preexisting_settings_path_is_refused_and_never_removed_as_owned_cleanup() {
    let directory = tempdir().expect("temporary directory");
    let settings_path = directory.path().join("settings.json");
    fs::write(&settings_path, "preexisting-user-bytes").expect("existing settings");
    let probe = Arc::new(SequenceProbe {
        calls: AtomicUsize::new(0),
        values: Mutex::new(VecDeque::new()),
        fallback: QwenBackendHealth::Offline,
    });
    let runtime = runtime_with_probe(
        QwenBackendRuntimeOptions {
            settings_path: Some(settings_path.clone()),
            ..QwenBackendRuntimeOptions::default()
        },
        probe,
    );

    assert_eq!(
        runtime.prepare_probe().await,
        Err(QwenBackendError::SettingsUnavailable)
    );
    runtime.dispose().await.expect("dispose without ownership");
    assert_eq!(
        fs::read_to_string(settings_path).expect("preserved settings"),
        "preexisting-user-bytes"
    );
}

#[tokio::test]
async fn owned_backend_receives_sigterm_and_is_cleaned_up() {
    let directory = tempdir().expect("temporary directory");
    let marker = directory.path().join("signals.txt");
    let started = directory.path().join("started");
    let executable = directory.path().join("qwen-mlx-server");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\ntrap 'echo TERM >> \"{}\"; exit 0' TERM\ntouch \"{}\"\nwhile :; do :; done\n",
            marker.display(),
            started.display(),
        ),
    )
    .expect("write server");
    let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("executable mode");
    let probe = Arc::new(SequenceProbe {
        calls: AtomicUsize::new(0),
        values: Mutex::new(VecDeque::from([
            QwenBackendHealth::Offline,
            QwenBackendHealth::Ready,
        ])),
        fallback: QwenBackendHealth::Ready,
    });
    let runtime = runtime_with_probe(
        QwenBackendRuntimeOptions {
            settings_path: Some(directory.path().join("settings.json")),
            server_executable: Some(executable),
            start_interval: Duration::from_millis(1),
            stop_timeout: Duration::from_secs(2),
            ..QwenBackendRuntimeOptions::default()
        },
        probe,
    );

    runtime.prepare().await.expect("prepare");
    timeout(Duration::from_secs(2), async {
        while !started.exists() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server start marker");
    runtime.dispose().await.expect("dispose");
    assert_eq!(
        fs::read_to_string(marker).expect("signal marker").trim(),
        "TERM"
    );
}

#[tokio::test]
async fn occupied_port_is_refused_without_spawning_the_configured_server() {
    let directory = tempdir().expect("temporary directory");
    let marker = directory.path().join("spawned");
    let executable = directory.path().join("qwen-mlx-server");
    fs::write(
        &executable,
        format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
    )
    .expect("write server");
    let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("executable mode");
    let probe = Arc::new(SequenceProbe {
        calls: AtomicUsize::new(0),
        values: Mutex::new(VecDeque::from([QwenBackendHealth::Occupied])),
        fallback: QwenBackendHealth::Occupied,
    });
    let runtime = runtime_with_probe(
        QwenBackendRuntimeOptions {
            settings_path: Some(directory.path().join("settings.json")),
            server_executable: Some(executable),
            ..QwenBackendRuntimeOptions::default()
        },
        probe,
    );

    assert_eq!(runtime.prepare().await, Err(QwenBackendError::PortOccupied));
    assert!(!marker.exists());
    runtime.dispose().await.expect("dispose");
}

#[tokio::test]
async fn shutdown_escalates_to_sigkill_when_the_owned_child_ignores_sigterm() {
    let directory = tempdir().expect("temporary directory");
    let marker = directory.path().join("term-seen");
    let started = directory.path().join("started");
    let executable = directory.path().join("qwen-mlx-server");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\ntrap 'echo TERM >> \"{}\"' TERM\ntouch \"{}\"\nwhile :; do :; done\n",
            marker.display(),
            started.display(),
        ),
    )
    .expect("write server");
    let mut permissions = fs::metadata(&executable).expect("metadata").permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).expect("executable mode");
    let probe = Arc::new(SequenceProbe {
        calls: AtomicUsize::new(0),
        values: Mutex::new(VecDeque::from([
            QwenBackendHealth::Offline,
            QwenBackendHealth::Ready,
        ])),
        fallback: QwenBackendHealth::Ready,
    });
    let runtime = runtime_with_probe(
        QwenBackendRuntimeOptions {
            settings_path: Some(directory.path().join("settings.json")),
            server_executable: Some(executable),
            start_interval: Duration::from_millis(1),
            stop_timeout: Duration::from_millis(100),
            ..QwenBackendRuntimeOptions::default()
        },
        probe,
    );

    runtime.prepare().await.expect("prepare");
    timeout(Duration::from_secs(2), async {
        while !started.exists() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("server start marker");
    runtime.dispose().await.expect("SIGKILL escalation");
    assert_eq!(
        fs::read_to_string(marker).expect("TERM marker").trim(),
        "TERM"
    );
}

#[test]
fn qwen_process_environment_binds_only_the_local_profile_and_settings_path() {
    assert_eq!(
        qwen_process_environment(Path::new("/tmp/qwen-settings.json")),
        vec![
            ("OPENAI_API_KEY".into(), "local".into()),
            ("OPENAI_BASE_URL".into(), QWEN_LOCAL_BASE_URL.into()),
            ("OPENAI_MODEL".into(), QWEN_MODEL_ID.into()),
            (
                "QWEN_CODE_SYSTEM_SETTINGS_PATH".into(),
                "/tmp/qwen-settings.json".into()
            )
        ]
    );
}

#[test]
fn loopback_8080_probe_requires_the_exact_advertised_model_identity() {
    assert_eq!(QWEN_MODELS_ADDRESS, ("127.0.0.1", 8080));
    assert_eq!(QWEN_MODELS_PATH, "/v1/models");
    let response = |status: u16, body: &str| {
        format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
    };
    assert_eq!(
        classify_qwen_health_response(
            response(200, &json!({"data": [{"id": QWEN_MODEL_ID}]}).to_string()).as_bytes()
        ),
        QwenBackendHealth::Ready
    );
    assert_eq!(
        classify_qwen_health_response(
            response(200, &json!({"data": [{"id": "another-model"}]}).to_string()).as_bytes()
        ),
        QwenBackendHealth::Occupied
    );
    assert_eq!(
        classify_qwen_health_response(response(503, "{}").as_bytes()),
        QwenBackendHealth::Occupied
    );
    assert_eq!(
        classify_qwen_health_response(b"not-http"),
        QwenBackendHealth::Offline
    );
}

#[test]
fn mlx_server_prefers_override_then_login_shell_without_spawning() {
    let override_path = PathBuf::from("/tmp/override/qwen-mlx-server");
    let login_path = PathBuf::from("/login/qwen-mlx-server");
    let home = Path::new("/Users/test");
    let exists = |path: &Path| {
        path == override_path.as_path()
            || path == login_path.as_path()
            || path == Path::new("/Users/test/.local/bin/qwen-mlx-server")
    };
    assert_eq!(
        resolve_qwen_mlx_server_executable(
            Some(override_path.clone()),
            Some(login_path.clone()),
            Some(home),
            exists,
        ),
        Some(override_path.clone())
    );
    assert_eq!(
        resolve_qwen_mlx_server_executable(None, Some(login_path.clone()), Some(home), exists),
        Some(login_path.clone())
    );
    assert_eq!(
        resolve_qwen_mlx_server_executable(None, None, Some(home), exists),
        Some(PathBuf::from("/Users/test/.local/bin/qwen-mlx-server"))
    );
    assert_eq!(
        resolve_qwen_mlx_server_executable(
            Some("relative/qwen-mlx-server".into()),
            None,
            None,
            |path| path == Path::new("relative/qwen-mlx-server"),
        ),
        None
    );
}
