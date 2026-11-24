import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AppearanceState, ProjectPresentation, SurfaceTranslucency, SystemThemes, ThemeId, WorkspacePresentation,
} from "../shared/appearance";
import {
  SOLID_SURFACES, DEFAULT_WORKSPACE_PRESENTATION, customThemeName, isCustomThemeId,
  resolvedThemeKind,
} from "../shared/appearance";
import type {
  AppPreferences, PreferenceStorageReader, UiChromePreference, UiFontPreference,
} from "./settings";
import { readAppPreferences } from "./settings";

import {
  AppearanceController, INITIAL_APPEARANCE, type AppearanceApi, type AppearanceSnapshot,
} from "./appearance-controller";
import type { IfaceApi } from "../shared/ipc";
import { equalValue } from "./stable-values";
import "./translucency.css";
import "./theme-look.css";
import { applyThemeLook, applyThemeMotion } from "./theme-look";

export function applyUiFont(font: UiFontPreference): void {
  document.documentElement.dataset.font = font;
}

/**
 * The chrome FORM axis. Deliberately not IPC-backed like `data-theme`: CLI mode
 * moves no colour token, so the main process never needs the value to paint the
 * native window background before React mounts. That makes it a renderer
 * preference exactly like `data-font`.
 *
 * Settings is a product surface, not a console. While the lock is down the
 * attribute is forced to refined so the page and anything it portals never
 * inherit CLI geometry or MonoLisa. The stored preference is unchanged and is
 * reapplied when the lock lifts.
 */
let lastUiChrome: UiChromePreference = "refined";
let settingsChromeLocked = false;

function paintUiChrome(): void {
  document.documentElement.dataset.chrome = settingsChromeLocked ? "refined" : lastUiChrome;
}

export function applyUiChrome(chrome: UiChromePreference): void {
  lastUiChrome = chrome;
  paintUiChrome();
}

export function applySettingsChromeLock(locked: boolean): void {
  settingsChromeLocked = locked;
  paintUiChrome();
}

/** Every renderer-owned appearance attribute, painted from one preference object. */
export function applyAppearancePreferences(preferences: AppPreferences): void {
  applyUiFont(preferences.uiFont);
  applyUiChrome(preferences.uiChrome);
  applyThemeMotion(preferences.themeMotion ?? "on");
}

/**
 * Startup: read the durable preferences and paint the attributes they own in the
 * same breath. App's state initialiser is the only caller, and it calls THIS
 * rather than applying one attribute inline — a form or font preference that
 * only takes effect after you toggle it is the failure this seam exists to make
 * impossible to reintroduce.
 */
export function initializeAppPreferences(storage: PreferenceStorageReader): AppPreferences {
  const preferences = readAppPreferences(storage);
  applyAppearancePreferences(preferences);
  return preferences;
}

/** Paint background materials only; text, icons and terminal glyphs stay opaque. */
export function applySurfaceTranslucency(value: SurfaceTranslucency): void {
  const root = document.documentElement;
  root.dataset.glass = String(value.terminal > 0 || value.sidebar > 0);
  root.dataset.terminalGlass = String(value.terminal > 0);
  root.dataset.sidebarGlass = String(value.sidebar > 0);
  root.style.setProperty("--terminal-surface-opacity", `${100 - value.terminal * 0.78}%`);
  root.style.setProperty("--terminal-head-surface-opacity", `${100 - value.terminal * 0.58}%`);
  root.style.setProperty("--sidebar-surface-opacity", `${100 - value.sidebar * 0.85}%`);
}

/**
 * The appearance the document shows: main.tsx's pre-mount read, then every
 * publish. useAppearance starts from it, so a dark or palette launch is never
 * repainted light (with solid materials and the default text size) while its
 * confirming read is in flight. Unset only when the pre-mount read failed.
 */
let painted: AppearanceState | undefined;

/** What a terminal's xterm theme reads from appearance: the palette (a theme
 * file's revision included, so saving it repaints terminals), whether terminal
 * glass makes its background transparent, and its text size. */
function terminalThemeKey(state: AppearanceState): string {
  const fontSize = (state.workspacePresentation ?? DEFAULT_WORKSPACE_PRESENTATION).terminalFontSize;
  const revision = state.customThemes?.find(theme => theme.id === state.resolved)?.revision ?? "";
  return `${state.resolved}|${revision}|${(state.translucency?.terminal ?? 0) > 0}|${fontSize}`;
}

/** The painted theme file's CSS, one sheet for the life of the document. */
let themeFileSheet: CSSStyleSheet | HTMLStyleElement | undefined;
let themeFileCss: string | null = null;

/**
 * Applies a theme file's CSS as written, after every app stylesheet: an
 * adopted sheet follows all of the document's own sheets in the cascade, and
 * the app's tokens sit in the interface-palette layer, so a plain `:root`
 * rule in the file wins. `@import` is ignored in an adopted sheet, and the
 * page's CSP keeps images and fonts to the app and data: URLs. Where adopted
 * sheets are missing (a DOM shim) a trailing <style> element stands in.
 */
export function applyThemeFile(css: string | null): void {
  if (css === themeFileCss) return;
  themeFileCss = css;
  const text = css ?? "";
  if (themeFileSheet === undefined) {
    if (css === null) return;
    if (typeof CSSStyleSheet === "function" && "replaceSync" in CSSStyleSheet.prototype
        && Array.isArray(document.adoptedStyleSheets)) {
      themeFileSheet = new CSSStyleSheet();
      document.adoptedStyleSheets = [...document.adoptedStyleSheets, themeFileSheet];
    } else {
      themeFileSheet = document.createElement("style");
      themeFileSheet.id = "interface-theme-file";
      document.head.append(themeFileSheet);
    }
  }
  if (themeFileSheet instanceof HTMLStyleElement) {
    themeFileSheet.textContent = text;
    return;
  }
  try {
    themeFileSheet.replaceSync(text);
  } catch (error) {
    // A sheet that does not parse at all paints nothing rather than half.
    console.warn("[appearance] the theme file could not be applied:", error);
    themeFileSheet.replaceSync("");
  }
}

export function applyAppearance(state: AppearanceState): void {
  const previous = painted;
  // A stored legacy Clean pane style is still valid on the wire and changes
  // nothing here: panes always show the native CLI.
  applySurfaceTranslucency(state.translucency ?? SOLID_SURFACES);
  document.documentElement.style.setProperty("--workspace-terminal-font-size", String((state.workspacePresentation ?? DEFAULT_WORKSPACE_PRESENTATION).terminalFontSize));
  const kind = resolvedThemeKind(state);
  const root = document.documentElement;
  if (isCustomThemeId(state.resolved)) {
    // A theme file paints over the custom palette of its kind (theme.css).
    root.dataset.theme = `custom-${kind}`;
    root.dataset.themeFile = customThemeName(state.resolved);
    applyThemeFile(state.themeCss ?? "");
  } else {
    root.dataset.theme = state.resolved;
    delete root.dataset.themeFile;
    applyThemeFile(null);
  }
  // A file that sets a backdrop gets it under the window and glass over it;
  // set before terminals re-read their styles below.
  applyThemeLook(isCustomThemeId(state.resolved));
  document.documentElement.style.colorScheme = kind;
  painted = state;
  // Every mounted terminal re-reads its styles and resets its xterm theme on
  // this event, so it goes out only when something that theme reads changed:
  // pins, later slider positions and a confirming read repaint no terminal.
  if (previous !== undefined && terminalThemeKey(previous) === terminalThemeKey(state)) return;
  document.documentElement.dispatchEvent(new CustomEvent("interface-theme-change", {
    detail: state.resolved,
  }));
}

/** The hook's first snapshot: what the document already shows, awaiting its
 * confirming read, or the sentinel when nothing was painted before mount. */
export function firstAppearanceSnapshot(): AppearanceSnapshot {
  return painted === undefined ? INITIAL_APPEARANCE : { appearance: painted, pending: true, error: null };
}

/**
 * useAppearance's mount: a controller that starts from what the document
 * shows, paints every publish, and confirms the stored state with one read.
 * Its first publish therefore repaints exactly what is already on screen.
 */
export function startAppearance(api: AppearanceApi,
    onSnapshot: (snapshot: AppearanceSnapshot) => void): AppearanceController {
  // A publish equal to the last one (a confirming read, a colour-scheme change
  // that resolves the same) neither renders App nor restyles the document.
  let last: AppearanceSnapshot | undefined;
  const current = new AppearanceController(api, next => {
    if (last !== undefined && equalValue(last, next)) return;
    last = next;
    onSnapshot(next);
    applyAppearance(next.appearance);
  }, painted);
  current.refresh();
  return current;
}

/** What `useAppearance` calls; App passes `window.iface`. */
export type AppearanceHookApi = AppearanceApi & Partial<Pick<IfaceApi, "onAppearanceChanged">>;

export function useAppearance(api: AppearanceHookApi): {
  appearance: AppearanceState;
  pending: boolean;
  error: string | null;
  setPreference: (preference: ThemeId) => void;
  setSystemThemes: (value: SystemThemes) => void;
  setTranslucency: (value: SurfaceTranslucency) => void;
  setWorkspacePresentation: (value: WorkspacePresentation) => void;
  setProjectPresentation: (value: ProjectPresentation) => void;
} {
  const [snapshot, setSnapshot] = useState(firstAppearanceSnapshot);
  const controller = useRef<AppearanceController | null>(null);
  useEffect(() => {
    const current = startAppearance(api, setSnapshot);
    controller.current = current;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const systemChanged = (): void => current.refresh();
    media.addEventListener("change", systemChanged);
    // A theme file saved in another app repaints this window.
    const stopThemeFiles = api.onAppearanceChanged?.(() => current.refresh());
    return () => {
      current.dispose();
      controller.current = null;
      media.removeEventListener("change", systemChanged);
      stopThemeFiles?.();
    };
  }, [api]);
  const setPreference = useCallback((value: ThemeId) => controller.current?.change(value), []);
  const setSystemThemes = useCallback((value: SystemThemes) => controller.current?.change(value), []);
  const setTranslucency = useCallback((value: SurfaceTranslucency) => controller.current?.change(value), []);
  const setWorkspacePresentation = useCallback((value: WorkspacePresentation) => controller.current?.change(value), []);
  const setProjectPresentation = useCallback((value: ProjectPresentation) => controller.current?.change(value), []);
  return { ...snapshot, setPreference, setSystemThemes, setTranslucency, setWorkspacePresentation, setProjectPresentation };
}
