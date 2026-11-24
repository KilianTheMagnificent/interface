export const THEME_PREFERENCES = [
  "system",
  "light",
  "dark",
  "anthropic-light",
  "anthropic-dark",
  "solarized-light",
  "solarized-dark",
  "catppuccin-mocha",
  "grok-build",
  "hermes",
  "hermes-light",
] as const;

export type ThemePreference = typeof THEME_PREFERENCES[number];

/**
 * Palettes retired on 2026-09-28 (owner) and the theme each became. The Rust
 * host accepts these identifiers and answers with the replacement; the
 * renderer maps its own remembered copies the same way.
 */
export const RETIRED_THEMES = {
  "sauteed-paper-light": "anthropic-light",
  "warm-editorial-light": "anthropic-light",
  "sauteed-paper-dark": "anthropic-dark",
  "warm-editorial-dark": "anthropic-dark",
  nightshade: "grok-build",
} as const satisfies Record<string, ThemePreference>;

/** A current theme for any remembered identifier, retired ones included. */
export function canonicalThemePreference(value: unknown): ThemePreference | null {
  if (isThemePreference(value)) return value;
  return typeof value === "string" && Object.hasOwn(RETIRED_THEMES, value)
    ? RETIRED_THEMES[value as keyof typeof RETIRED_THEMES] : null;
}

export type ResolvedTheme = Exclude<ThemePreference, "system">;
export type ThemeKind = "light" | "dark";
export type NativeThemeSource = "system" | ThemeKind;

/**
 * Appearance surface v5 (protocol v33): a theme the user keeps as
 * `~/.config/interface/themes/<name>.css`. Rust lists and reads the files;
 * the renderer applies the CSS as written.
 */
export const CUSTOM_THEME_PREFIX = "custom:";
export type CustomThemeId = `custom:${string}`;
/** What a preference or a Follow macOS slot names. */
export type ThemeId = ThemePreference | CustomThemeId;
/** What the window paints: never "system". */
export type ResolvedThemeId = ResolvedTheme | CustomThemeId;

/** The file name without `.css`: one plain path segment of 1–64 characters. */
export function isCustomThemeName(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const count = [...value].length;
  return count >= 1 && count <= 64 && !value.startsWith(".") && value.trim() === value
    && !/[\u0000-\u001f\u007f-\u009f\/\\:]/u.test(value);
}
export function isCustomThemeId(value: unknown): value is CustomThemeId {
  return typeof value === "string" && value.startsWith(CUSTOM_THEME_PREFIX)
    && isCustomThemeName(value.slice(CUSTOM_THEME_PREFIX.length));
}
export function isThemeId(value: unknown): value is ThemeId {
  return isThemePreference(value) || isCustomThemeId(value);
}
export function isResolvedThemeId(value: unknown): value is ResolvedThemeId {
  return isResolvedTheme(value) || isCustomThemeId(value);
}
export function customThemeName(id: CustomThemeId): string {
  return id.slice(CUSTOM_THEME_PREFIX.length);
}

/** One file in the themes folder, as Rust read it. */
export interface CustomThemeSummary {
  id: CustomThemeId;
  name: string;
  kind: ThemeKind;
  /** The window fill, when the file states one as a hex colour. */
  background?: string;
  /** Changes whenever the file's bytes do. */
  revision: string;
  /** Swatch colours the file declares, by custom property name. */
  preview: Record<string, string>;
  /** Why the file cannot be painted. */
  problem?: string;
}
export function isCustomThemeSummary(value: unknown): value is CustomThemeSummary {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  const allowed = new Set(["id", "name", "kind", "background", "revision", "preview", "problem"]);
  return Object.keys(v).every(key => allowed.has(key))
    && isCustomThemeId(v.id) && v.name === customThemeName(v.id)
    && (v.kind === "light" || v.kind === "dark")
    && (v.background === undefined || (typeof v.background === "string" && /^#[0-9a-f]{6}$/.test(v.background)))
    && typeof v.revision === "string" && v.revision.length > 0 && v.revision.length <= 64
    && typeof v.preview === "object" && v.preview !== null && !Array.isArray(v.preview)
    && Object.entries(v.preview as Record<string, unknown>).every(([key, colour]) =>
      // Rust caps a value at 128 characters; one character is at most two UTF-16 units.
      /^--[a-z0-9-]{1,64}$/.test(key) && typeof colour === "string" && colour.length <= 256)
    && (v.problem === undefined || typeof v.problem === "string");
}

/** The themes Follow macOS switches between. Either may name a theme file. */
export interface SystemThemes { version: 1; light: Exclude<ThemeId, "system">; dark: Exclude<ThemeId, "system"> }
export const DEFAULT_SYSTEM_THEMES: SystemThemes = { version: 1, light: "light", dark: "dark" };
export function isSystemThemes(value: unknown): value is SystemThemes {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  return Object.keys(v).length === 3 && v.version === 1
    && isThemeId(v.light) && v.light !== "system" && isThemeId(v.dark) && v.dark !== "system";
}

/** Versioned, Rust-owned material preferences. 0 is the existing solid look. */
export interface SurfaceTranslucency { version: 1; terminal: number; sidebar: number }
export const SOLID_SURFACES: SurfaceTranslucency = { version: 1, terminal: 0, sidebar: 0 };
export function isSurfaceTranslucency(value: unknown): value is SurfaceTranslucency {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  return Object.keys(v).length === 3 && v.version === 1
    && typeof v.terminal === "number" && Number.isInteger(v.terminal) && v.terminal >= 0 && v.terminal <= 100
    && typeof v.sidebar === "number" && Number.isInteger(v.sidebar) && v.sidebar >= 0 && v.sidebar <= 100;
}

/** Surface protocol v2. This changes presentation, never terminal/session identity. */
export interface PanePresentation { version: 1; paneStyle: "terminal" | "clean" }
export function isPanePresentation(value: unknown): value is PanePresentation {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  return Object.keys(v).length === 2 && v.version === 1
    && (v.paneStyle === "terminal" || v.paneStyle === "clean");
}
/** The smallest and largest terminal text size, in CSS pixels. */
export const TERMINAL_FONT_SIZE_MIN = 10;
export const TERMINAL_FONT_SIZE_MAX = 26;
/**
 * Appearance surface v6 (version 2): terminal text in half-pixel steps from 10
 * to 26. Rust persists it independently of legacy workspaces, and still serves
 * surfaces v3–v5 the whole-pixel version 1 this replaced.
 */
export interface WorkspacePresentation { version: 2; terminalFontSize: number; pinnedWorkspaces: string[] }
export const DEFAULT_WORKSPACE_PRESENTATION: WorkspacePresentation = {
  version: 2, terminalFontSize: 11.5, pinnedWorkspaces: [],
};
export function isWorkspacePresentation(value: unknown): value is WorkspacePresentation {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  return Object.keys(v).length === 3 && v.version === 2
    && typeof v.terminalFontSize === "number" && Number.isInteger(v.terminalFontSize * 2)
    && v.terminalFontSize >= TERMINAL_FONT_SIZE_MIN && v.terminalFontSize <= TERMINAL_FONT_SIZE_MAX
    && Array.isArray(v.pinnedWorkspaces) && v.pinnedWorkspaces.length <= 128
    && v.pinnedWorkspaces.every(id => typeof id === "string" && /^[a-zA-Z0-9_-]{1,128}$/.test(id))
    && new Set(v.pinnedWorkspaces).size === v.pinnedWorkspaces.length;
}
/** Appearance surface v4. Paths identify sidebar projects; they grant no file access. */
export interface ProjectPresentation { version: 1; pinnedProjects: string[] }
export function isProjectPresentation(value: unknown): value is ProjectPresentation {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const v = value as Record<string, unknown>;
  return Object.keys(v).length === 2 && v.version === 1
    && Array.isArray(v.pinnedProjects) && v.pinnedProjects.length <= 128
    && v.pinnedProjects.every(path => typeof path === "string" && path.startsWith("/")
      && new TextEncoder().encode(path).length <= 4096 && !/[\u0000-\u001f\u007f-\u009f\ud800-\udfff]/u.test(path))
    && new Set(v.pinnedProjects).size === v.pinnedProjects.length;
}
export type AppearanceChange = ThemeId | SystemThemes | SurfaceTranslucency | PanePresentation | WorkspacePresentation | ProjectPresentation;

export interface AppearanceState {
  preference: ThemeId;
  resolved: ResolvedThemeId;
  translucency?: SurfaceTranslucency;
  panePresentation?: PanePresentation;
  workspacePresentation?: WorkspacePresentation;
  projectPresentation?: ProjectPresentation;
  /** Surface v5: every file in the themes folder, by name. */
  customThemes?: CustomThemeSummary[];
  /** Surface v5: the pair Follow macOS switches between. */
  systemThemes?: SystemThemes;
  /** Surface v5: the CSS of `resolved` when it is a theme file. */
  themeCss?: string;
}

/**
 * Palettes designed as consoles. Bare CLI switches to Carbon (the grok-build
 * id) and restores the previous workspace theme when the harness comes back.
 */
export const CLI_THEME_PREFERENCES = ["grok-build"] as const;
export type CliThemePreference = typeof CLI_THEME_PREFERENCES[number];
export const DEFAULT_CLI_THEME: CliThemePreference = "grok-build";

export const THEME_KIND = {
  light: "light",
  dark: "dark",
  "anthropic-light": "light",
  "anthropic-dark": "dark",
  "solarized-light": "light",
  "solarized-dark": "dark",
  "catppuccin-mocha": "dark",
  "grok-build": "dark",
  hermes: "dark",
  "hermes-light": "light",
} as const satisfies Record<ResolvedTheme, ThemeKind>;

/** Main-process window fill. Mirrors each theme's --editor-bg in theme.css. */
export const WINDOW_BACKGROUND = {
  light: "#f0f0ee",
  dark: "#0e0e0d",
  "anthropic-light": "#faf9f5",
  "anthropic-dark": "#141413",
  "solarized-light": "#fdf6e3",
  "solarized-dark": "#002b36",
  "catppuccin-mocha": "#1e1e2e",
  "grok-build": "#0e0e0e",
  hermes: "#142762",
  "hermes-light": "#f8fafd",
} as const satisfies Record<ResolvedTheme, string>;

export const THEME_LABELS = {
  system: "System",
  light: "Porcelain",
  dark: "Porcelain Night",
  "anthropic-light": "Clay Light",
  "anthropic-dark": "Clay Dark",
  "solarized-light": "Solarized Light",
  "solarized-dark": "Solarized Dark",
  "catppuccin-mocha": "Catppuccin Mocha",
  "grok-build": "Carbon",
  hermes: "Navy",
  "hermes-light": "Ice",
} as const satisfies Record<ThemePreference, string>;

export const THEME_GROUPS = {
  system: "Interface",
  light: "Interface",
  dark: "Interface",
  "anthropic-light": "Clay",
  "anthropic-dark": "Clay",
  "solarized-light": "Solarized",
  "solarized-dark": "Solarized",
  "catppuccin-mocha": "Catppuccin",
  "grok-build": "Carbon",
  hermes: "Navy",
  "hermes-light": "Ice",
} as const satisfies Record<ThemePreference, string>;

export const THEME_DESCRIPTIONS = {
  system: "Follow the macOS appearance.",
  light: "Pale porcelain frames around dark screens.",
  dark: "Warm graphite frames around near-black screens.",
  "anthropic-light": "Ivory and slate with deepened clay accents.",
  "anthropic-dark": "Slate with ivory type and clay, blue and olive accents.",
  "solarized-light": "Ethan Schoonover's cream Solarized Light.",
  "solarized-dark": "Ethan Schoonover's base03 Solarized Dark.",
  "catppuccin-mocha": "Cool mauve Catppuccin Mocha.",
  "grok-build": "Flat carbon console with paper type and one ember accent.",
  hermes: "Deep navy with cream text and gold accents.",
  "hermes-light": "Cool white with Klein-blue accents.",
} as const satisfies Record<ThemePreference, string>;

export function isThemePreference(value: unknown): value is ThemePreference {
  return typeof value === "string"
    && (THEME_PREFERENCES as readonly string[]).includes(value);
}

export function isResolvedTheme(value: unknown): value is ResolvedTheme {
  return isThemePreference(value) && value !== "system";
}

export function isCliThemePreference(value: unknown): value is CliThemePreference {
  return typeof value === "string"
    && (CLI_THEME_PREFERENCES as readonly string[]).includes(value);
}

/** Remembered palettes for each chrome form, plus the theme to apply now. */
export interface ChromeThemePairing {
  theme: ThemeId;
  lastCliTheme: CliThemePreference;
  lastWorkspaceTheme: ThemeId;
}

/**
 * Pair Bare CLI with a console palette and Refined with the last workspace
 * theme. Current theme is captured into the slot it belongs to, then the
 * destination chrome's remembered palette is applied.
 */
export function pairThemeWithChrome(
  chrome: "refined" | "cli",
  currentTheme: ThemeId,
  lastCliTheme: CliThemePreference,
  lastWorkspaceTheme: ThemeId,
): ChromeThemePairing {
  const cli = isCliThemePreference(currentTheme) ? currentTheme : lastCliTheme;
  const workspace = isCliThemePreference(currentTheme) ? lastWorkspaceTheme : currentTheme;
  return chrome === "cli"
    ? { theme: cli, lastCliTheme: cli, lastWorkspaceTheme: workspace }
    : { theme: workspace, lastCliTheme: cli, lastWorkspaceTheme: workspace };
}

export function themeKind(theme: ResolvedTheme): ThemeKind {
  return THEME_KIND[theme];
}

/** Light or dark for any painted theme; a theme file says which it is. */
export function resolvedThemeKind(state: Pick<AppearanceState, "resolved" | "customThemes">): ThemeKind {
  if (!isCustomThemeId(state.resolved)) return themeKind(state.resolved);
  return state.customThemes?.find(theme => theme.id === state.resolved)?.kind ?? "dark";
}

/** The native window fill for any painted theme. */
export function windowBackground(state: Pick<AppearanceState, "resolved" | "customThemes">): string {
  if (!isCustomThemeId(state.resolved)) return WINDOW_BACKGROUND[state.resolved];
  const theme = state.customThemes?.find(candidate => candidate.id === state.resolved);
  return theme?.background ?? WINDOW_BACKGROUND[theme?.kind === "light" ? "light" : "dark"];
}

export function nativeThemeSource(preference: ThemePreference): NativeThemeSource {
  return preference === "system" ? "system" : themeKind(preference);
}

/** What macOS should draw native controls as: a chosen theme file says light
 * or dark; a missing one follows the system, as its fallback does. */
export function appearanceThemeSource(
  state: Pick<AppearanceState, "preference" | "customThemes">,
): NativeThemeSource {
  if (isThemePreference(state.preference)) return nativeThemeSource(state.preference);
  return state.customThemes?.find(theme =>
    theme.id === state.preference && theme.problem === undefined)?.kind ?? "system";
}

/** Main's own resolution between host reads, when macOS changes appearance:
 * the same rule the host applies (a missing or broken theme file paints the
 * built-in for the system's appearance). */
export function resolveAppearanceTheme(
  state: Pick<AppearanceState, "preference" | "systemThemes" | "customThemes">,
  systemDark: boolean,
): ResolvedThemeId {
  const usable = (theme: ThemeId): theme is ResolvedThemeId => theme !== "system"
    && (!isCustomThemeId(theme) || state.customThemes?.some(candidate =>
      candidate.id === theme && candidate.problem === undefined) === true);
  const preference = isThemePreference(state.preference)
    ? canonicalThemePreference(state.preference) ?? "system" : state.preference;
  if (usable(preference)) return preference;
  const pair = state.systemThemes ?? DEFAULT_SYSTEM_THEMES;
  const slot = systemDark ? pair.dark : pair.light;
  return usable(slot) ? slot : systemDark ? "dark" : "light";
}

export function resolveThemePreference(preference: ThemePreference,
    systemDark: boolean): ResolvedTheme {
  return preference === "system" ? (systemDark ? "dark" : "light") : preference;
}
