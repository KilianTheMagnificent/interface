use std::path::{Path, PathBuf};

use interface_contracts::appearance::{
    PanePresentation, ProjectPresentation, SurfaceTranslucency, SystemThemes, ThemeId,
    ThemePreference, WorkspacePresentation, WorkspacePresentationV2,
};
use interface_storage::{JsonLimits, read_bounded};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::leased::write_document;
use crate::private_path::{SettingsError, prepare_store};

const APPEARANCE_BYTES: usize = 16 * 1024;

/// A separately versioned appearance file beside the legacy theme file, so
/// adding a preference never changes the accepted `appearance.json` bytes.
struct Sidecar {
    lock: &'static str,
    bytes: usize,
    nodes: usize,
}

const TRANSLUCENCY: Sidecar = Sidecar {
    lock: ".surface-translucency.lock",
    bytes: 4096,
    nodes: 16,
};
const PANE_PRESENTATION: Sidecar = Sidecar {
    lock: ".pane-presentation.lock",
    bytes: 4096,
    nodes: 16,
};
const WORKSPACE_PRESENTATION: Sidecar = Sidecar {
    lock: ".workspace-presentation.lock",
    bytes: 32768,
    nodes: 512,
};
const SYSTEM_THEMES: Sidecar = Sidecar {
    lock: ".system-themes.lock",
    // Two theme ids of at most 64 characters, each up to four UTF-8 bytes.
    bytes: 4096,
    nodes: 16,
};
const PROJECT_PRESENTATION: Sidecar = Sidecar {
    lock: ".project-presentation.lock",
    // Escaped quotes/backslashes can double the validated UTF-8 path budget.
    bytes: 2 * 1024 * 1024,
    nodes: 512,
};

impl Sidecar {
    /// Missing or malformed data reads as the default; unsafe storage fails.
    fn load<T: DeserializeOwned + Default>(&self, file: &Path) -> Result<T, SettingsError> {
        Ok(self.read(file)?.unwrap_or_default())
    }

    /// Missing or malformed data reads as `None`; unsafe storage fails.
    fn read<T: DeserializeOwned>(&self, file: &Path) -> Result<Option<T>, SettingsError> {
        let location = prepare_store(file)?;
        let value = read_bounded(&location.directory, &location.name, self.bytes)?;
        Ok(value
            .as_ref()
            .and_then(|v| serde_json::from_slice(v.bytes()).ok()))
    }

    fn save<T: Serialize + DeserializeOwned>(
        &self,
        file: &Path,
        value: &T,
    ) -> Result<(), SettingsError> {
        let location = prepare_store(file)?;
        write_document(
            &location,
            self.lock,
            value,
            JsonLimits::new(self.bytes, 8, self.nodes)?,
            SettingsError::Storage,
        )
    }
}

/// `theme` is a built-in id or, since appearance surface v5, `custom:<name>`
/// for a theme file. Builds before v5 read a theme file id as malformed and
/// fall back to System, exactly as they treat any unknown theme.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct StoredAppearance {
    theme: ThemeId,
}

/// In-memory preference backed by an owner-private atomic JSON file.
pub struct AppearanceStore {
    location: crate::private_path::StoreLocation,
    preference: ThemeId,
    translucency_file: PathBuf,
    pane_presentation_file: PathBuf,
    workspace_presentation_file: PathBuf,
    project_presentation_file: PathBuf,
    system_themes_file: PathBuf,
}

impl AppearanceStore {
    /// Opens a legacy-compatible file path, safely hardening existing 0755/0644
    /// permissions without ever following a link.
    ///
    /// # Errors
    ///
    /// Refuses relative, linked, hard-linked, non-regular, or unstable storage.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        Ok(Self {
            location: prepare_store(file)?,
            translucency_file: file.with_file_name("surface-translucency.v1.json"),
            pane_presentation_file: file.with_file_name("pane-presentation.v1.json"),
            workspace_presentation_file: file.with_file_name("workspace-presentation.v2.json"),
            project_presentation_file: file.with_file_name("project-presentation.v1.json"),
            system_themes_file: file.with_file_name("system-themes.v1.json"),
            preference: ThemeId::Builtin(ThemePreference::System),
        })
    }

    /// Reads the independently persisted v1 material preferences.
    /// # Errors
    /// Refuses unsafe storage without altering the legacy theme file.
    pub fn translucency(&self) -> Result<SurfaceTranslucency, SettingsError> {
        TRANSLUCENCY.load(&self.translucency_file)
    }

    /// Saves already validated material preferences.
    /// # Errors
    /// Refuses unsafe storage, concurrent writers, or unproven durability.
    pub fn save_translucency(&self, value: SurfaceTranslucency) -> Result<(), SettingsError> {
        TRANSLUCENCY.save(&self.translucency_file, &value)
    }

    /// Reads the optional CLI pane appearance.
    /// # Errors
    /// Refuses unsafe storage.
    pub fn pane_presentation(&self) -> Result<PanePresentation, SettingsError> {
        PANE_PRESENTATION.load(&self.pane_presentation_file)
    }

    /// Saves appearance without mutating any workspace or process.
    /// # Errors
    /// Refuses unsafe storage, concurrent writes, or unproven durability.
    pub fn save_pane_presentation(&self, value: PanePresentation) -> Result<(), SettingsError> {
        PANE_PRESENTATION.save(&self.pane_presentation_file, &value)
    }

    /// Reads the themes Follow macOS switches between.
    /// # Errors
    /// Refuses unsafe storage.
    pub fn system_themes(&self) -> Result<SystemThemes, SettingsError> {
        SYSTEM_THEMES.load(&self.system_themes_file)
    }

    /// Saves an already validated Follow macOS pairing.
    /// # Errors
    /// Refuses unsafe storage, concurrent writers, or unproven durability.
    pub fn save_system_themes(&self, value: &SystemThemes) -> Result<(), SettingsError> {
        SYSTEM_THEMES.save(&self.system_themes_file, value)
    }

    /// Reads versioned reading and navigation preferences. Until the v2 file
    /// exists, what an earlier build saved in the v1 file carries forward:
    /// builds before appearance surface v6 keep reading that file, so it is
    /// read here and never written.
    /// # Errors
    /// Refuses unsafe storage.
    pub fn workspace_presentation(&self) -> Result<WorkspacePresentationV2, SettingsError> {
        if let Some(current) = WORKSPACE_PRESENTATION.read(&self.workspace_presentation_file)? {
            return Ok(current);
        }
        let legacy = self
            .workspace_presentation_file
            .with_file_name("workspace-presentation.v1.json");
        Ok(WORKSPACE_PRESENTATION
            .read::<WorkspacePresentation>(&legacy)?
            .map(WorkspacePresentationV2::from)
            .unwrap_or_default())
    }

    /// Reads project pins independently of workspace pins and legacy files.
    /// # Errors
    /// Refuses unsafe storage.
    pub fn project_presentation(&self) -> Result<ProjectPresentation, SettingsError> {
        PROJECT_PRESENTATION.load(&self.project_presentation_file)
    }

    /// Saves validated project pins through the Rust writer.
    /// # Errors
    /// Refuses unsafe storage, concurrent writers, or unproven durability.
    pub fn save_project_presentation(
        &self,
        value: &ProjectPresentation,
    ) -> Result<(), SettingsError> {
        PROJECT_PRESENTATION.save(&self.project_presentation_file, value)
    }

    /// Saves reading and navigation preferences through the Rust writer, in
    /// the v2 file only: the v1 file stays as the earlier build left it.
    /// # Errors
    /// Refuses unsafe storage, concurrent writes, or unproven durability.
    pub fn save_workspace_presentation(
        &self,
        value: &WorkspacePresentationV2,
    ) -> Result<(), SettingsError> {
        WORKSPACE_PRESENTATION.save(&self.workspace_presentation_file, value)
    }

    #[must_use]
    pub const fn get(&self) -> &ThemeId {
        &self.preference
    }

    /// Loads a preference. Missing, malformed, or obsolete theme data defaults
    /// to System exactly as the Electron store did; unsafe storage still fails.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe or over-budget filesystem state.
    pub fn load(&mut self) -> Result<ThemeId, SettingsError> {
        let Some(file) = read_bounded(
            &self.location.directory,
            &self.location.name,
            APPEARANCE_BYTES,
        )?
        else {
            self.preference = ThemeId::Builtin(ThemePreference::System);
            return Ok(self.preference.clone());
        };
        self.preference = serde_json::from_slice::<StoredAppearance>(file.bytes())
            .map_or(ThemeId::Builtin(ThemePreference::System), |stored| {
                stored.theme.canonical()
            });
        Ok(self.preference.clone())
    }

    /// Atomically saves one validated enum preference under a short-lived
    /// one-writer lease.
    ///
    /// # Errors
    ///
    /// Returns an error for drift, another writer, or unproven durability.
    pub fn save(&mut self, preference: impl Into<ThemeId>) -> Result<(), SettingsError> {
        let preference = preference.into().canonical();
        write_document(
            &self.location,
            ".appearance.lock",
            &StoredAppearance {
                theme: preference.clone(),
            },
            JsonLimits::new(APPEARANCE_BYTES, 8, 16)?,
            SettingsError::Storage,
        )?;
        self.preference = preference;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use interface_contracts::appearance::{
        PanePresentation, ProjectPresentation, SurfaceTranslucency, ThemePreference,
        WorkspacePresentationV2,
    };
    use tempfile::TempDir;

    use super::AppearanceStore;

    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn defaults_malformed_and_persists_every_enum_value() {
        let temp = TempDir::new().expect("temp");
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).expect("open");
        assert_eq!(store.load().expect("missing load"), ThemePreference::System);
        fs::write(&file, "not json").expect("malformed fixture");
        #[cfg(unix)]
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("file mode");
        assert_eq!(
            AppearanceStore::open(&file)
                .expect("reopen")
                .load()
                .expect("malformed load"),
            ThemePreference::System
        );
        let mut store = AppearanceStore::open(&file).expect("reopen save");
        for theme in [
            ThemePreference::Light,
            ThemePreference::Dark,
            ThemePreference::System,
            ThemePreference::CatppuccinMocha,
        ] {
            store.save(theme).expect("save theme");
            assert_eq!(*store.get(), theme);
            assert_eq!(
                AppearanceStore::open(&file)
                    .expect("reload open")
                    .load()
                    .expect("reload"),
                theme
            );
        }
    }

    #[test]
    fn retired_themes_load_and_save_as_their_replacement() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).unwrap();
        store.save(ThemePreference::WarmEditorialDark).unwrap();
        assert_eq!(*store.get(), ThemePreference::AnthropicDark);
        let saved = fs::read_to_string(&file).unwrap();
        assert!(saved.contains("anthropic-dark"), "{saved}");
        fs::write(&file, r#"{"theme":"nightshade"}"#).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            AppearanceStore::open(&file).unwrap().load().unwrap(),
            ThemePreference::GrokBuild
        );
    }

    #[test]
    fn theme_files_and_the_macos_pairing_persist() {
        use interface_contracts::appearance::{SystemThemes, ThemeId};

        let temp = TempDir::new().unwrap();
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).unwrap();
        let moss: ThemeId = "custom:Moss".parse().unwrap();
        store.save(moss.clone()).unwrap();
        assert!(
            fs::read_to_string(&file)
                .unwrap()
                .contains("\"theme\": \"custom:Moss\"")
        );
        assert_eq!(AppearanceStore::open(&file).unwrap().load().unwrap(), moss);
        assert_eq!(store.system_themes().unwrap(), SystemThemes::default());
        let pairing: SystemThemes = serde_json::from_value(serde_json::json!({
            "version": 1, "light": "solarized-light", "dark": "custom:Moss"
        }))
        .unwrap();
        store.save_system_themes(&pairing).unwrap();
        let theme_bytes = fs::read(&file).unwrap();
        let reopened = AppearanceStore::open(&file).unwrap();
        assert_eq!(reopened.system_themes().unwrap(), pairing);
        assert_eq!(fs::read(&file).unwrap(), theme_bytes);
        fs::write(
            temp.path().join("system-themes.v1.json"),
            br#"{"version":1,"light":"system","dark":"dark"}"#,
        )
        .unwrap();
        assert_eq!(reopened.system_themes().unwrap(), SystemThemes::default());
        assert!(temp.path().join(".system-themes.lock").exists());
    }

    #[test]
    fn written_bytes_match_the_pinned_format() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).unwrap();
        store.save(ThemePreference::CatppuccinMocha).unwrap();
        store
            .save_translucency(
                serde_json::from_value(serde_json::json!({"version":1,"terminal":75,"sidebar":30}))
                    .unwrap(),
            )
            .unwrap();
        store
            .save_pane_presentation(
                serde_json::from_value(serde_json::json!({"version":1,"paneStyle":"clean"}))
                    .unwrap(),
            )
            .unwrap();
        let workspace: WorkspacePresentationV2 = serde_json::from_value(serde_json::json!({
            "version": 2,
            "terminalFontSize": 11.5,
            "pinnedWorkspaces": ["w-1", "w-2"]
        }))
        .unwrap();
        store.save_workspace_presentation(&workspace).unwrap();
        let project: ProjectPresentation = serde_json::from_value(serde_json::json!({
            "version": 1,
            "pinnedProjects": ["/projects/one", "/projects/\\\"quoted\\\""]
        }))
        .unwrap();
        store.save_project_presentation(&project).unwrap();
        for (golden, name) in [
            ("appearance.theme.json", "appearance.json"),
            (
                "appearance.translucency.json",
                "surface-translucency.v1.json",
            ),
            ("appearance.pane.json", "pane-presentation.v1.json"),
            (
                "appearance.workspace.json",
                "workspace-presentation.v2.json",
            ),
            ("appearance.project.json", "project-presentation.v1.json"),
        ] {
            crate::golden::assert_golden(golden, temp.path(), &temp.path().join(name));
        }
        for lock in [
            ".appearance.lock",
            ".surface-translucency.lock",
            ".pane-presentation.lock",
            ".workspace-presentation.lock",
            ".project-presentation.lock",
        ] {
            assert!(temp.path().join(lock).exists(), "{lock}");
        }
        let mut reopened = AppearanceStore::open(&file).unwrap();
        assert_eq!(reopened.load().unwrap(), ThemePreference::CatppuccinMocha);
        assert_eq!(reopened.workspace_presentation().unwrap(), workspace);
        assert_eq!(reopened.project_presentation().unwrap(), project);
    }

    #[test]
    fn a_saved_v1_terminal_size_and_pins_carry_into_v2_and_the_v1_file_is_left_alone() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("appearance.json");
        let store = AppearanceStore::open(&file).unwrap();
        // Nothing saved by any build: the owner's 11.5px default.
        assert_eq!(
            store.workspace_presentation().unwrap(),
            WorkspacePresentationV2::default()
        );
        let v1 = temp.path().join("workspace-presentation.v1.json");
        let saved = "{\n  \"version\": 1,\n  \"terminalFontSize\": 14,\n  \"pinnedWorkspaces\": [\n    \"w-1\"\n  ]\n}\n";
        fs::write(&v1, saved).unwrap();
        let carried: WorkspacePresentationV2 = serde_json::from_value(serde_json::json!({
            "version": 2, "terminalFontSize": 14, "pinnedWorkspaces": ["w-1"]
        }))
        .unwrap();
        assert_eq!(store.workspace_presentation().unwrap(), carried);
        assert!(!temp.path().join("workspace-presentation.v2.json").exists());

        let chosen: WorkspacePresentationV2 = serde_json::from_value(serde_json::json!({
            "version": 2, "terminalFontSize": 11.5, "pinnedWorkspaces": ["w-1", "w-2"]
        }))
        .unwrap();
        store.save_workspace_presentation(&chosen).unwrap();
        assert_eq!(fs::read_to_string(&v1).unwrap(), saved);
        let reopened = AppearanceStore::open(&file).unwrap();
        assert_eq!(reopened.workspace_presentation().unwrap(), chosen);

        // A v2 file nobody can read falls back to the v1 file, not to nothing.
        fs::write(temp.path().join("workspace-presentation.v2.json"), "{").unwrap();
        assert_eq!(reopened.workspace_presentation().unwrap(), carried);
    }

    #[test]
    fn maximum_escaped_pins_fit_the_store_and_survive_reopen() {
        let temp = TempDir::new().unwrap();
        let store = AppearanceStore::open(&temp.path().join("appearance.json")).unwrap();
        let paths: Vec<_> = (0..128)
            .map(|i| format!("/{i:03}{}", "\\".repeat(4092)))
            .collect();
        let value: ProjectPresentation =
            serde_json::from_value(serde_json::json!({"version":1,"pinnedProjects":paths}))
                .unwrap();
        store.save_project_presentation(&value).unwrap();
        assert_eq!(store.project_presentation().unwrap(), value);
    }

    #[test]
    fn translucency_survives_reload_without_changing_legacy_theme_bytes() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).unwrap();
        store.save(ThemePreference::Dark).unwrap();
        let original = fs::read(&file).unwrap();
        assert_eq!(
            serde_json::to_value(store.translucency().unwrap()).unwrap(),
            serde_json::json!({"version":1,"terminal":0,"sidebar":0})
        );
        let value = serde_json::json!({"version":1,"terminal":75,"sidebar":30});
        store
            .save_translucency(serde_json::from_value(value.clone()).unwrap())
            .unwrap();
        let mut reopened = AppearanceStore::open(&file).unwrap();
        assert_eq!(reopened.load().unwrap(), ThemePreference::Dark);
        assert_eq!(
            serde_json::to_value(reopened.translucency().unwrap()).unwrap(),
            value
        );
        assert_eq!(fs::read(&file).unwrap(), original);
        let surfaces = temp.path().join("surface-translucency.v1.json");
        fs::write(&surfaces, br#"{"version":1,"terminal":999,"sidebar":0}"#).unwrap();
        assert_eq!(
            reopened.translucency().unwrap(),
            SurfaceTranslucency::default()
        );
        assert_eq!(fs::read(&file).unwrap(), original);
    }

    #[test]
    fn pane_style_preserves_other_preferences_and_defaults_invalid_data() {
        let temp = TempDir::new().unwrap();
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).unwrap();
        store.save(ThemePreference::Dark).unwrap();
        store
            .save_translucency(SurfaceTranslucency::default())
            .unwrap();
        let theme_bytes = fs::read(&file).unwrap();
        let surface_file = temp.path().join("surface-translucency.v1.json");
        let surface_bytes = fs::read(&surface_file).unwrap();
        let value = serde_json::json!({"version":1,"paneStyle":"clean"});
        store
            .save_pane_presentation(serde_json::from_value(value.clone()).unwrap())
            .unwrap();
        let reopened = AppearanceStore::open(&file).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.pane_presentation().unwrap()).unwrap(),
            value
        );
        fs::write(
            temp.path().join("pane-presentation.v1.json"),
            br#"{"version":2,"paneStyle":"clean"}"#,
        )
        .unwrap();
        assert_eq!(
            reopened.pane_presentation().unwrap(),
            PanePresentation::default()
        );
        assert_eq!(fs::read(&file).unwrap(), theme_bytes);
        assert_eq!(fs::read(&surface_file).unwrap(), surface_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn project_pins_refuse_linked_store_without_touching_destination() {
        let temp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let destination = other.path().join("untouched.json");
        fs::write(&destination, b"untouched").unwrap();
        symlink(
            &destination,
            temp.path().join("project-presentation.v1.json"),
        )
        .unwrap();
        let store = AppearanceStore::open(&temp.path().join("appearance.json")).unwrap();
        assert!(store.project_presentation().is_err());
        assert!(
            store
                .save_project_presentation(&ProjectPresentation::default())
                .is_err()
        );
        assert_eq!(fs::read(destination).unwrap(), b"untouched");
    }

    #[cfg(unix)]
    #[test]
    fn pane_style_refuses_linked_store_without_touching_destination() {
        let temp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let destination = other.path().join("untouched.json");
        fs::write(&destination, b"untouched").unwrap();
        symlink(&destination, temp.path().join("pane-presentation.v1.json")).unwrap();
        let store = AppearanceStore::open(&temp.path().join("appearance.json")).unwrap();
        assert!(store.pane_presentation().is_err());
        assert!(
            store
                .save_pane_presentation(PanePresentation::default())
                .is_err()
        );
        assert_eq!(fs::read(destination).unwrap(), b"untouched");
    }

    #[cfg(unix)]
    #[test]
    fn translucency_refuses_linked_store_without_touching_destination() {
        let temp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let destination = other.path().join("untouched.json");
        fs::write(&destination, b"untouched").unwrap();
        symlink(
            &destination,
            temp.path().join("surface-translucency.v1.json"),
        )
        .unwrap();
        let store = AppearanceStore::open(&temp.path().join("appearance.json")).unwrap();
        assert!(store.translucency().is_err());
        assert!(
            store
                .save_translucency(SurfaceTranslucency::default())
                .is_err()
        );
        assert_eq!(fs::read(destination).unwrap(), b"untouched");
    }

    #[test]
    fn reads_and_saves_of_a_private_store_never_touch_its_modes() {
        use std::os::unix::fs::MetadataExt;

        let temp = TempDir::new().expect("temp");
        let file = temp.path().join("appearance.json");
        let mut store = AppearanceStore::open(&file).expect("open");
        store.save(ThemePreference::Dark).expect("save theme");
        store
            .save_translucency(SurfaceTranslucency::default())
            .expect("save sidecar");
        let sidecar = temp.path().join("surface-translucency.v1.json");
        let changed = |path: &std::path::Path| {
            let metadata = fs::metadata(path).expect("metadata");
            (metadata.ctime(), metadata.ctime_nsec())
        };
        let before = (changed(temp.path()), changed(&sidecar));
        std::thread::sleep(std::time::Duration::from_millis(20));
        store.translucency().expect("read sidecar");
        let _ = AppearanceStore::open(&file).expect("reopen").load();
        assert_eq!((changed(temp.path()), changed(&sidecar)), before);
    }

    #[cfg(unix)]
    #[test]
    fn repairs_modes_and_refuses_links() {
        let temp = TempDir::new().expect("temp");
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755)).expect("loose dir");
        let file = temp.path().join("appearance.json");
        fs::write(&file, r#"{"theme":"light"}"#).expect("fixture");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("loose file");
        let mut store = AppearanceStore::open(&file).expect("harden");
        assert_eq!(store.load().expect("load"), ThemePreference::Light);
        assert_eq!(
            fs::metadata(temp.path())
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let other = TempDir::new().expect("other");
        let linked = other.path().join("appearance.json");
        symlink(&file, &linked).expect("link");
        assert!(AppearanceStore::open(&linked).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_unreadable_and_directory_targets() {
        let temp = TempDir::new().expect("temp");
        let unreadable = temp.path().join("appearance.json");
        fs::write(&unreadable, r#"{"theme":"dark"}"#).expect("fixture");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("unreadable");
        assert!(AppearanceStore::open(&unreadable).is_err());
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).expect("restore");

        let dir_temp = TempDir::new().expect("dir temp");
        let as_dir = dir_temp.path().join("appearance.json");
        fs::create_dir(&as_dir).expect("directory target");
        fs::set_permissions(&as_dir, fs::Permissions::from_mode(0o700)).expect("dir mode");
        assert!(AppearanceStore::open(&as_dir).is_err());
    }
}
