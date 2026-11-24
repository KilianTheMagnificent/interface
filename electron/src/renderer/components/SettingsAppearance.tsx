import { useRef, useState } from "react";
import type { CSSProperties, KeyboardEvent } from "react";
import { AppSelect, type AppSelectOption } from "./AppSelect";
import { ProductName } from "./ProductName";
import { TranslucencySlider } from "./TranslucencySlider";
import { SettingsGroup, SettingsRow, SettingsSwitch, useSettingsDisclosure } from "./SettingsControls";
import { applySurfaceTranslucency } from "../appearance";
import { productLabel } from "../product";
import {
  isUiChromePreference, type AppPreferencePatch, type AppPreferences,
} from "../settings";
import type {
  AppearanceState, CustomThemeSummary, ResolvedTheme, SurfaceTranslucency, SystemThemes, ThemeId,
  WorkspacePresentation,
} from "../../shared/appearance";
import {
  DEFAULT_SYSTEM_THEMES, DEFAULT_WORKSPACE_PRESENTATION, isResolvedTheme, TERMINAL_FONT_SIZE_MAX,
  TERMINAL_FONT_SIZE_MIN, THEME_DESCRIPTIONS, THEME_LABELS, THEME_PREFERENCES, themeKind,
} from "../../shared/appearance";
import { THEME_PREVIEWS, customThemeMoves, customThemePreview, type ThemePreview } from "../theme-previews";
import type { ThemeMotionPreference } from "../theme-look";

/** The theme picker's styles, part of `SETTINGS_PAGE_CSS`. They read the
 *  swatch colours this file sets on each swatch. */
export const SETTINGS_APPEARANCE_CSS = `
/* Every theme as a small window in its own colours. The swatch reads custom
   properties set from THEME_PREVIEWS, never the tokens of the palette
   currently on the document. The live backgrounds lead; the rest sit one tab
   at a time (owner, 2026-11-15). */
.settings-theme-block { display:flex; flex-direction:column; gap:10px; min-width:0; padding:12px 16px 14px; }
.settings-theme-block > .settings-row { padding:0; }
/* Follow macOS's two themes, in its row beside the switch. */
.settings-theme-pairing { display:inline-flex; flex-wrap:wrap; justify-content:flex-end; gap:8px; }
.settings-theme-tabs-row { display:flex; align-items:center; justify-content:space-between; gap:12px 24px; flex-wrap:wrap; }
.settings-tabs {
  display:inline-flex; gap:2px; padding:2px; background:var(--surface);
  border:var(--border-w) solid var(--border); border-radius:var(--radius-control);
}
.settings-tab {
  height:24px; padding:0 12px; color:var(--fg-dim); background:transparent; border:0;
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); cursor:pointer;
}
.settings-tab:hover { color:var(--fg-strong); }
.settings-tab[aria-selected="true"] { color:var(--fg-strong); background:var(--row-selected); font-weight:var(--wght-strong); }
.settings-theme-hint { margin:0; color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.settings-theme-hint[hidden] { display:none; }
.settings-theme-hint code { color:var(--fg-muted); font-family:var(--font-mono); font-size:var(--text-meta); }
.settings-theme-swatches { display:flex; flex-wrap:wrap; gap:12px; }
[role="tabpanel"][hidden] { display:none; }
.settings-theme-swatch {
  display:flex; flex-direction:column; gap:6px; width:110px; min-width:0; padding:0; color:var(--fg-muted);
  background:transparent; border:0; font:inherit; text-align:left; cursor:pointer;
}
.settings-theme-swatch:disabled { cursor:default; }
/* A theme file whose backdrop can move carries a play mark on its swatch, in
   the swatch's own colours so it reads on any theme. */
.settings-theme-moving.codicon {
  position:absolute; right:7px; bottom:6px; display:grid; place-items:center; width:18px; height:18px;
  color:var(--swatch-screen); background:var(--swatch-text); border-radius:50%; font-size:var(--text-meta);
}
.settings-theme-preview {
  position:relative; display:flex; flex-direction:column; height:60px; padding:0 6px 6px; overflow:hidden;
  background:var(--swatch-chrome); border-radius:var(--radius-container);
  box-shadow:0 0 0 var(--border-w) var(--border);
  transition:box-shadow var(--motion-fast) linear;
}
.settings-theme-swatch:hover:not(:disabled) .settings-theme-preview { box-shadow:0 0 0 var(--border-w) var(--border-strong); }
.settings-theme-swatch.is-selected .settings-theme-preview { box-shadow:0 0 0 2px var(--selection-accent); }
.settings-theme-preview-bar { align-self:flex-end; width:26px; height:5px; margin:6px 2px; border-radius:var(--radius-control); background:var(--swatch-accent); }
.settings-theme-preview-frame {
  display:flex; flex:1 1 auto; flex-direction:column; padding:0 3px 3px;
  background:var(--swatch-frame); border-radius:var(--radius-control);
  box-shadow:0 0 0 var(--border-w) color-mix(in srgb, var(--swatch-text) 14%, transparent);
}
.settings-theme-preview-lights { display:flex; align-items:center; gap:3px; height:10px; padding:0 3px; }
.settings-theme-preview-lights i { width:4px; height:4px; border-radius:50%; background:var(--swatch-working); }
.settings-theme-preview-lights i + i { background:var(--swatch-done); }
.settings-theme-preview-screen {
  display:flex; flex:1 1 auto; flex-direction:column; gap:3px; padding:5px;
  background:var(--swatch-screen); border-radius:var(--radius-control);
}
.settings-theme-preview-screen i { width:80%; height:2px; background:var(--swatch-text); border-radius:var(--radius-pill); }
.settings-theme-preview-screen i:nth-child(2) { width:55%; opacity:.55; }
.settings-theme-preview-screen i:nth-child(3) { width:35%; background:var(--swatch-working); }
.settings-theme-name { overflow:hidden; font-size:var(--text-ui); text-overflow:ellipsis; white-space:nowrap; }
.settings-theme-swatch.is-selected .settings-theme-name { color:var(--fg-strong); font-weight:var(--wght-strong); }
`;

const PICKER_THEMES: readonly ResolvedTheme[] = THEME_PREFERENCES.filter(isResolvedTheme);

/** Where theme files live, as Settings names it. */
export const THEMES_FOLDER_LABEL = "~/.config/interface/themes";
/** The picker's name for theme files whose backdrop moves. */
export const LIVE_THEMES_LABEL = "Live backgrounds";

/** The one Motion menu for every live background (owner, 2026-11-15), over
 *  the stored `themeMotion`: on, still on battery (`plugged`), off. */
export const THEME_MOTION_OPTIONS: readonly (AppSelectOption & { value: ThemeMotionPreference })[] = [
  { value: "on", label: "On",
    description: "Moves while Interface is in front. On battery it moves at half pace and rests after a minute without input." },
  { value: "plugged", label: "Still on battery",
    description: "Moves only while the Mac is plugged in." },
  { value: "off", label: "Off", description: "Always still. Saves the most battery." },
];

/** Terminal text sizes: half steps from 10px to 14px, where half a pixel
 *  shows, then whole pixels to 26px. */
export const TERMINAL_FONT_SIZES: readonly number[] = [
  ...Array.from({ length: 9 }, (_, i) => TERMINAL_FONT_SIZE_MIN + i / 2),
  ...Array.from({ length: TERMINAL_FONT_SIZE_MAX - 14 }, (_, i) => i + 15),
];
const TERMINAL_FONT_SIZE_OPTIONS: readonly AppSelectOption[] = TERMINAL_FONT_SIZES
  .map(size => ({ value: String(size), label: `${size} px` }));

/* Bare CLI is still the form axis (mono, square, no elevation). It also
   switches to a console palette; the pairing lives in App, not this picker. */
const CHROME_OPTIONS: readonly AppSelectOption[] = [
  { value: "refined", label: "Refined",
    description: "Rounded corners, soft shadows and the sans-serif ramp." },
  { value: "cli", label: "Bare CLI",
    description: "Flat terminal console: monospace, square corners, no shadows or blur. Switches to a console palette." },
];

/** The picker's tabs: one group of themes at a time. */
export type ThemeTab = "light" | "dark" | "yours";

/**
 * The tab that opens first: the one holding the selected theme. A live
 * background is not in a tab of its own, so its light or dark kind's tab
 * opens; a still theme file opens Your themes.
 */
export function initialThemeTab(selected: ThemeId, customThemes: readonly CustomThemeSummary[]): ThemeTab {
  if (isResolvedTheme(selected)) return themeKind(selected);
  const file = customThemes.find(theme => theme.id === selected);
  if (file === undefined) return "dark";
  return customThemeMoves(file) ? file.kind : "yours";
}

interface ThemeSwatchProps {
  id: ThemeId;
  label: string;
  title: string;
  preview: ThemePreview;
  checked: boolean;
  disabled: boolean;
  /** The theme paints a backdrop that can move: marked, so it can be found. */
  moving?: boolean;
  onPick: () => void;
}

function ThemeSwatch({ id, label, title, preview, checked, disabled, moving, onPick }: ThemeSwatchProps) {
  return <button key={id} type="button" role="radio" aria-checked={checked}
    className={`settings-theme-swatch${checked ? " is-selected" : ""}`}
    title={moving === true ? `${title}. Moving backdrop.` : title}
    aria-label={moving === true ? `${label}, moving backdrop` : undefined}
    disabled={disabled} onClick={onPick}
    style={{
      "--swatch-chrome": preview.chrome, "--swatch-frame": preview.frame,
      "--swatch-screen": preview.screen, "--swatch-text": preview.text,
      "--swatch-accent": preview.accent, "--swatch-working": preview.working,
      "--swatch-done": preview.done,
    } as CSSProperties}>
    <span className="settings-theme-preview" aria-hidden="true">
      <span className="settings-theme-preview-bar" />
      <span className="settings-theme-preview-frame">
        <span className="settings-theme-preview-lights"><i /><i /></span>
        <span className="settings-theme-preview-screen"><i /><i /><i /></span>
      </span>
      {moving === true && <span className="settings-theme-moving codicon codicon-play" />}
    </span>
    <span className="settings-theme-name">{label}</span>
  </button>;
}

/** Every theme Follow macOS can switch to, built-ins first, files after. */
function systemThemeOptions(customThemes: readonly CustomThemeSummary[]): readonly AppSelectOption[] {
  return [
    ...PICKER_THEMES.map(theme => ({ value: theme, label: THEME_LABELS[theme], group: "Built in" })),
    ...[true, false].flatMap(moving => customThemes
      .filter(theme => theme.problem === undefined && customThemeMoves(theme) === moving)
      .map(theme => ({ value: theme.id, label: theme.name, group: moving ? LIVE_THEMES_LABEL : "Your themes" }))),
  ];
}

export interface AppearanceSectionProps {
  id: string;
  hidden: boolean;
  appearance: AppearanceState;
  pending: boolean;
  error: string | null;
  onAppearanceChange: (preference: ThemeId) => void;
  onSystemThemesChange?: (value: SystemThemes) => void;
  onTranslucencyChange?: (value: SurfaceTranslucency) => void;
  onWorkspacePresentationChange?: (value: WorkspacePresentation) => void;
  preferences: AppPreferences;
  preferencesError: string | null;
  changePreferences: (patch: AppPreferencePatch) => void;
  onRevealDataFolder?: () => void;
}

/**
 * Settings › Appearance, as the owner's mockup draws it (2026-11-15): Follow
 * macOS with its two theme menus in its row; the live backgrounds, when a
 * theme file moves, with the one Motion menu; then the themes one tab at a
 * time; then text and style, and the window.
 */
export function AppearanceSection({
  id, hidden, appearance, pending, error, onAppearanceChange, onSystemThemesChange,
  onTranslucencyChange, onWorkspacePresentationChange, preferences, preferencesError,
  changePreferences, onRevealDataFolder,
}: AppearanceSectionProps) {
  const customThemes = appearance.customThemes ?? [];
  const following = appearance.preference === "system";
  const selected: ThemeId = following ? appearance.resolved : appearance.preference;
  const live = customThemes.filter(customThemeMoves);
  const files = customThemes.filter(theme => !customThemeMoves(theme));
  const [tab, setTab] = useState<ThemeTab>(() => initialThemeTab(selected, customThemes));
  const tabs: readonly { id: ThemeTab; label: string }[] = [
    { id: "light", label: "Light" },
    { id: "dark", label: "Dark" },
    ...files.length > 0 ? [{ id: "yours" as const, label: `Your themes · ${files.length}` }] : [],
  ];
  const shownTab = tabs.some(entry => entry.id === tab) ? tab : "dark";
  const tabRefs = useRef<Partial<Record<ThemeTab, HTMLButtonElement | null>>>({});
  const adding = useSettingsDisclosure();
  const pick = (theme: ThemeId, checked: boolean) => () => {
    if (!checked || following) onAppearanceChange(theme);
  };
  const fileSwatch = (theme: CustomThemeSummary) => {
    const checked = theme.id === selected;
    return <ThemeSwatch key={theme.id} id={theme.id} label={theme.name}
      title={theme.problem ?? `${theme.name}.css in ${THEMES_FOLDER_LABEL}`}
      preview={customThemePreview(theme)} checked={checked} moving={customThemeMoves(theme)}
      disabled={pending || theme.problem !== undefined} onPick={pick(theme.id, checked)} />;
  };
  const moveTab = (event: KeyboardEvent<HTMLDivElement>): void => {
    const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const index = tabs.findIndex(entry => entry.id === shownTab);
    const next = tabs[(index + step + tabs.length) % tabs.length]!.id;
    setTab(next);
    tabRefs.current[next]?.focus();
  };
  const pairing = appearance.systemThemes ?? DEFAULT_SYSTEM_THEMES;
  const motion = preferences.themeMotion ?? "on";

  return <section id={id} className="settings-section" hidden={hidden} aria-labelledby={`${id}-title`}>
    <div className="settings-section-heading">
      <div>
        <h2 id={`${id}-title`} className="settings-section-title">Appearance</h2>
        <p className="settings-section-copy">Colours, backgrounds and text size for the whole window.</p>
      </div>
    </div>

    <SettingsGroup label="Theme">
      <SettingsRow labelId={`${id}-theme`} label="Follow macOS light and dark"
        description="Switches between the two themes you pick.">
        {following && <span className="settings-theme-pairing" role="group" aria-label="Themes Follow macOS uses">
          {(["light", "dark"] as const).map(slot => <AppSelect key={slot}
            label={slot === "light" ? "Theme when macOS is light" : "Theme when macOS is dark"}
            prefix={slot === "light" ? "Light" : "Dark"}
            value={pairing[slot]} options={systemThemeOptions(customThemes)}
            disabled={pending || appearance.systemThemes === undefined || onSystemThemesChange === undefined}
            onChange={value => {
              if (appearance.systemThemes) onSystemThemesChange?.({
                ...appearance.systemThemes, [slot]: value as SystemThemes["light"],
              });
            }} />)}
        </span>}
        <SettingsSwitch labelId={`${id}-theme`} checked={following} disabled={pending} busy={pending}
          onChange={on => { onAppearanceChange(on ? "system" : appearance.resolved); }} />
      </SettingsRow>

      {/* Theme files whose backdrop moves come first, under their own name:
          the owner looked for "live backgrounds" twice (2026-11-15). */}
      {live.length > 0 && <div className="settings-theme-block">
        <SettingsRow labelId={`${id}-live`} label={LIVE_THEMES_LABEL}
          description={`A moving scene behind your panes. It rests while ${productLabel()} is in the background.`}>
          <AppSelect label="Motion" prefix="Motion" value={motion} options={THEME_MOTION_OPTIONS}
            onChange={value => {
              const next = THEME_MOTION_OPTIONS.find(option => option.value === value)?.value;
              if (next !== undefined) changePreferences({ themeMotion: next });
            }} />
        </SettingsRow>
        <div className="settings-theme-swatches" role="radiogroup" aria-label="Themes with a live background">
          {live.map(fileSwatch)}
        </div>
      </div>}

      <div className="settings-theme-block">
        <div className="settings-theme-tabs-row">
          <div className="settings-tabs" role="tablist" aria-label="Themes" onKeyDown={moveTab}>
            {tabs.map(entry => <button key={entry.id} type="button" role="tab"
              ref={element => { tabRefs.current[entry.id] = element; }}
              id={`${id}-tab-${entry.id}`} aria-controls={`${id}-panel-${entry.id}`}
              aria-selected={shownTab === entry.id} tabIndex={shownTab === entry.id ? 0 : -1}
              className="settings-tab" onClick={() => { setTab(entry.id); }}>{entry.label}</button>)}
          </div>
          {adding.link("Add your own", "Hide")}
        </div>
        {adding.panel(<>Save a theme as a <code>.css</code> file in <code>{THEMES_FOLDER_LABEL}</code> and
          it appears here.</>, "settings-theme-hint")}
        {(["light", "dark"] as const).map(kind => <div key={kind} role="tabpanel" id={`${id}-panel-${kind}`}
          aria-labelledby={`${id}-tab-${kind}`} hidden={shownTab !== kind}>
          <div className="settings-theme-swatches" role="radiogroup"
            aria-label={kind === "light" ? "Light themes" : "Dark themes"}>
            {PICKER_THEMES.filter(theme => themeKind(theme) === kind).map(theme => {
              const checked = theme === selected;
              return <ThemeSwatch key={theme} id={theme} label={THEME_LABELS[theme]}
                title={THEME_DESCRIPTIONS[theme]} preview={THEME_PREVIEWS[theme]} checked={checked}
                disabled={pending} onPick={pick(theme, checked)} />;
            })}
          </div>
        </div>)}
        {files.length > 0 && <div role="tabpanel" id={`${id}-panel-yours`}
          aria-labelledby={`${id}-tab-yours`} hidden={shownTab !== "yours"}>
          <div className="settings-theme-swatches" role="radiogroup" aria-label="Your theme files">
            {files.map(fileSwatch)}
          </div>
        </div>}
      </div>
    </SettingsGroup>

    <SettingsGroup label="Text and style">
      <SettingsRow labelId={`${id}-terminal-size`} label="Terminal text size"
        description="Applies to every terminal straight away.">
        <AppSelect label="Terminal text size" value={String((appearance.workspacePresentation ?? DEFAULT_WORKSPACE_PRESENTATION).terminalFontSize)}
          options={TERMINAL_FONT_SIZE_OPTIONS}
          disabled={appearance.workspacePresentation === undefined || onWorkspacePresentationChange === undefined}
          onChange={value => {
            if (appearance.workspacePresentation) onWorkspacePresentationChange?.({
              ...appearance.workspacePresentation, terminalFontSize: Number(value),
            });
          }} />
      </SettingsRow>
      <SettingsRow labelId={`${id}-chrome`} label={<ProductName suffix=" style" />}
        description="Refined is the standard look. Bare CLI is a flat, all-monospace console in Carbon.">
        <span className="settings-control-group" role="group"
          aria-labelledby={`${id}-chrome`} aria-describedby={`${id}-chrome-description`}>
          <AppSelect label={`${productLabel()} style`} value={preferences.uiChrome} options={CHROME_OPTIONS}
            onChange={value => {
              if (isUiChromePreference(value)) changePreferences({ uiChrome: value });
            }} />
        </span>
      </SettingsRow>
      <SettingsRow labelId={`${id}-font`} label="Serif replies"
        description="Show the assistant's replies in a serif typeface.">
        <SettingsSwitch labelId={`${id}-font`} checked={preferences.uiFont === "serif"}
          onChange={on => { changePreferences({ uiFont: on ? "serif" : "sans" }); }} />
      </SettingsRow>
    </SettingsGroup>

    <SettingsGroup label="Window">
      <SettingsRow labelId={`${id}-translucency`} label="See-through panes"
        description="Lets the desktop show softly behind. Text stays crisp.">
        <div className="translucency-controls">
          <TranslucencySlider id={`${id}-terminal-translucency`} label="Terminals"
            value={appearance.translucency?.terminal ?? 0}
            disabled={appearance.translucency === undefined || onTranslucencyChange === undefined}
            onPreview={terminal => {
              if (appearance.translucency) applySurfaceTranslucency({ ...appearance.translucency, terminal });
            }}
            onChange={terminal => {
              if (appearance.translucency) onTranslucencyChange?.({ ...appearance.translucency, terminal });
            }} />
          <TranslucencySlider id={`${id}-sidebar-translucency`} label="Sidebar"
            value={appearance.translucency?.sidebar ?? 0}
            disabled={appearance.translucency === undefined || onTranslucencyChange === undefined}
            onPreview={sidebar => {
              if (appearance.translucency) applySurfaceTranslucency({ ...appearance.translucency, sidebar });
            }}
            onChange={sidebar => {
              if (appearance.translucency) onTranslucencyChange?.({ ...appearance.translucency, sidebar });
            }} />
        </div>
      </SettingsRow>
    </SettingsGroup>

    <p className="settings-foot">
      Stored on this Mac.
      {onRevealDataFolder !== undefined && <>
        {" "}<button type="button" className="settings-link" onClick={onRevealDataFolder}>Reveal data folder</button>
      </>}
    </p>
    {error !== null && <p className="settings-error" role="alert">Appearance could not be updated: {error}</p>}
    {preferencesError !== null && <p className="settings-error" role="alert">{preferencesError}</p>}
  </section>;
}
