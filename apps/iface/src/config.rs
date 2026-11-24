use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use interface_storage::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE};
use serde::Serialize;
use serde_json::Value;

use crate::argv::{LaunchScope, TuiMode, is_engine_id};

const QWEN_FIXED_MODEL: &str = "mlx-community/Qwen3.8-27B-4bit";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CliConfig {
    pub tui_mode: TuiMode,
    pub last_engine: Option<String>,
    pub last_model: Option<String>,
    pub last_effort: Option<String>,
    pub notifications: bool,
    pub collaboration: bool,
    pub scope: LaunchScope,
    pub access_modes: BTreeMap<String, String>,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            tui_mode: TuiMode::Inline,
            last_engine: None,
            last_model: None,
            last_effort: None,
            notifications: true,
            collaboration: true,
            scope: LaunchScope::Project,
            access_modes: BTreeMap::new(),
        }
    }
}

impl CliConfig {
    #[must_use]
    pub fn provider_preferences(&self, engine: &str) -> (Option<String>, Option<String>) {
        if engine == "acp:qwen" {
            return (Some(QWEN_FIXED_MODEL.into()), None);
        }
        if self.last_engine.as_deref() != Some(engine) {
            return (None, None);
        }
        (self.last_model.clone(), self.last_effort.clone())
    }

    #[must_use]
    pub fn access_mode_for_launch(&self, engine: &str) -> Option<String> {
        self.access_modes
            .get(engine)
            .filter(|mode| is_restorable_access_mode(engine, mode))
            .cloned()
            .or_else(|| default_access_mode(engine).map(str::to_owned))
    }
}

#[derive(Clone, Debug)]
pub struct CliConfigStore {
    path: PathBuf,
    config: CliConfig,
}

impl CliConfigStore {
    /// Creates a store for the exact legacy `config.json` path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is not absolute.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        if !path.is_absolute() {
            return Err("CLI config path must be absolute".into());
        }
        Ok(Self {
            path,
            config: CliConfig::default(),
        })
    }

    /// Loads a legacy configuration, quarantining malformed state just as
    /// Interface 0.1.169 did and falling back to its safe defaults.
    #[must_use]
    pub fn load(&mut self) -> CliConfig {
        match read_config(&self.path) {
            Ok(source) => {
                let parsed = serde_json::from_str::<Value>(&source)
                    .ok()
                    .and_then(|value| parse_config(&value));
                if let Some(parsed) = parsed {
                    self.config = parsed;
                } else {
                    self.quarantine_corrupt();
                    self.config = CliConfig::default();
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                self.quarantine_corrupt();
                self.config = CliConfig::default();
            }
        }
        self.config.clone()
    }

    #[must_use]
    pub fn get(&self) -> CliConfig {
        self.config.clone()
    }

    /// Whether desktop notifications are on, without cloning the config the
    /// draw loop would otherwise copy every frame.
    #[must_use]
    pub const fn notifications(&self) -> bool {
        self.config.notifications
    }

    pub fn set_tui_mode(&mut self, mode: TuiMode) {
        self.config.tui_mode = mode;
    }

    pub fn set_notifications(&mut self, enabled: bool) {
        self.config.notifications = enabled;
    }

    pub fn set_collaboration(&mut self, enabled: bool) {
        self.config.collaboration = enabled;
    }

    pub fn remember_engine(&mut self, engine: String) {
        self.config.last_engine = Some(engine);
        self.config.last_model = None;
        self.config.last_effort = None;
    }

    pub fn remember_model(&mut self, model: Option<String>) {
        self.config.last_model = model;
    }

    pub fn remember_effort(&mut self, effort: Option<String>) {
        self.config.last_effort = effort;
    }

    pub fn remember_access_mode(&mut self, engine: &str, mode: &str) {
        if is_restorable_access_mode(engine, mode) {
            self.config
                .access_modes
                .insert(engine.to_owned(), mode.to_owned());
        } else {
            self.config.access_modes.remove(engine);
        }
    }

    pub fn remember_session(
        &mut self,
        engine: String,
        model: Option<String>,
        effort: Option<String>,
    ) {
        self.config.last_engine = Some(engine);
        self.config.last_model = model;
        self.config.last_effort = effort;
    }

    /// Persists an owner-private legacy-compatible configuration atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory or temporary file cannot be made
    /// private, written, synchronized, or renamed into place.
    pub fn save(&self) -> Result<(), String> {
        let mut payload = serde_json::to_vec(&PersistedConfig::from(&self.config))
            .map_err(|error| error.to_string())?;
        payload.push(b'\n');
        crate::private_write::write_private_bytes(&self.path, &payload)
    }

    fn quarantine_corrupt(&self) {
        let corrupt = PathBuf::from(format!("{}.corrupt", self.path.display()));
        if fs::rename(&self.path, &corrupt).is_ok()
            && fs::symlink_metadata(&corrupt).is_ok_and(|metadata| metadata.is_file())
        {
            let _ = fs::set_permissions(corrupt, fs::Permissions::from_mode(OWNER_FILE_MODE));
        }
    }
}

fn read_config(path: &Path) -> std::io::Result<String> {
    if let Some(parent) = path.parent() {
        // Read paths only re-assert the mode: creating or refusing the parent here
        // would route a symlinked data directory into quarantine_corrupt.
        fs::set_permissions(parent, fs::Permissions::from_mode(OWNER_DIRECTORY_MODE))?;
    }
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "CLI config must be one regular file",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(OWNER_FILE_MODE))?;
    let mut source = String::new();
    file.read_to_string(&mut source)?;
    Ok(source)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedConfig<'a> {
    tui_mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_engine: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_effort: Option<&'a str>,
    notifications: bool,
    collaboration: bool,
    scope: &'static str,
    access_modes: &'a BTreeMap<String, String>,
}

impl<'a> From<&'a CliConfig> for PersistedConfig<'a> {
    fn from(config: &'a CliConfig) -> Self {
        Self {
            tui_mode: config.tui_mode.as_str(),
            last_engine: config.last_engine.as_deref(),
            last_model: config.last_model.as_deref(),
            last_effort: config.last_effort.as_deref(),
            notifications: config.notifications,
            collaboration: config.collaboration,
            scope: config.scope.as_str(),
            access_modes: &config.access_modes,
        }
    }
}

fn parse_config(value: &Value) -> Option<CliConfig> {
    let object = value.as_object()?;
    let tui_mode = match object.get("tuiMode") {
        None => TuiMode::Inline,
        Some(Value::String(value)) => TuiMode::parse(value)?,
        Some(_) => return None,
    };
    let notifications = match object.get("notifications") {
        None => true,
        Some(value) => value.as_bool()?,
    };
    let collaboration = match object.get("collaboration") {
        None => true,
        Some(value) => value.as_bool()?,
    };
    let scope = match object.get("scope") {
        None => LaunchScope::Project,
        Some(Value::String(value)) => LaunchScope::parse(value)?,
        Some(_) => return None,
    };
    let last_engine = optional_nonempty_string(object.get("lastEngine"))?;
    if last_engine
        .as_deref()
        .is_some_and(|value| !is_engine_id(value))
    {
        return None;
    }
    let last_model = optional_nonempty_string(object.get("lastModel"))?;
    let last_effort = optional_nonempty_string(object.get("lastEffort"))?;
    let mut access_modes = BTreeMap::new();
    if let Some(value) = object.get("accessModes") {
        let values = value.as_object()?;
        for (engine, mode) in values {
            let Some(mode) = mode.as_str() else {
                continue;
            };
            if is_engine_id(engine) && is_restorable_access_mode(engine, mode) {
                access_modes.insert(engine.clone(), mode.to_owned());
            }
        }
    }
    Some(CliConfig {
        tui_mode,
        last_engine,
        last_model,
        last_effort,
        notifications,
        collaboration,
        scope,
        access_modes,
    })
}

#[allow(clippy::option_option)]
fn optional_nonempty_string(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        None => Some(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Some(Some(value.clone())),
        Some(_) => None,
    }
}

pub(crate) fn is_restorable_access_mode(engine: &str, mode: &str) -> bool {
    match engine {
        "claude" => matches!(mode, "default" | "plan"),
        "codex" => matches!(mode, "untrusted" | "on-request"),
        "acp:grok" | "acp:qwen" => mode == "default",
        _ => false,
    }
}

fn default_access_mode(engine: &str) -> Option<&'static str> {
    match engine {
        "claude" | "acp:grok" | "acp:qwen" => Some("default"),
        "codex" => Some("on-request"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_partial_configs_use_exact_legacy_defaults() {
        let root = tempfile::tempdir().expect("temp");
        let path = root.path().join("config.json");
        let mut store = CliConfigStore::new(&path).expect("store");
        assert_eq!(store.load(), CliConfig::default());
        fs::write(
            &path,
            r#"{"lastEngine":"codex","lastModel":"gpt-5.6","lastEffort":"high"}"#,
        )
        .expect("fixture");
        let loaded = store.load();
        assert_eq!(loaded.scope, LaunchScope::Project);
        assert_eq!(loaded.tui_mode, TuiMode::Inline);
        assert_eq!(
            loaded.provider_preferences("codex"),
            (Some("gpt-5.6".into()), Some("high".into()))
        );
        assert_eq!(loaded.provider_preferences("claude"), (None, None));
    }

    #[test]
    fn corrupt_config_is_quarantined_and_atomic_save_preserves_schema_and_modes() {
        let root = tempfile::tempdir().expect("temp");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("mode");
        let path = root.path().join("config.json");
        fs::write(&path, "not-json").expect("fixture");
        let mut store = CliConfigStore::new(&path).expect("store");
        assert_eq!(store.load(), CliConfig::default());
        assert!(std::path::Path::new(&format!("{}.corrupt", path.display())).is_file());
        store.remember_session("codex".into(), Some("gpt-5.6".into()), Some("high".into()));
        store.set_tui_mode(TuiMode::Fullscreen);
        store.save().expect("save");
        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
        let value: Value = serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        assert_eq!(value["tuiMode"], "fullscreen");
        assert_eq!(value["scope"], "project");
        assert_eq!(value["lastEngine"], "codex");
        assert!(value.get("serviceTier").is_none());
    }

    #[test]
    fn a_symlinked_data_directory_loads_instead_of_quarantining_the_config() {
        let root = tempfile::tempdir().expect("temp");
        let real = root.path().join("real");
        fs::create_dir(&real).expect("real");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).expect("mode");
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).expect("symlink");
        let path = linked.join("config.json");
        fs::write(&path, r#"{"lastEngine":"codex"}"#).expect("fixture");

        let mut store = CliConfigStore::new(&path).expect("store");

        assert_eq!(store.load().last_engine.as_deref(), Some("codex"));
        assert!(!PathBuf::from(format!("{}.corrupt", path.display())).exists());
    }

    #[test]
    fn reading_a_config_never_creates_its_parent_directory() {
        let root = tempfile::tempdir().expect("temp");
        let missing = root.path().join("missing");
        let mut store = CliConfigStore::new(missing.join("config.json")).expect("store");

        assert_eq!(store.load(), CliConfig::default());
        assert!(!missing.exists());
    }

    #[test]
    fn null_defaults_and_symlinked_configs_are_rejected_without_reading_the_target() {
        let root = tempfile::tempdir().expect("temp");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).expect("mode");
        let null_path = root.path().join("null-config.json");
        fs::write(&null_path, r#"{"tuiMode":null}"#).expect("null fixture");
        let mut null_store = CliConfigStore::new(&null_path).expect("store");
        assert_eq!(null_store.load(), CliConfig::default());
        assert!(PathBuf::from(format!("{}.corrupt", null_path.display())).is_file());

        let target = root.path().join("do-not-read");
        let target_bytes = b"{\"lastEngine\":\"codex\"}\n";
        fs::write(&target, target_bytes).expect("target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).expect("target mode");
        let link = root.path().join("linked-config.json");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let mut linked_store = CliConfigStore::new(&link).expect("store");

        assert_eq!(linked_store.load(), CliConfig::default());
        assert_eq!(fs::read(&target).expect("target preserved"), target_bytes);
        assert_eq!(
            fs::metadata(&target)
                .expect("target metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
}
