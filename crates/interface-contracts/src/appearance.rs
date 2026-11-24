use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A persisted colour theme. Sauteed Paper, Warm Editorial and Nightshade were
/// retired on 2026-09-28 (owner): their identifiers stay accepted so saved
/// preferences and older clients keep parsing, but [`ThemePreference::canonical`]
/// maps each to the palette that replaced it, and nothing emits them again.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemePreference {
    System,
    Light,
    Dark,
    SauteedPaperLight,
    SauteedPaperDark,
    WarmEditorialLight,
    WarmEditorialDark,
    AnthropicLight,
    AnthropicDark,
    SolarizedLight,
    SolarizedDark,
    CatppuccinMocha,
    GrokBuild,
    Nightshade,
    Hermes,
    HermesLight,
}

pub const THEME_PREFERENCES: [ThemePreference; 16] = [
    ThemePreference::System,
    ThemePreference::Light,
    ThemePreference::Dark,
    ThemePreference::SauteedPaperLight,
    ThemePreference::SauteedPaperDark,
    ThemePreference::WarmEditorialLight,
    ThemePreference::WarmEditorialDark,
    ThemePreference::AnthropicLight,
    ThemePreference::AnthropicDark,
    ThemePreference::SolarizedLight,
    ThemePreference::SolarizedDark,
    ThemePreference::CatppuccinMocha,
    ThemePreference::GrokBuild,
    ThemePreference::Nightshade,
    ThemePreference::Hermes,
    ThemePreference::HermesLight,
];

/// Desktop wire snapshot. `resolved` is guaranteed by constructors to be a
/// non-System theme even though both fields share one identifier type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceState {
    pub preference: ThemeId,
    pub resolved: ThemeId,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "translucency"
    )]
    pub translucency: Option<SurfaceTranslucency>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "panePresentation"
    )]
    pub pane_presentation: Option<PanePresentation>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "workspacePresentation"
    )]
    pub workspace_presentation: Option<WorkspacePresentationView>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "projectPresentation"
    )]
    pub project_presentation: Option<ProjectPresentation>,
    /// Appearance surface v5: every theme file in the user's themes folder.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "customThemes"
    )]
    pub custom_themes: Option<Vec<CustomThemeSummary>>,
    /// Appearance surface v5: the themes Follow macOS switches between.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "systemThemes"
    )]
    pub system_themes: Option<SystemThemes>,
    /// Appearance surface v5: the CSS of `resolved` when it names a theme
    /// file, so the renderer paints it without a second read.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "themeCss")]
    pub theme_css: Option<String>,
}

impl AppearanceState {
    #[must_use]
    pub const fn new(preference: ThemePreference, system_dark: bool) -> Self {
        let preference = preference.canonical();
        Self {
            preference: ThemeId::Builtin(preference),
            resolved: ThemeId::Builtin(resolve_theme_preference(preference, system_dark)),
            translucency: None,
            pane_presentation: None,
            workspace_presentation: None,
            project_presentation: None,
            custom_themes: None,
            system_themes: None,
            theme_css: None,
        }
    }

    /// The snapshot for any preference. Follow macOS takes the slot the
    /// system is in; a theme file that is not in the folder (renamed,
    /// deleted, unreadable) paints what Follow macOS would, while the
    /// preference keeps naming it so the theme returns with its file.
    #[must_use]
    pub fn resolve(
        preference: ThemeId,
        system: &SystemThemes,
        system_dark: bool,
        available: impl Fn(&CustomThemeName) -> bool,
    ) -> Self {
        let preference = preference.canonical();
        let usable = |theme: &ThemeId| match theme {
            ThemeId::Builtin(ThemePreference::System) => false,
            ThemeId::Builtin(_) => true,
            ThemeId::Custom(name) => available(name),
        };
        let follow = || {
            let slot = if system_dark {
                system.dark()
            } else {
                system.light()
            };
            if usable(slot) {
                slot.clone()
            } else {
                ThemeId::Builtin(resolve_theme_preference(
                    ThemePreference::System,
                    system_dark,
                ))
            }
        };
        let resolved = if usable(&preference) {
            preference.clone()
        } else {
            follow()
        };
        Self {
            preference,
            resolved,
            translucency: None,
            pane_presentation: None,
            workspace_presentation: None,
            project_presentation: None,
            custom_themes: None,
            system_themes: None,
            theme_css: None,
        }
    }
}

/// Wire prefix of a theme kept as a file in the user's themes folder.
pub const CUSTOM_THEME_PREFIX: &str = "custom:";
/// The longest theme file name, in characters, without `.css`.
pub const CUSTOM_THEME_NAME_MAX_CHARS: usize = 64;

/// The name of a theme file without its `.css` extension. It is also the name
/// Settings shows, so it stays a plain single path segment.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CustomThemeName(String);

impl CustomThemeName {
    /// # Errors
    ///
    /// Refuses empty, over-long, hidden, padded or path-like names and any
    /// control character.
    pub fn new(name: impl Into<String>) -> Result<Self, &'static str> {
        let name = name.into();
        let count = name.chars().count();
        if count == 0
            || count > CUSTOM_THEME_NAME_MAX_CHARS
            || name.starts_with('.')
            || name.trim() != name
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
        {
            return Err("Invalid theme file name.");
        }
        Ok(Self(name))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file this theme is read from inside the themes folder.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.css", self.0)
    }
}

impl fmt::Display for CustomThemeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a preference or a Follow macOS slot names: a built-in palette, or a
/// theme file (`custom:<name>` on the wire and on disk).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ThemeId {
    Builtin(ThemePreference),
    Custom(CustomThemeName),
}

impl ThemeId {
    /// Retired built-ins become their replacement; theme files are themselves.
    #[must_use]
    pub fn canonical(self) -> Self {
        match self {
            Self::Builtin(theme) => Self::Builtin(theme.canonical()),
            custom @ Self::Custom(_) => custom,
        }
    }

    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::Builtin(theme) => theme.id().to_owned(),
            Self::Custom(name) => format!("{CUSTOM_THEME_PREFIX}{name}"),
        }
    }
}

impl PartialEq<ThemePreference> for ThemeId {
    fn eq(&self, other: &ThemePreference) -> bool {
        matches!(self, Self::Builtin(theme) if theme == other)
    }
}

impl From<ThemePreference> for ThemeId {
    fn from(theme: ThemePreference) -> Self {
        Self::Builtin(theme)
    }
}

impl fmt::Display for ThemeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.id())
    }
}

impl FromStr for ThemeId {
    type Err = ParseThemePreferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.strip_prefix(CUSTOM_THEME_PREFIX) {
            Some(name) => CustomThemeName::new(name)
                .map(Self::Custom)
                .map_err(|_| ParseThemePreferenceError),
            None => value.parse().map(Self::Builtin),
        }
    }
}

impl Serialize for ThemeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.id())
    }
}

impl<'de> Deserialize<'de> for ThemeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value
            .parse()
            .map_err(|_| serde::de::Error::custom("unknown theme"))
    }
}

/// Whether a palette is drawn light or dark: the native window, form controls
/// and the Settings group it is listed under follow it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeKind {
    Light,
    Dark,
}

/// Appearance surface v5. What one file in the themes folder offers Settings
/// and the main process; the CSS itself travels only for the painted theme.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomThemeSummary {
    pub id: ThemeId,
    pub name: String,
    pub kind: ThemeKind,
    /// The window fill, when the file states one as a hex colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Changes whenever the file's bytes do.
    pub revision: String,
    /// Swatch colours the file declares, by custom property name.
    #[serde(default)]
    pub preview: std::collections::BTreeMap<String, String>,
    /// Why the file cannot be painted (too large, not text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// Appearance surface v5. The theme Follow macOS paints in each system
/// appearance. Either may name a theme file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "SystemThemesWire")]
pub struct SystemThemes {
    version: u8,
    light: ThemeId,
    dark: ThemeId,
}

impl Default for SystemThemes {
    fn default() -> Self {
        Self {
            version: 1,
            light: ThemeId::Builtin(ThemePreference::Light),
            dark: ThemeId::Builtin(ThemePreference::Dark),
        }
    }
}

impl SystemThemes {
    #[must_use]
    pub const fn light(&self) -> &ThemeId {
        &self.light
    }

    #[must_use]
    pub const fn dark(&self) -> &ThemeId {
        &self.dark
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemThemesWire {
    version: u8,
    light: ThemeId,
    dark: ThemeId,
}

impl TryFrom<SystemThemesWire> for SystemThemes {
    type Error = &'static str;
    fn try_from(value: SystemThemesWire) -> Result<Self, Self::Error> {
        let system = ThemeId::Builtin(ThemePreference::System);
        if value.version != 1 || value.light == system || value.dark == system {
            return Err("System themes require version 1 and two named themes.");
        }
        Ok(Self {
            version: value.version,
            light: value.light.canonical(),
            dark: value.dark.canonical(),
        })
    }
}

/// Appearance surface protocol v2: both styles draw the same CLI processes.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneStyle {
    #[default]
    Terminal,
    Clean,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "PanePresentationWire")]
pub struct PanePresentation {
    version: u8,
    #[serde(rename = "paneStyle")]
    pane_style: PaneStyle,
}

impl Default for PanePresentation {
    fn default() -> Self {
        Self {
            version: 1,
            pane_style: PaneStyle::Terminal,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PanePresentationWire {
    version: u8,
    pane_style: PaneStyle,
}

impl TryFrom<PanePresentationWire> for PanePresentation {
    type Error = &'static str;
    fn try_from(value: PanePresentationWire) -> Result<Self, Self::Error> {
        if value.version != 1 {
            return Err("Pane presentation requires version 1.");
        }
        Ok(Self {
            version: value.version,
            pane_style: value.pane_style,
        })
    }
}

/// Appearance surface v3. Stored separately from every accepted legacy schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "WorkspacePresentationWire", rename_all = "camelCase")]
pub struct WorkspacePresentation {
    version: u8,
    terminal_font_size: u8,
    pinned_workspaces: Vec<String>,
}
impl Default for WorkspacePresentation {
    fn default() -> Self {
        Self {
            version: 1,
            terminal_font_size: 12,
            pinned_workspaces: Vec::new(),
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspacePresentationWire {
    version: u8,
    terminal_font_size: u8,
    pinned_workspaces: Vec<String>,
}
/// At most 128 distinct workspace ids of 1–128 `[A-Za-z0-9_-]` characters.
fn valid_workspace_pins(pins: &[String]) -> bool {
    pins.len() <= 128
        && pins.iter().enumerate().all(|(i, id)| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                && !pins[..i].contains(id)
        })
}

impl TryFrom<WorkspacePresentationWire> for WorkspacePresentation {
    type Error = &'static str;
    fn try_from(value: WorkspacePresentationWire) -> Result<Self, Self::Error> {
        if value.version != 1
            || !(12..=26).contains(&value.terminal_font_size)
            || !valid_workspace_pins(&value.pinned_workspaces)
        {
            return Err("Invalid workspace presentation settings.");
        }
        Ok(Self {
            version: value.version,
            terminal_font_size: value.terminal_font_size,
            pinned_workspaces: value.pinned_workspaces,
        })
    }
}

/// The smallest and largest terminal text v2 accepts, in half CSS pixels.
const TERMINAL_FONT_HALF_PX_MIN: u8 = 20;
const TERMINAL_FONT_HALF_PX_MAX: u8 = 52;

/// Appearance surface v6. Terminal text runs from 10 to 26 CSS pixels in half
/// steps; v1 above keeps whole sizes from 12 and is what surfaces v3–v5 still
/// read and write. The size is held as a count of half pixels, so 11.5 is
/// exact and the type stays `Eq`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "WorkspacePresentationV2Wire")]
pub struct WorkspacePresentationV2 {
    terminal_font_half_px: u8,
    pinned_workspaces: Vec<String>,
}

impl Default for WorkspacePresentationV2 {
    /// 11.5px, the owner's terminal size (settled 2026-11-15).
    fn default() -> Self {
        Self {
            terminal_font_half_px: 23,
            pinned_workspaces: Vec::new(),
        }
    }
}

impl WorkspacePresentationV2 {
    /// What surfaces v3–v5 read: the next whole size, never under v1's 12.
    #[must_use]
    pub fn legacy(&self) -> WorkspacePresentation {
        WorkspacePresentation {
            version: 1,
            terminal_font_size: self.terminal_font_half_px.div_ceil(2).max(12),
            pinned_workspaces: self.pinned_workspaces.clone(),
        }
    }
}

impl From<WorkspacePresentation> for WorkspacePresentationV2 {
    fn from(value: WorkspacePresentation) -> Self {
        Self {
            terminal_font_half_px: value.terminal_font_size * 2,
            pinned_workspaces: value.pinned_workspaces,
        }
    }
}

impl Serialize for WorkspacePresentationV2 {
    /// A whole size is written as an integer, as JavaScript writes it.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut out = serializer.serialize_struct("WorkspacePresentationV2", 3)?;
        out.serialize_field("version", &2_u8)?;
        if self.terminal_font_half_px % 2 == 0 {
            out.serialize_field("terminalFontSize", &(self.terminal_font_half_px / 2))?;
        } else {
            out.serialize_field(
                "terminalFontSize",
                &(f64::from(self.terminal_font_half_px) / 2.0),
            )?;
        }
        out.serialize_field("pinnedWorkspaces", &self.pinned_workspaces)?;
        out.end()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspacePresentationV2Wire {
    version: u8,
    terminal_font_size: f64,
    pinned_workspaces: Vec<String>,
}

impl TryFrom<WorkspacePresentationV2Wire> for WorkspacePresentationV2 {
    type Error = &'static str;
    fn try_from(value: WorkspacePresentationV2Wire) -> Result<Self, Self::Error> {
        let doubled = value.terminal_font_size * 2.0;
        let half_px = (TERMINAL_FONT_HALF_PX_MIN..=TERMINAL_FONT_HALF_PX_MAX)
            .find(|half| (f64::from(*half) - doubled).abs() < f64::EPSILON);
        match half_px {
            Some(terminal_font_half_px)
                if value.version == 2 && valid_workspace_pins(&value.pinned_workspaces) =>
            {
                Ok(Self {
                    terminal_font_half_px,
                    pinned_workspaces: value.pinned_workspaces,
                })
            }
            _ => Err("Invalid workspace presentation settings."),
        }
    }
}

/// What a snapshot carries as `workspacePresentation`: v2 on appearance
/// surface v6, its v1 view on surfaces v3–v5.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WorkspacePresentationView {
    Current(WorkspacePresentationV2),
    Legacy(WorkspacePresentation),
}

/// Appearance surface v4. Project paths are display identities, never grants.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "ProjectPresentationWire", rename_all = "camelCase")]
pub struct ProjectPresentation {
    version: u8,
    pinned_projects: Vec<String>,
}

impl Default for ProjectPresentation {
    fn default() -> Self {
        Self {
            version: 1,
            pinned_projects: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectPresentationWire {
    version: u8,
    pinned_projects: Vec<String>,
}

impl TryFrom<ProjectPresentationWire> for ProjectPresentation {
    type Error = &'static str;
    fn try_from(value: ProjectPresentationWire) -> Result<Self, Self::Error> {
        if value.version != 1
            || value.pinned_projects.len() > 128
            || value.pinned_projects.iter().enumerate().any(|(i, path)| {
                !path.starts_with('/')
                    || path.len() > 4096
                    || path.chars().any(char::is_control)
                    || value.pinned_projects[..i].contains(path)
            })
        {
            return Err("Invalid project presentation settings.");
        }
        Ok(Self {
            version: value.version,
            pinned_projects: value.pinned_projects,
        })
    }
}

/// Opt-in v1 extension to the unchanged legacy appearance request/snapshot.
/// Percentages describe material strength: zero preserves the solid palette.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "SurfaceTranslucencyWire")]
pub struct SurfaceTranslucency {
    version: u8,
    terminal: u8,
    sidebar: u8,
}

impl Default for SurfaceTranslucency {
    fn default() -> Self {
        Self {
            version: 1,
            terminal: 0,
            sidebar: 0,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceTranslucencyWire {
    version: u8,
    terminal: u8,
    sidebar: u8,
}

impl TryFrom<SurfaceTranslucencyWire> for SurfaceTranslucency {
    type Error = &'static str;
    fn try_from(value: SurfaceTranslucencyWire) -> Result<Self, Self::Error> {
        if value.version != 1 || value.terminal > 100 || value.sidebar > 100 {
            return Err("Translucency requires version 1 and integer percentages from 0 to 100.");
        }
        Ok(Self {
            version: value.version,
            terminal: value.terminal,
            sidebar: value.sidebar,
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum AppearanceChange {
    Theme(ThemeId),
    SystemThemes(SystemThemes),
    Translucency(SurfaceTranslucency),
    PanePresentation(PanePresentation),
    WorkspacePresentation(WorkspacePresentation),
    WorkspacePresentationV2(WorkspacePresentationV2),
    ProjectPresentation(ProjectPresentation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseThemePreferenceError;

impl ThemePreference {
    /// The palette a preference paints today. Retired themes resolve to their
    /// replacement: both warm paper pairs to Clay (the `anthropic-*` ids) and
    /// Nightshade to Carbon (`grok-build`). Every other theme is itself.
    #[must_use]
    pub const fn canonical(self) -> Self {
        match self {
            Self::SauteedPaperLight | Self::WarmEditorialLight => Self::AnthropicLight,
            Self::SauteedPaperDark | Self::WarmEditorialDark => Self::AnthropicDark,
            Self::Nightshade => Self::GrokBuild,
            current => current,
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
            Self::SauteedPaperLight => "sauteed-paper-light",
            Self::SauteedPaperDark => "sauteed-paper-dark",
            Self::WarmEditorialLight => "warm-editorial-light",
            Self::WarmEditorialDark => "warm-editorial-dark",
            Self::AnthropicLight => "anthropic-light",
            Self::AnthropicDark => "anthropic-dark",
            Self::SolarizedLight => "solarized-light",
            Self::SolarizedDark => "solarized-dark",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::GrokBuild => "grok-build",
            Self::Nightshade => "nightshade",
            Self::Hermes => "hermes",
            Self::HermesLight => "hermes-light",
        }
    }
}

impl fmt::Display for ThemePreference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

impl FromStr for ThemePreference {
    type Err = ParseThemePreferenceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        THEME_PREFERENCES
            .into_iter()
            .find(|theme| theme.id() == value)
            .ok_or(ParseThemePreferenceError)
    }
}

#[must_use]
pub const fn resolve_theme_preference(
    preference: ThemePreference,
    system_dark: bool,
) -> ThemePreference {
    match preference.canonical() {
        ThemePreference::System if system_dark => ThemePreference::Dark,
        ThemePreference::System => ThemePreference::Light,
        resolved => resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_presentation_cross_language_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/project-presentation-v1.json"
        ))
        .unwrap();
        for value in fixture["accepted"].as_array().unwrap() {
            let decoded: ProjectPresentation = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
            assert!(matches!(
                serde_json::from_value::<AppearanceChange>(value.clone()).unwrap(),
                AppearanceChange::ProjectPresentation(_)
            ));
        }
        for value in fixture["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<ProjectPresentation>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn workspace_presentation_cross_language_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/workspace-presentation-v1.json"
        ))
        .unwrap();
        for value in fixture["accepted"].as_array().unwrap() {
            let decoded: WorkspacePresentation = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
            assert!(matches!(
                serde_json::from_value::<AppearanceChange>(value.clone()).unwrap(),
                AppearanceChange::WorkspacePresentation(_)
            ));
        }
        for value in fixture["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<WorkspacePresentation>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn workspace_presentation_v2_cross_language_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/workspace-presentation-v2.json"
        ))
        .unwrap();
        for value in fixture["accepted"].as_array().unwrap() {
            let decoded: WorkspacePresentationV2 = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(&decoded).unwrap(), *value);
            assert!(matches!(
                serde_json::from_value::<AppearanceChange>(value.clone()).unwrap(),
                AppearanceChange::WorkspacePresentationV2(_)
            ));
            assert_eq!(
                serde_json::from_value::<WorkspacePresentationView>(value.clone()).unwrap(),
                WorkspacePresentationView::Current(decoded)
            );
        }
        for value in fixture["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<WorkspacePresentationV2>(value.clone()).is_err(),
                "{value}"
            );
        }
        assert_eq!(
            serde_json::to_value(WorkspacePresentationV2::default()).unwrap(),
            fixture["default"]
        );
    }

    #[test]
    fn workspace_presentation_v2_reads_as_v1_on_older_surfaces_and_v1_upgrades() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/workspace-presentation-v2.json"
        ))
        .unwrap();
        for case in fixture["legacyViews"].as_array().unwrap() {
            let current: WorkspacePresentationV2 =
                serde_json::from_value(case["current"].clone()).unwrap();
            assert_eq!(
                serde_json::to_value(current.legacy()).unwrap(),
                case["legacy"],
                "{case}"
            );
        }
        let v1: WorkspacePresentation = serde_json::from_value(serde_json::json!({
            "version": 1, "terminalFontSize": 14, "pinnedWorkspaces": ["w-1"]
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(WorkspacePresentationV2::from(v1)).unwrap(),
            serde_json::json!({"version": 2, "terminalFontSize": 14, "pinnedWorkspaces": ["w-1"]})
        );
    }

    #[test]
    fn pane_presentation_cross_language_contract() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/pane-presentation-v1.json"
        ))
        .unwrap();
        for value in fixture["accepted"].as_array().unwrap() {
            let decoded: PanePresentation = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
            assert!(matches!(
                serde_json::from_value::<AppearanceChange>(value.clone()).unwrap(),
                AppearanceChange::PanePresentation(_)
            ));
        }
        for value in fixture["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<PanePresentation>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn surface_v1_cross_language_contract_and_legacy_snapshot() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../docs/fixtures/surface-translucency-v1.json"
        ))
        .unwrap();
        for value in fixture["accepted"].as_array().unwrap() {
            let decoded: SurfaceTranslucency = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
        }
        for value in fixture["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<SurfaceTranslucency>(value.clone()).is_err(),
                "{value}"
            );
        }
        assert_eq!(
            serde_json::to_value(AppearanceState::new(ThemePreference::Dark, false)).unwrap(),
            serde_json::json!({"preference":"dark","resolved":"dark"})
        );
    }

    #[test]
    fn persisted_identifiers_round_trip() {
        let identifiers = THEME_PREFERENCES.map(ThemePreference::id);
        assert_eq!(
            identifiers,
            [
                "system",
                "light",
                "dark",
                "sauteed-paper-light",
                "sauteed-paper-dark",
                "warm-editorial-light",
                "warm-editorial-dark",
                "anthropic-light",
                "anthropic-dark",
                "solarized-light",
                "solarized-dark",
                "catppuccin-mocha",
                "grok-build",
                "nightshade",
                "hermes",
                "hermes-light",
            ]
        );
        for theme in THEME_PREFERENCES {
            assert_eq!(theme.id().parse(), Ok(theme));
            assert_eq!(
                serde_json::to_string(&theme).unwrap(),
                format!("\"{}\"", theme.id())
            );
        }
        assert!("auto".parse::<ThemePreference>().is_err());
    }

    #[test]
    fn retired_themes_parse_and_resolve_to_their_replacements() {
        for (retired, replacement) in [
            ("sauteed-paper-light", ThemePreference::AnthropicLight),
            ("warm-editorial-light", ThemePreference::AnthropicLight),
            ("sauteed-paper-dark", ThemePreference::AnthropicDark),
            ("warm-editorial-dark", ThemePreference::AnthropicDark),
            ("nightshade", ThemePreference::GrokBuild),
        ] {
            let parsed: ThemePreference = retired.parse().unwrap();
            assert_eq!(parsed.canonical(), replacement, "{retired}");
            assert_eq!(
                resolve_theme_preference(parsed, true),
                replacement,
                "{retired}"
            );
            let state = AppearanceState::new(parsed, false);
            assert_eq!(
                (state.preference, state.resolved),
                (replacement.into(), replacement.into())
            );
        }
        for kept in THEME_PREFERENCES
            .into_iter()
            .filter(|theme| theme.canonical() == *theme)
        {
            assert_eq!(kept.canonical().canonical(), kept);
        }
    }

    #[test]
    fn custom_themes_cross_language_contract() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/fixtures/custom-themes-v1.json"))
                .unwrap();
        for value in fixture["themeIds"]["accepted"].as_array().unwrap() {
            let decoded: ThemeId = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(&decoded).unwrap(), *value);
            assert!(matches!(
                serde_json::from_value::<AppearanceChange>(value.clone()).unwrap(),
                AppearanceChange::Theme(_)
            ));
        }
        for value in fixture["themeIds"]["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<ThemeId>(value.clone()).is_err(),
                "{value}"
            );
        }
        for value in fixture["systemThemes"]["accepted"].as_array().unwrap() {
            let decoded: SystemThemes = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
            assert!(matches!(
                serde_json::from_value::<AppearanceChange>(value.clone()).unwrap(),
                AppearanceChange::SystemThemes(_)
            ));
        }
        for value in fixture["systemThemes"]["rejected"].as_array().unwrap() {
            assert!(
                serde_json::from_value::<SystemThemes>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn theme_files_resolve_and_fall_back_when_missing() {
        let moss = ThemeId::Custom(CustomThemeName::new("Moss").unwrap());
        let paper: ThemeId = "custom:Paper Lantern".parse().unwrap();
        let system: SystemThemes = serde_json::from_value(serde_json::json!({
            "version": 1, "light": "custom:Paper Lantern", "dark": "custom:Moss"
        }))
        .unwrap();
        let all = |_: &CustomThemeName| true;
        let none = |_: &CustomThemeName| false;
        let state = AppearanceState::resolve(moss.clone(), &system, false, all);
        assert_eq!(
            (state.preference, state.resolved),
            (moss.clone(), moss.clone())
        );
        let follow = ThemeId::Builtin(ThemePreference::System);
        assert_eq!(
            AppearanceState::resolve(follow.clone(), &system, false, all).resolved,
            paper
        );
        assert_eq!(
            AppearanceState::resolve(follow.clone(), &system, true, all).resolved,
            moss
        );
        // A missing file keeps the preference and paints what macOS would.
        let gone = AppearanceState::resolve(moss.clone(), &system, true, none);
        assert_eq!(gone.preference, moss);
        assert_eq!(gone.resolved, ThemeId::Builtin(ThemePreference::Dark));
        assert_eq!(
            AppearanceState::resolve(follow, &SystemThemes::default(), true, none).resolved,
            ThemeId::Builtin(ThemePreference::Dark)
        );
        assert_eq!(
            AppearanceState::resolve(
                ThemeId::Builtin(ThemePreference::Nightshade),
                &system,
                true,
                none
            )
            .resolved,
            ThemeId::Builtin(ThemePreference::GrokBuild)
        );
    }

    #[test]
    fn theme_file_names_are_single_plain_segments() {
        for good in ["Moss", "Paper Lantern", "solarized-ish 2", "café"] {
            assert_eq!(
                CustomThemeName::new(good).unwrap().file_name(),
                format!("{good}.css")
            );
        }
        for bad in [
            "",
            ".hidden",
            " padded",
            "a/b",
            "a\\b",
            "a:b",
            "tab\there",
            &"x".repeat(65),
        ] {
            assert!(CustomThemeName::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn system_resolves_while_named_themes_stay_pinned() {
        assert_eq!(
            resolve_theme_preference(ThemePreference::System, false),
            ThemePreference::Light
        );
        assert_eq!(
            resolve_theme_preference(ThemePreference::System, true),
            ThemePreference::Dark
        );
        assert_eq!(
            resolve_theme_preference(ThemePreference::CatppuccinMocha, false),
            ThemePreference::CatppuccinMocha
        );
    }
}
