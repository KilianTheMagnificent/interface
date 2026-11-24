import type { CustomThemeSummary, ResolvedTheme, ThemeKind } from "../shared/appearance";

/**
 * What each theme's swatch in Settings draws: its chrome, a pane frame and
 * screen with a line of screen text, the accent and the working and done
 * lights. Settings shows every theme at once while only one palette is on the
 * document, so the swatches cannot read the tokens; theme-previews.test.ts
 * holds each value to its token in theme.css.
 */
export interface ThemePreview {
  chrome: string; frame: string; screen: string; text: string;
  accent: string; working: string; done: string;
}

export const THEME_PREVIEWS: Readonly<Record<ResolvedTheme, ThemePreview>> = {
  light: { chrome: "#f0f0ee", frame: "#f7f7f5", screen: "#18191b", text: "#d5d7da", accent: "#1d1e20", working: "#ad4a16", done: "#2f5fcf" },
  dark: { chrome: "#0e0e0d", frame: "#151514", screen: "#0a0a09", text: "#e4e2de", accent: "#edebe8", working: "#ff8a4c", done: "#7aa7ff" },
  "anthropic-light": { chrome: "#f0eee6", frame: "#e9e5d9", screen: "#fdfcf9", text: "#141413", accent: "#a24425", working: "#a24425", done: "#2f5fb8" },
  "anthropic-dark": { chrome: "#1a1918", frame: "#232220", screen: "#121211", text: "#ebe9e1", accent: "#d97757", working: "#d97757", done: "#7ea6e0" },
  "solarized-light": { chrome: "#eee8d5", frame: "#ebe4cf", screen: "#fdf6e3", text: "#50646a", accent: "#18608a", working: "#11645d", done: "#1b6aa8" },
  "solarized-dark": { chrome: "#073642", frame: "#0b3a45", screen: "#002731", text: "#9eabab", accent: "#7ec8ef", working: "#4cc2b4", done: "#5aaef0" },
  "catppuccin-mocha": { chrome: "#181825", frame: "#272738", screen: "#1b1b29", text: "#cdd6f4", accent: "#ceacf8", working: "#cca8f7", done: "#89b4fa" },
  "grok-build": { chrome: "#1a1a1a", frame: "#1a1919", screen: "#0d0d0d", text: "#e8e2d6", accent: "#f0761f", working: "#f0761f", done: "#6ea8ff" },
  hermes: { chrome: "#12276b", frame: "#1f3168", screen: "#122358", text: "#eddcc8", accent: "#e9c46a", working: "#d5c6ba", done: "#9cc3ff" },
  "hermes-light": { chrome: "#f4f7fe", frame: "#e6eaf3", screen: "#fbfcfe", text: "#242427", accent: "#2451f2", working: "#a8490f", done: "#2451f2" },
};

/** The token each preview field mirrors, for the test and for readers. */
export const THEME_PREVIEW_TOKENS: Readonly<Record<keyof ThemePreview, string>> = {
  chrome: "--chrome-bg", frame: "--terminal-header-bg", screen: "--terminal-content-bg", text: "--terminal-fg", accent: "--accent", working: "--status-working", done: "--status-done",
};

/** What an empty theme file of each kind paints (the custom palettes in theme.css). */
const THEME_FILE_DEFAULTS: Readonly<Record<ThemeKind, ThemePreview>> = {
  dark: { chrome: "#0e0e0d", frame: "#151513", screen: "#0a0a09", text: "#e4e2de", accent: "#edebe8", working: "#ff8a4c", done: "#7aa7ff" },
  light: { chrome: "#f0f0ee", frame: "#e7e7e5", screen: "#fbfbfa", text: "#2a2b2e", accent: "#1d1e20", working: "#ad4a16", done: "#2f5fcf" },
};

/** Where each swatch part looks in a theme file, most specific first. */
const THEME_FILE_PREVIEW_SOURCES: Readonly<Record<keyof ThemePreview, readonly string[]>> = {
  chrome: ["--chrome-bg", "--theme-panel", "--theme-background", "--editor-bg"],
  frame: ["--terminal-header-bg", "--theme-panel", "--chrome-bg", "--theme-background"],
  screen: ["--terminal-content-bg", "--theme-screen"],
  text: ["--terminal-fg", "--theme-screen-text", "--theme-text"],
  accent: ["--accent", "--theme-accent"],
  working: ["--status-working"],
  done: ["--status-done"],
};

function cssColour(value: string, declared: Readonly<Record<string, string>>, depth = 0): string | null {
  const reference = /^var\(\s*(--[a-z0-9-]+)\s*(?:,\s*(.+))?\)$/i.exec(value.trim());
  if (reference !== null) {
    if (depth > 4) return null;
    const target = declared[reference[1]!];
    if (target !== undefined) return cssColour(target, declared, depth + 1);
    return reference[2] === undefined ? null : cssColour(reference[2], declared, depth + 1);
  }
  if (/^#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.test(value)) return value;
  if (/[;{}<>]|url\(/i.test(value)) return null;
  return typeof CSS !== "undefined" && typeof CSS.supports === "function" && CSS.supports("color", value)
    ? value : null;
}

/**
 * Whether a theme file paints a backdrop that can move: the host says
 * `--theme-look: moving` for a file that sets a scene or a shader (owner,
 * 2026-11-15: "I'm not seeing where the option for the live/active background
 * themes are"). Settings marks those swatches.
 */
export function customThemeMoves(theme: Pick<CustomThemeSummary, "preview">): boolean {
  return theme.preview["--theme-look"] === "moving";
}

/** A theme file's swatch: the colours it declares, else its kind's defaults.
 * Settings shows it beside the built-ins without painting the file. */
export function customThemePreview(theme: Pick<CustomThemeSummary, "kind" | "preview">): ThemePreview {
  const defaults = THEME_FILE_DEFAULTS[theme.kind];
  const pick = (part: keyof ThemePreview): string => {
    for (const property of THEME_FILE_PREVIEW_SOURCES[part]) {
      const value = theme.preview[property];
      const colour = value === undefined ? null : cssColour(value, theme.preview);
      if (colour !== null) return colour;
    }
    return defaults[part];
  };
  return {
    chrome: pick("chrome"), frame: pick("frame"), screen: pick("screen"), text: pick("text"),
    accent: pick("accent"), working: pick("working"), done: pick("done"),
  };
}
