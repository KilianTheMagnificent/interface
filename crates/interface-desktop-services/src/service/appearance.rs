//! The appearance store and the get/set appearance operations.

use std::path::PathBuf;
use std::sync::MutexGuard;

use interface_contracts::appearance::{
    AppearanceChange, AppearanceState, PanePresentation, ProjectPresentation, SurfaceTranslucency,
    SystemThemes, ThemeId, ThemePreference, WorkspacePresentationV2, WorkspacePresentationView,
};
use interface_settings::{AppearanceStore, CustomThemeLibrary, SettingsError};
use serde_json::Value;

use super::{DesktopService, encode, map_settings_error};
use crate::DesktopServiceError;
use crate::payload::{GetAppearancePayload, SetAppearancePayload, decode};

pub(super) enum RecoverableAppearanceStore {
    Ready(AppearanceStore),
    Unavailable(PathBuf),
}

impl RecoverableAppearanceStore {
    fn snapshot(
        &self,
        themes: &CustomThemeLibrary,
        system_dark: bool,
        surface_version: Option<u8>,
    ) -> Result<AppearanceState, DesktopServiceError> {
        if surface_version.is_some_and(|version| !(1..=6).contains(&version)) {
            return Err(DesktopServiceError::new(
                "invalid-payload",
                "Unsupported appearance surface version.",
                false,
            ));
        }
        let mut state = if matches!(surface_version, Some(5 | 6)) {
            // Theme files and the Follow macOS pairing arrived with v5.
            let system = self.read_or_default(AppearanceStore::system_themes)?;
            let mut state = AppearanceState::resolve(self.get(), &system, system_dark, |name| {
                themes.is_paintable(name)
            });
            state.theme_css = match &state.resolved {
                ThemeId::Custom(name) => themes.read(name).and_then(|file| file.css),
                ThemeId::Builtin(_) => None,
            };
            state.custom_themes = Some(themes.list());
            state.system_themes = Some(system);
            state
        } else {
            // Earlier surfaces only know built-ins: a theme file reads as
            // Follow macOS over the built-in pair.
            let preference = match self.get() {
                ThemeId::Builtin(theme) => theme,
                ThemeId::Custom(_) => ThemePreference::System,
            };
            AppearanceState::new(preference, system_dark)
        };
        if surface_version.is_some() {
            state.translucency = Some(self.read_or_default(AppearanceStore::translucency)?);
        }
        if matches!(surface_version, Some(2..=6)) {
            state.pane_presentation =
                Some(self.read_or_default(AppearanceStore::pane_presentation)?);
        }
        if matches!(surface_version, Some(3..=6)) {
            let current = self.read_or_default(AppearanceStore::workspace_presentation)?;
            // Half-step terminal sizes arrived with v6; v3–v5 read the v1 view.
            state.workspace_presentation = Some(if surface_version == Some(6) {
                WorkspacePresentationView::Current(current)
            } else {
                WorkspacePresentationView::Legacy(current.legacy())
            });
        }
        if matches!(surface_version, Some(4..=6)) {
            state.project_presentation =
                Some(self.read_or_default(AppearanceStore::project_presentation)?);
        }
        Ok(state)
    }

    /// An unavailable store reads as the defaults it would have started with.
    fn read_or_default<T: Default>(
        &self,
        read: impl FnOnce(&AppearanceStore) -> Result<T, SettingsError>,
    ) -> Result<T, DesktopServiceError> {
        match self {
            Self::Ready(store) => read(store).map_err(map_settings_error),
            Self::Unavailable(_) => Ok(T::default()),
        }
    }

    /// The store a save writes through; an unavailable one refuses every save.
    fn ready(&self) -> Result<&AppearanceStore, DesktopServiceError> {
        match self {
            Self::Ready(store) => Ok(store),
            Self::Unavailable(_) => Err(DesktopServiceError::appearance_unavailable()),
        }
    }

    fn save_project_presentation(
        &self,
        value: &ProjectPresentation,
    ) -> Result<(), DesktopServiceError> {
        self.ready()?
            .save_project_presentation(value)
            .map_err(map_settings_error)
    }

    fn save_workspace_presentation(
        &self,
        value: &WorkspacePresentationV2,
    ) -> Result<(), DesktopServiceError> {
        self.ready()?
            .save_workspace_presentation(value)
            .map_err(map_settings_error)
    }

    fn save_pane_presentation(&self, value: PanePresentation) -> Result<(), DesktopServiceError> {
        self.ready()?
            .save_pane_presentation(value)
            .map_err(map_settings_error)
    }

    fn save_translucency(&self, value: SurfaceTranslucency) -> Result<(), DesktopServiceError> {
        self.ready()?
            .save_translucency(value)
            .map_err(map_settings_error)
    }

    fn save_system_themes(&self, value: &SystemThemes) -> Result<(), DesktopServiceError> {
        self.ready()?
            .save_system_themes(value)
            .map_err(map_settings_error)
    }

    fn get(&self) -> ThemeId {
        match self {
            Self::Ready(store) => store.get().clone(),
            Self::Unavailable(_) => ThemeId::Builtin(ThemePreference::System),
        }
    }

    fn save(&mut self, preference: ThemeId) -> Result<(), DesktopServiceError> {
        match self {
            Self::Ready(store) => store.save(preference).map_err(map_settings_error),
            // Never write to a target that failed open/load safety checks, and
            // never auto-reopen: set on Unavailable is a typed refusal.
            Self::Unavailable(path) => {
                eprintln!(
                    "[desktop] appearance file is unavailable; the theme was not saved: {}",
                    path.display()
                );
                Err(DesktopServiceError::appearance_unavailable())
            }
        }
    }
}

impl DesktopService {
    fn lock_appearance(
        &self,
    ) -> Result<MutexGuard<'_, RecoverableAppearanceStore>, DesktopServiceError> {
        self.appearance
            .lock()
            .map_err(|_| DesktopServiceError::state_unavailable())
    }
    pub(super) fn get_appearance(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<GetAppearancePayload>(operation, payload)?;
        encode(
            operation,
            self.lock_appearance()?.snapshot(
                &self.custom_themes,
                request.system_dark,
                request.surface_version,
            )?,
        )
    }
    pub(super) fn set_appearance(
        &self,
        operation: &str,
        payload: Value,
    ) -> Result<Value, DesktopServiceError> {
        let request = decode::<SetAppearancePayload>(operation, payload).map_err(|_| {
            DesktopServiceError::new("invalid-payload", "Unknown appearance theme.", false)
        })?;
        let mut store = self.lock_appearance()?;
        // Validate the requested response version before either store can change.
        store.snapshot(
            &self.custom_themes,
            request.system_dark,
            request.surface_version,
        )?;
        match request.preference {
            AppearanceChange::Theme(preference) => {
                if matches!(preference, ThemeId::Custom(_))
                    && !matches!(request.surface_version, Some(5 | 6))
                {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "Theme files require appearance surface version 5.",
                        false,
                    ));
                }
                store.save(preference)?;
            }
            AppearanceChange::SystemThemes(value) => {
                if !matches!(request.surface_version, Some(5 | 6)) {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "System themes require appearance surface version 5.",
                        false,
                    ));
                }
                store.save_system_themes(&value)?;
            }
            AppearanceChange::Translucency(value) => {
                if !matches!(request.surface_version, Some(1..=6)) {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "Translucency requires surface version 1.",
                        false,
                    ));
                }
                store.save_translucency(value)?;
            }
            AppearanceChange::ProjectPresentation(value) => {
                if !matches!(request.surface_version, Some(4..=6)) {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "Project presentation requires appearance surface version 4.",
                        false,
                    ));
                }
                store.save_project_presentation(&value)?;
            }
            AppearanceChange::WorkspacePresentation(value) => {
                // v1 belongs to surfaces v3–v5; v6 speaks v2 only.
                if !matches!(request.surface_version, Some(3..=5)) {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "Workspace presentation requires appearance surface version 3.",
                        false,
                    ));
                }
                store.save_workspace_presentation(&value.into())?;
            }
            AppearanceChange::WorkspacePresentationV2(value) => {
                if request.surface_version != Some(6) {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "Workspace presentation version 2 requires appearance surface version 6.",
                        false,
                    ));
                }
                store.save_workspace_presentation(&value)?;
            }
            AppearanceChange::PanePresentation(value) => {
                if !matches!(request.surface_version, Some(2..=6)) {
                    return Err(DesktopServiceError::new(
                        "invalid-payload",
                        "Pane presentation requires appearance surface version 2.",
                        false,
                    ));
                }
                store.save_pane_presentation(value)?;
            }
        }
        encode(
            operation,
            store.snapshot(
                &self.custom_themes,
                request.system_dark,
                request.surface_version,
            )?,
        )
    }
}
