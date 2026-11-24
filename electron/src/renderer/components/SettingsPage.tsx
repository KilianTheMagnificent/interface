import { AppearanceSection, SETTINGS_APPEARANCE_CSS } from "./SettingsAppearance";
import { ProvidersSection } from "./SettingsProviders";
import { SettingsGroup, SettingsRow, SettingsSwitch, useSettingsDisclosure } from "./SettingsControls";
import { RemoteAccessPanel } from "./RemoteAccessPanel";
import { PrivacySection } from "./SettingsPrivacy";
import { UPDATE_LOG_PANEL_CSS } from "./UpdateLogPanelCss";
import type { RemoteAdministrationRequest, RemoteAdministrationStatus } from "../../shared/remote";
import { lazy, Suspense, useEffect, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import type {
  AppearanceState, SurfaceTranslucency, SystemThemes, ThemeId, WorkspacePresentation,
} from "../../shared/appearance";
import type {
  CustomizationEntry, CustomizationInventory, CustomizationProvider, ProviderCustomization, EngineProbe, IfaceApi, TerminalLaunchProfile,
} from "../../shared/ipc";
import type { ThreadSettings } from "../../shared/threads";
import type { EngineId } from "../../shared/engine";
import type { UpdateState } from "../../shared/updater";
import {
  normalizeAppPreferences,
  summarizeProviderProbes,
  summarizeTerminalTools,
  summarizeUpdateState,
  updateAppPreferences,
  type UpdateSettingsSummary,
  type AppPreferencePatch,
  type AppPreferences,
} from "../settings";
import { APPROVAL_ALLOW_KEYS, APPROVAL_DENY_KEYS } from "../approval-keys";
import { productLabel } from "../product";
import { withoutIpcErrorShape } from "../ipc-message";
import { homeRelativePath } from "../display-paths";
import { AppSelect, type AppSelectOption } from "./AppSelect";
import type { AcpProfileView } from "./AcpProfilesEditor";
import { InstallHint } from "./InstallHint";
import {
  OUTSIDE_AGENT_OPTIONS, OUTSIDE_AGENT_SETUP, OUTSIDE_AGENTS_DESCRIPTION, outsideAgentsLevel,
} from "../outside-agents";
import type { ProviderAccountsCommand } from "../../shared/provider-accounts";
import type { ProviderAccountsState } from "../use-provider-accounts";
import { CUSTOMIZATION_GROUPS, CUSTOMIZATION_SCOPE_LABELS } from "../task-inspector-view";
import { ThreadSchedulesSettings } from "./ThreadSchedulesSettings";
import { ThreadSecretsSettings } from "./ThreadSecretsSettings";
import type { HeldSecretRow } from "../thread-secrets";
import type { ScheduleSettingsRow } from "../thread-schedules";
import "../codicons.css";
import "../theme.css";

// The update history is about 500 KB of text that only Settings -> Updates
// reads, so it loads in its own chunk the first time that section opens.
const UpdateLogPanel = lazy(() => import("./UpdateLogPanel")
  .then(module => ({ default: module.UpdateLogPanel })));
const WhatsNew = lazy(() => import("./UpdateLogPanel")
  .then(module => ({ default: module.WhatsNew })));

export interface SettingsPageProps {
  /** The section to open on; absent opens Updates when one is ready, else Appearance. */
  initialSection?: SettingsSectionId;
  onRemoteAccess?: (request: RemoteAdministrationRequest) => Promise<RemoteAdministrationStatus>;
  /** Appearance stays IPC-backed and controlled by App/useAppearance. */
  appearance: AppearanceState;
  appearancePending?: boolean;
  appearanceError?: string | null;
  onAppearanceChange: (preference: ThemeId) => void;
  /** The light and dark theme Follow macOS switches between. */
  onSystemThemesChange?: (value: SystemThemes) => void;
  onTranslucencyChange?: (value: SurfaceTranslucency) => void;
  onWorkspacePresentationChange?: (value: WorkspacePresentation) => void;

  /** App owns persistence so editor and composer defaults update immediately. */
  preferences: AppPreferences;
  preferencesError?: string | null;
  onPreferencesChange: (preferences: AppPreferences) => void;

  /** Live provider probes; no provider status is inferred or hardcoded here. */
  engineHealth: readonly EngineProbe[];
  providersRefreshing?: boolean;
  providerRefreshError?: string | null;
  onRefreshProviders: () => void;
  acpProfiles?: readonly AcpProfileView[];
  acpProfilesPending?: boolean;
  acpProfilesError?: string | null;
  acpProfilesRestartRequired?: boolean;
  acpProfilesSavedRevision?: number;
  onSaveAcpProfiles?: (profiles: AcpProfileView[]) => void;

  /**
   * Which project the customization inventory describes: the focused session's
   * cwd when there is one. Falls back to the default directory, so the section
   * still answers for something when no session is selected.
   */
  customizationRoot?: string;
  /** The inventory's two reads. App passes its IPC API; the page makes no IPC call itself. */
  customizationApi: CustomizationApi;

  /**
   * Rust's thread settings (`threads.view.settings`), null until the first
   * read answers. Threads is absent from the rail without a change handler.
   */
  threadSettings?: ThreadSettings | null;
  threadSettingsPending?: boolean;
  threadSettingsError?: string | null;
  /** The whole settings object, as `setSettings` takes it. */
  onThreadSettingsChange?: (settings: ThreadSettings) => void;
  /**
   * Protocol 67: the secrets Interface holds for agents, by name and thread,
   * with Forget; `onShown` reads them when the Threads section opens.
   */
  threadSecrets?: {
    rows: readonly HeldSecretRow[];
    pending: boolean;
    error: string | null;
    onShown: () => void;
    onForget: (row: HeldSecretRow) => void;
  };
  /** Protocol 62: every thread's scheduled prompts and watch, and their switches. */
  threadSchedules?: {
    rows: readonly ScheduleSettingsRow[];
    pending: boolean;
    error: string | null;
    onEnable: (row: ScheduleSettingsRow, enabled: boolean) => void;
    onDelete: (row: ScheduleSettingsRow) => void;
  };

  /** Opens a signed-out provider's own sign-in in a Code terminal. */
  onSignIn?: (engine: EngineId) => void;
  /** Protocol 60: each provider's accounts; the section is absent without it. */
  providerAccounts?: ProviderAccountsState;
  onProviderAccountsChange?: (command: ProviderAccountsCommand) => void;
  /** Opens one account's own sign-in in a Code terminal. */
  onAccountSignIn?: (engine: string, account: string) => void;
  /** Models & providers was shown: App reads the accounts and checks each sign-in. */
  onProvidersShown?: () => void;
  /** Every launch profile with Rust's details (protocol 43); null until read. */
  terminalTools?: readonly TerminalLaunchProfile[] | null;
  /** Protocol 48: switches one CLI's use of Interface's tools in its own config. */
  onPaneToolsChange?: (profileId: TerminalLaunchProfile["id"], enabled: boolean) => void;
  /** The profile whose switch Rust is changing now, or null. */
  paneToolsPending?: TerminalLaunchProfile["id"] | null;
  paneToolsError?: string | null;
  /** Shows the data folder (stores, settings, host.log) in Finder. */
  onRevealDataFolder?: () => void;

  /** Controlled updater state. Omit it when this build has no updater bridge. */
  updateState?: UpdateState | null;
  onCheckForUpdates?: () => void;
  onRestartToUpdate?: () => void;

  onClose: () => void;
}

export interface SettingsShortcut {
  label: string;
  keys: readonly string[];
  description: string;
}

/** Kept beside the page so the visible reference can be audited against App's handlers. */
export const SETTINGS_SHORTCUTS: readonly SettingsShortcut[] = [
  { label: "New thread, workspace, or task", keys: ["⌘", "N"], description: "Start new work on the current surface." },
  { label: "Settings", keys: ["⌘", ","], description: "Open or close this settings page." },
  { label: "Command palette", keys: ["⌘", "⇧", "P"], description: `Find ${productLabel()} commands. Works from the composer.` },
  { label: "Filter sidebar", keys: ["⌘", "K"], description: "Filter threads, workspaces, or tasks on the current surface." },
  { label: "Next pane", keys: ["⌃", "Tab"], description: "Move focus to the next pane; a focused terminal pane keeps this chord for the shell." },
  { label: "Previous pane", keys: ["⌃", "⇧", "Tab"], description: "Move focus to the previous pane; a focused terminal pane keeps this chord for the shell." },
  { label: "Cycle panes", keys: ["⌘", "⌥", "←/→"], description: "Move focus to the previous or next pane." },
  { label: "Maximize tile", keys: ["⌘", "⇧", "M"], description: "Maximize or restore the focused tile." },
  { label: "Show terminals", keys: ["⌃", "`"], description: "Switch to the terminal workspace." },
  { label: "Split terminal left/right", keys: ["⌘", "D"], description: "Open a shell beside the focused terminal pane. In Threads, ⌘D shows the diff panel instead." },
  { label: "Split terminal top/bottom", keys: ["⌘", "⇧", "D"], description: "Open a shell below the focused terminal pane." },
  { label: "New terminal pane", keys: ["⌘", "↩"], description: "Open a shell beside the focused terminal pane, on the side with more room. In the launcher, ⌘↩ launches." },
  { label: "Close pane", keys: ["⌘", "W"], description: "Close the selected pane: a terminal ends, a thread's pane closes and the thread keeps running. With Settings open, closes Settings." },
  { label: "Close window", keys: ["⇧", "⌘", "W"], description: "Close the window to the Dock. Agents keep working." },
  { label: "Previous terminal", keys: ["⌘", "⇧", "["], description: "Move focus to the previous terminal pane." },
  { label: "Next terminal", keys: ["⌘", "⇧", "]"], description: "Move focus to the next terminal pane." },
  { label: "Previous thread", keys: ["⌘", "⇧", "["], description: "Open the thread above in the Threads sidebar. In a terminal pane this chord moves between terminals." },
  { label: "Next thread", keys: ["⌘", "⇧", "]"], description: "Open the thread below in the Threads sidebar." },
  { label: "Settle thread", keys: ["⌘", "⇧", "S"], description: "Settle the open thread, or bring a settled one back." },
  { label: "Pin thread", keys: ["⌘", "⌥", "P"], description: "Pin or unpin the open thread. ⌘⇧P stays the command palette." },
  { label: "Thread changes", keys: ["⌘", "D"], description: "In Threads, show or hide the open thread's diff panel. In Code, ⌘D splits the terminal." },
  { label: "Undo in Threads", keys: ["⌘", "Z"], description: "Undo the last settle, snooze or archive while the sidebar has focus." },
  { label: "Preview session", keys: ["Space"], description: "Show the hover card for the focused sidebar row. Space again or Escape hides it." },
  { label: "Find in conversation", keys: ["⌘", "F"], description: "Search the focused transcript." },
  { label: "Send prompt", keys: ["↩"], description: "Send from a prompt or steering editor." },
  { label: "New line", keys: ["⇧", "↩"], description: "Insert a line break without sending." },
  { label: "Allow request", keys: [...APPROVAL_ALLOW_KEYS], description: "Answer the blocking approval in the focused conversation. Never while you are typing." },
  { label: "Deny request", keys: [...APPROVAL_DENY_KEYS], description: "Refuse it. In a conversation this opens the reason field first, exactly as the button does." },
];

/** The shortcut reference's groups, as runs of SETTINGS_SHORTCUTS. */
const SHORTCUT_GROUPS: readonly { label: string; from: number; to: number }[] = [
  { label: "Anywhere", from: 0, to: 4 },
  { label: "Panes and terminals", from: 4, to: 16 },
  { label: "Threads", from: 16, to: 23 },
  { label: "Writing and answering", from: 23, to: 28 },
];

/** The reads behind the Agent customization section. */
export type CustomizationApi = Pick<IfaceApi, "getDefaultDirectory" | "listCustomization">
  & Partial<Pick<IfaceApi, "customizationControl">>;

/** Where one customization read reports. Nothing is reported once it is cancelled. */
export interface CustomizationReadSink {
  /** The folder being read, known before its inventory arrives. */
  root(label: string): void;
  inventory(inventory: CustomizationInventory): void;
  provider?(inventory: ProviderCustomization): void;
  failed(message: string): void;
  /** The read is over, whichever way it went. */
  settled(): void;
}

/**
 * Read the inventory for `requestedRoot`, or for the default directory when
 * none is given, reporting each step to `sink` until `cancelled()` says the
 * page has moved on.
 */
export async function readCustomizationInventory(api: CustomizationApi,
    requestedRoot: string | undefined, cancelled: () => boolean,
    sink: CustomizationReadSink, provider: CustomizationProvider = "claude"): Promise<void> {
  try {
    const root = requestedRoot !== undefined && requestedRoot.length > 0
      ? requestedRoot : await api.getDefaultDirectory();
    if (cancelled()) return;
    sink.root(root);
    if (api.customizationControl !== undefined) {
      const result = await api.customizationControl({ kind: "read", root, provider });
      if (cancelled()) return;
      sink.provider?.(result);
      sink.inventory(result.inventory);
    } else {
      const inventory = await api.listCustomization(root);
      if (cancelled()) return;
      sink.inventory(inventory);
    }
  } catch (err) {
    if (cancelled()) return;
    sink.failed(err instanceof Error ? withoutIpcErrorShape(err.message) : String(err));
  } finally {
    if (!cancelled()) sink.settled();
  }
}

export type SettingsPageKeyAction = "close";
export type SettingsSectionId =
  "general" | "providers" | "customization" | "threads" | "notifications" | "updates" | "privacy"
  | "remote" | "shortcuts";

/** The menu in DOM order. One list, so the menu and the pages cannot drift.
 *  Words only (owner, 2026-11-15): the menu takes the sidebar's place while
 *  Settings is open, and reads as a list of pages, not a toolbar of icons. */
const RAIL: readonly { id: SettingsSectionId; label: string }[] = [
  { id: "general", label: "Appearance" },
  { id: "providers", label: "Models & providers" },
  { id: "customization", label: "Agent customization" },
  { id: "threads", label: "Threads" },
  { id: "notifications", label: "Notifications" },
  { id: "updates", label: "Updates" },
  { id: "privacy", label: "Privacy" },
  { id: "remote", label: "iPhone remote access" },
  { id: "shortcuts", label: "Shortcuts" },
];

/** Settings › Notifications, in reading order: what to be told about, then how. */
export const NOTIFICATION_ROWS: readonly {
  key: "notifyFinished" | "notifyNeedsInput" | "notifySound"; label: string; description: string;
}[] = [
  { key: "notifyFinished", label: "When a run finishes",
    description: "A turn, task or terminal agent you are not watching finished or failed." },
  { key: "notifyNeedsInput", label: "When an agent needs input",
    description: "An agent you are not watching asks a question or waits for an approval." },
  { key: "notifySound", label: "Sound",
    description: "Play the macOS notification sound. Off, notifications arrive silently." },
];

/** Settle idle threads after: Never (0) or this many days. */
export const THREAD_SETTLE_DAYS: readonly number[] = [0, 1, 3, 7, 14, 30];

/**
 * The idle-settle choices. A day count Rust holds that is not one of the
 * standard choices is listed too, so the select never shows a value it lacks.
 */
export function threadSettleDayOptions(current: number): readonly AppSelectOption[] {
  const days = THREAD_SETTLE_DAYS.includes(current) ? THREAD_SETTLE_DAYS
    : [...THREAD_SETTLE_DAYS, current].sort((a, b) => a - b);
  return days.map(day => ({
    value: String(day),
    label: day === 0 ? "Never" : `${day} day${day === 1 ? "" : "s"}`,
  }));
}

export function settingsPageKeyAction(key: string): SettingsPageKeyAction | null {
  return key === "Escape" ? "close" : null;
}

export { LIVE_THEMES_LABEL, THEMES_FOLDER_LABEL } from "./SettingsAppearance";

export const SETTINGS_PAGE_CSS = `
/* Settings fills the window under the toolbar (owner, 2026-11-15): its own
   menu takes the sidebar's place (Back to app, the heading, the pages, the
   version at its foot) and each page is one centred column at most 760px
   wide. No pane inside a pane. */
.settings-page {
  display:grid; grid-template-columns:240px minmax(0, 1fr); grid-template-rows:minmax(0, 1fr);
  width:100%; height:100%; min-width:0; min-height:0; overflow:hidden; color:var(--fg);
  background:var(--editor-bg); border-top:var(--border-w) solid var(--border);
  font-family:var(--font-ui); font-size:var(--text-body); line-height:var(--lh-ui); outline:none;
}
.settings-page :where(button, a, summary, [tabindex]):focus-visible {
  outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); box-shadow:0 0 0 1px var(--focus-contrast);
}
.settings-menu {
  display:flex; flex-direction:column; gap:2px; min-width:0; min-height:0; padding:16px 12px; overflow:auto;
  background:var(--workbench-panel); border-right:var(--border-w) solid var(--border);
}
.settings-back {
  display:inline-flex; align-self:flex-start; align-items:center; gap:6px; height:28px; padding:0 10px 0 8px;
  color:var(--fg); background:var(--surface); border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); cursor:pointer;
  -webkit-app-region:no-drag;
}
.settings-back:hover { color:var(--fg-strong); background:var(--surface-raised); }
.settings-title {
  min-width:0; margin:18px 10px 8px; color:var(--fg-strong);
  font-size:var(--text-h1); font-weight:var(--wght-strong); line-height:var(--lh-tight);
}
.settings-nav-list { display:flex; flex-direction:column; gap:2px; margin:0; padding:0; list-style:none; }
/* The sidebar's neutral hover and selection fills, so the two menus agree. */
.settings-nav-link {
  display:flex; align-items:center; justify-content:space-between; gap:8px; width:100%; min-height:32px;
  padding:0 10px; color:var(--fg-muted); background:transparent; border:0; border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-body); text-align:left; cursor:pointer;
}
.settings-nav-link:hover { color:var(--fg-strong); background:var(--row-hover); }
.settings-nav-link[aria-current="page"] {
  color:var(--fg-strong); background:var(--row-selected); font-weight:var(--wght-strong);
}
/* A page that wants you says so in the menu: amber for setup you owe, the
   done hue for an update waiting on a restart. */
.settings-nav-badge { flex:none; font-size:var(--text-meta); font-weight:var(--wght-regular); white-space:nowrap; }
.settings-nav-badge.is-attention { color:var(--status-waiting); }
.settings-nav-badge.is-ready { color:var(--status-done); }
.settings-menu-version { margin-top:auto; padding:12px 10px 0; color:var(--fg-dim); font-size:var(--text-ui); }
/* The scrollbar's lane is kept on every page, so the column stays put when
   a page that scrolls follows one that does not. */
.settings-content { min-width:0; min-height:0; padding:26px 40px 48px; overflow:auto; scrollbar-gutter:stable; }
.settings-column { width:100%; max-width:760px; margin:0 auto; }
.settings-section { display:flex; flex-direction:column; gap:20px; min-width:0; }
.settings-section[hidden] { display:none; }
.settings-section-heading { display:flex; align-items:flex-end; justify-content:space-between; gap:24px; margin:0; }
.settings-section-title {
  margin:0; color:var(--fg-strong); font-size:var(--text-display); font-weight:var(--wght-strong); line-height:var(--lh-tight);
}
.settings-section-copy { margin:4px 0 0; color:var(--fg-dim); font-size:var(--text-body); line-height:var(--lh-ui); }
.settings-heading-spacer { flex:1 1 auto; }
/* One secondary button (bordered) and one primary (filled) on every page,
   and a text link for what sits behind a disclosure. */
:is(.settings-button, .settings-refresh) {
  display:inline-flex; align-items:center; justify-content:center; flex:none; gap:6px;
  height:var(--control-h); padding:0 12px; color:var(--fg); background:var(--surface);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-ui); white-space:nowrap; cursor:pointer;
}
:is(.settings-button, .settings-refresh):hover:not(:disabled) { color:var(--fg-strong); background:var(--surface-raised); }
:is(.settings-button, .settings-refresh):disabled { color:var(--fg-faint); cursor:default; }
.settings-button.is-primary {
  color:var(--accent-fg); background:var(--accent); border-color:var(--accent); font-weight:var(--wght-strong);
}
.settings-button.is-primary:hover:not(:disabled) { color:var(--accent-fg); background:var(--accent-hover); border-color:var(--accent-hover); }
.settings-button.is-primary:disabled { color:var(--fg-faint); background:var(--surface-raised); border-color:var(--border); }
.settings-button.is-quiet { background:transparent; }
.settings-link {
  display:inline; padding:0; color:var(--fg-muted); background:none; border:0; font:inherit;
  text-decoration:underline; text-underline-offset:2px; cursor:pointer;
}
.settings-link:hover { color:var(--fg-strong); }
/* Sentence case, not the sidebar's uppercase micro-label: the reference names
   a group the way a person would say it. One small heading over one card. */
.settings-group { display:flex; flex-direction:column; gap:8px; min-width:0; }
.settings-group-label { margin:0; color:var(--fg-dim); font-size:var(--text-ui); font-weight:var(--wght-strong); line-height:var(--lh-tight); }
.settings-group-label.is-attention { color:var(--status-waiting); }
.settings-card {
  min-width:0; background:var(--workbench-panel); border:var(--border-w) solid var(--border);
  border-radius:var(--radius-container);
}
/* Hairlines between the rows of a card, edge to edge. */
.settings-card > * + * { border-top:var(--border-w) solid var(--border); }
/* A name and one short help line on the left, the control on the right. */
.settings-row {
  display:flex; align-items:center; justify-content:space-between; gap:10px 24px; min-width:0; padding:11px 16px;
}
.settings-row-copy { display:flex; flex:1 1 auto; flex-direction:column; gap:2px; min-width:0; }
.settings-row-label { color:var(--fg-strong); font-size:var(--text-body); font-weight:var(--wght-strong); line-height:var(--lh-ui); }
.settings-row-description { color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.settings-row-action { display:flex; flex:none; align-items:center; justify-content:flex-end; gap:8px; min-width:0; }
.settings-row-count { color:var(--fg-dim); font-size:var(--text-ui); font-variant-numeric:tabular-nums; }
/* What a row keeps behind its link or button, opened under it. */
.settings-row-more { padding:10px 16px 14px; color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.settings-row-more[hidden] { display:none; }
.settings-row-copy > .settings-row-more { padding:6px 0 0; }
.settings-row-more code { color:var(--fg-muted); font-family:var(--font-mono); font-size:var(--text-meta); overflow-wrap:anywhere; }
/* A switch says its state by its look and to assistive tech; no word beside it. */
.settings-switch { display:inline-flex; align-items:center; flex:none; cursor:pointer; }
.settings-switch input {
  appearance:none; position:relative; flex:none; width:36px; height:20px; margin:0; cursor:pointer;
  background:var(--border-strong); border-radius:var(--radius-pill);
  box-shadow:inset 0 0 0 var(--border-w) var(--border-strong);
}
.settings-switch input::after {
  content:""; position:absolute; top:2px; left:2px; width:16px; height:16px; border-radius:50%;
  background:var(--fg-dim); transition:transform var(--motion-fast) var(--ease-out);
}
.settings-switch input:checked { background:var(--accent); box-shadow:none; }
.settings-switch input:checked::after { background:var(--accent-fg); transform:translateX(16px); }
.settings-switch input:disabled { cursor:default; background:var(--surface-raised); }
.settings-switch input:disabled::after { background:var(--fg-faint); }
.settings-switch input:checked:disabled { background:color-mix(in srgb, var(--accent) 45%, var(--surface-raised)); }
.settings-switch input:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
/* Every choice is a bordered menu button; its key (Light, Motion) sits inside. */
.settings-page .app-select-trigger {
  gap:8px; padding:0 10px; color:var(--fg); background:var(--surface);
  border-color:var(--border-strong); font-size:var(--text-ui);
}
.settings-page .app-select-trigger:hover:not(:disabled), .settings-page .app-select-trigger[aria-expanded="true"] {
  background:var(--surface-raised);
}
.settings-page .app-select-prefix { color:var(--fg-dim); }
${SETTINGS_APPEARANCE_CSS}
.settings-control-group { display:inline-flex; min-width:0; }
/* See-through panes: each slider on one line, its name, the track, its value. */
.settings-page .translucency-controls { display:flex; flex-wrap:wrap; justify-content:flex-end; gap:8px 20px; width:auto; }
.settings-page .translucency-slider {
  display:grid; grid-template-columns:auto 120px 32px; align-items:center; gap:8px; width:auto;
}
.settings-page .translucency-slider-heading { display:contents; }
.settings-page .translucency-slider-heading label { grid-row:1; grid-column:1; color:var(--fg-muted); font-size:var(--text-ui); }
.settings-page .translucency-slider-heading output { grid-row:1; grid-column:3; color:var(--fg-dim); font-size:var(--text-ui); }
.settings-page .translucency-slider input[type="range"] { grid-row:1; grid-column:2; }
.settings-page .translucency-slider-endpoints { display:none; }
/* A line under the cards: where things are kept, what else decides. */
.settings-foot { margin:0; color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.settings-row.is-item .settings-row-label { font-weight:var(--wght-regular); color:var(--fg); }
.settings-row-more-copy { margin:0 0 6px; }
.settings-row-more-line { display:block; margin-top:4px; }
.settings-card > .settings-row { flex-wrap:wrap; }
.settings-row-copy { flex-basis:240px; }
/* Updates (owner, 2026-11-15): one status card for the state the updater is
   in, What's new in plain sentences, and the developer log behind a link. */
.settings-update-card {
  display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:12px 24px;
  padding:18px 20px; background:var(--surface); border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-container);
}
.settings-update-status { display:flex; flex:1 1 280px; flex-direction:column; gap:4px; min-width:0; }
.settings-update-title { color:var(--fg-strong); font-size:var(--text-h2); font-weight:var(--wght-strong); line-height:var(--lh-tight); }
.settings-update-description { color:var(--fg-muted); font-size:var(--text-body); line-height:var(--lh-ui); }
.settings-update-actions { display:flex; flex:none; align-items:center; gap:8px; }
.settings-update-progress { display:flex; flex:1 1 100%; align-items:center; gap:12px; }
.settings-update-progress-copy { flex:none; color:var(--fg-dim); font-size:var(--text-ui); }
.settings-update-progress-track {
  flex:1 1 auto; height:5px; overflow:hidden; background:var(--surface-raised); border-radius:var(--radius-control);
}
.settings-update-progress-fill {
  height:100%; background:var(--accent); border-radius:var(--radius-control); transition:width var(--motion-fast) var(--ease-out);
}
.settings-update-foot {
  display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px 24px;
  color:var(--fg-dim); font-size:var(--text-ui);
}
.settings-update-links { display:flex; gap:16px; }
.settings-link:disabled { color:var(--fg-faint); text-decoration:none; cursor:default; }
.settings-whats-new { display:flex; flex-direction:column; gap:10px; min-width:0; }
.settings-whats-new-head { display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px 24px; }
.settings-whats-new-title { margin:0; color:var(--fg-strong); font-size:var(--text-h3); font-weight:var(--wght-strong); }
.settings-whats-new-filters { display:flex; flex-wrap:wrap; gap:6px; }
.settings-whats-new-release-head {
  display:flex; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:8px 24px; padding:12px 16px;
}
.settings-whats-new-version { color:var(--fg-strong); font-weight:var(--wght-strong); }
.settings-whats-new-yours { color:var(--fg-dim); font-weight:var(--wght-regular); }
.settings-whats-new-meta { display:flex; align-items:center; gap:12px; color:var(--fg-dim); font-size:var(--text-ui); }
.settings-whats-new-change { display:flex; gap:12px; padding:9px 16px; color:var(--fg); line-height:var(--lh-ui); }
.settings-whats-new-kind { flex:none; width:44px; color:var(--fg-dim); font-size:var(--text-ui); line-height:inherit; }
.settings-whats-new-none { margin:0; padding:9px 16px; color:var(--fg-dim); font-size:var(--text-ui); }
/* Models & providers (owner, 2026-11-15): Needs setup, Ready and More, each
   provider a row led by its mark. */
.settings-provider-groups { display:flex; flex-direction:column; gap:20px; min-width:0; }
.settings-provider-groups:empty { display:none; }
.settings-card > .settings-provider-row { flex-wrap:nowrap; gap:14px; padding:12px 16px; }
.settings-provider-row > .settings-row-copy { flex:1 1 0; }
.settings-provider-mark {
  display:inline-flex; align-items:center; justify-content:center; flex:none; width:30px; height:30px;
  color:var(--fg-muted); background:var(--surface); border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control);
}
.settings-provider-mark .provider-logo { width:18px; height:18px; }
.settings-row-description.is-error, .settings-row-description .is-error { color:var(--danger-fg); overflow-wrap:anywhere; }
.settings-row-description.is-error .settings-link { color:var(--fg-muted); }
.settings-provider-meta { display:flex; flex-wrap:wrap; align-items:center; gap:6px; margin-top:2px; color:var(--fg-dim); font-size:var(--text-meta); }
.settings-provider-health {
  display:inline-flex; align-items:center; gap:6px; color:var(--fg-muted); font-size:var(--text-ui); white-space:nowrap;
}
.settings-provider-dot { width:7px; height:7px; background:var(--status-idle); border-radius:50%; }
/* No .is-checking rule: a probe in flight is a process, not a summons, so it
   keeps the base idle grey — the state nobody knows yet — while the health
   label beside it says "Checking" in words. Amber is reserved for an agent
   blocked on the user. */
/* Ready is healthy, not busy: the working hue means an agent is running. */
.settings-provider-dot.is-ready { background:var(--diff-added); }
.settings-provider-dot.is-unavailable { background:var(--status-failed); }
/* Signed out is a problem the owner fixes, like unavailable, not a busy state. */
.settings-provider-dot.is-signedOut { background:var(--status-failed); }
/* A provider's details, under its row: version, models, access, effort. */
.settings-provider-details { display:flex; flex-direction:column; gap:8px; padding-left:60px; }
.settings-provider-fact { margin:0; }
.settings-provider-fact code { color:var(--fg-muted); font-family:var(--font-mono); font-size:var(--text-meta); overflow-wrap:anywhere; }
.settings-provider-chips { display:flex; flex-wrap:wrap; align-items:center; gap:5px; }
.settings-provider-chip-label { min-width:48px; color:var(--fg-dim); font-size:var(--text-meta); }
.settings-provider-model, .settings-provider-capability {
  max-width:220px; overflow:hidden; padding:2px 6px; color:var(--fg-muted); background:var(--surface-raised);
  border-radius:var(--radius-control); font-size:var(--text-meta); line-height:var(--lh-ui);
  text-overflow:ellipsis; white-space:nowrap;
}
.settings-provider-model { font-family:var(--font-mono); }
/* A text field in a row (iPhone remote access' relay address). */
.settings-input {
  width:min(320px, 100%); height:var(--control-h); box-sizing:border-box; padding:0 10px; color:var(--input-fg);
  background:var(--input-bg); border:var(--border-w) solid var(--input-border); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-ui);
}
.settings-input::placeholder { color:var(--input-placeholder); }
.remote-qr, .remote-code { display:flex; flex-direction:column; gap:10px; padding:12px 16px 14px; }
.remote-pairing-code { color:var(--fg-strong); font-family:var(--font-mono); font-size:var(--text-display); user-select:text; }
.settings-row-description code, .remote-verification { color:var(--fg-muted); font-family:var(--font-mono); font-size:var(--text-meta); user-select:text; overflow-wrap:anywhere; }
/* Agent customization: one card per kind, a row per file it found. */
.settings-inventory { display:flex; flex-direction:column; gap:20px; min-width:0; }
.settings-inventory-count { color:var(--fg-faint); font-weight:var(--wght-regular); }
.settings-inventory-detail { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.settings-inventory-error { color:var(--danger-fg); font-size:var(--text-ui); }
.settings-scope-badge {
  flex:none; padding:2px 6px; color:var(--fg-muted); background:var(--surface-raised);
  border-radius:var(--radius-control); font-size:var(--text-meta); line-height:var(--lh-ui);
}
.settings-refresh-note { margin:0; color:var(--fg-dim); font-size:var(--text-ui); }
/* Terminal tools (review C4), behind Show all N: the launcher's tools as rows
   of the More card. */
.settings-tool-list { display:grid; margin:0; padding:0; list-style:none; }
.settings-tool-list[hidden] { display:none; }
.settings-tool { display:flex; align-items:center; gap:14px; min-width:0; padding:10px 16px 10px 32px; }
.settings-tool + .settings-tool { border-top:var(--border-w) solid var(--border); }
.settings-tool-copy { display:block; flex:1 1 auto; min-width:0; }
.settings-tool-name { display:block; color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.settings-tool-missing { display:block; margin-top:4px; color:var(--fg-dim); font-size:var(--text-meta); line-height:var(--lh-ui); overflow-wrap:anywhere; }
.settings-tool-missing code { color:var(--fg-muted); font-family:var(--font-mono); }
.settings-tool-pane-tools { display:block; margin-top:8px; }
.settings-tool-pane-tools-head { display:flex; align-items:center; justify-content:space-between; gap:12px; color:var(--fg-strong); font-size:var(--text-ui); }
/* Accounts (protocol 60): what a row's Accounts menu opens under it. */
.settings-accounts { display:flex; flex-direction:column; gap:8px; padding:4px 16px 12px 60px; }
.settings-accounts[hidden] { display:none; }
.settings-account-list { display:grid; margin:0; padding:0; list-style:none; }
.settings-account { display:flex; align-items:center; justify-content:space-between; gap:8px 16px; flex-wrap:wrap; padding:8px 0; }
.settings-account + .settings-account { border-top:var(--border-w) solid var(--border); }
.settings-account-title { color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.settings-account-badge {
  margin-left:8px; padding:1px 6px; color:var(--fg-muted); background:var(--surface-raised);
  border-radius:var(--radius-control); font-size:var(--text-meta); font-weight:var(--wght-regular);
}
.settings-account-actions { display:flex; flex-wrap:wrap; justify-content:flex-end; gap:6px; }
.settings-account-add { align-self:flex-start; font-size:var(--text-ui); }
.settings-account-name { display:flex; align-items:center; gap:6px; margin-top:6px; }
.settings-account-input { min-width:0; flex:1 1 auto; height:var(--control-h); box-sizing:border-box; padding:0 8px; color:var(--input-fg); background:var(--input-bg); border:var(--border-w) solid var(--input-border); border-radius:var(--radius-control); font:inherit; font-size:var(--text-ui); }
.settings-shortcut-list { margin:0; padding:0; list-style:none; }
.settings-shortcut {
  display:flex; align-items:center; justify-content:space-between; gap:12px 24px; min-width:0; padding:10px 16px;
}
.settings-shortcut + .settings-shortcut { border-top:var(--border-w) solid var(--border); }
.settings-shortcut-label { color:var(--fg-strong); font-size:var(--text-body); font-weight:var(--wght-strong); }
.settings-shortcut-description { color:var(--fg-dim); font-size:var(--text-ui); line-height:var(--lh-ui); }
.settings-shortcut-keys { display:flex; flex:none; align-items:center; justify-content:flex-end; gap:4px; }
.settings-shortcut-key {
  display:inline-flex; align-items:center; justify-content:center; min-width:22px; height:22px; padding:0 6px;
  color:var(--fg-muted); background:var(--surface); border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); font-family:var(--font-ui); font-size:var(--text-ui);
}
.settings-error {
  margin:0; padding:7px 9px; color:var(--danger-fg); background:var(--danger-bg);
  border-radius:var(--radius-control); font-size:var(--text-ui); line-height:var(--lh-ui);
}
.settings-empty { padding:24px; color:var(--fg-dim); text-align:center; font-size:var(--text-ui); }
/* A narrow window: the menu becomes a strip over the page, its pages
   scrolling sideways. */
@media (max-width:840px) {
  .settings-page { grid-template-columns:minmax(0, 1fr); grid-template-rows:auto minmax(0, 1fr); }
  .settings-menu { flex-direction:row; align-items:center; gap:8px; padding:8px; overflow:hidden;
    border-right:0; border-bottom:var(--border-w) solid var(--border); }
  .settings-back { align-self:center; }
  .settings-title, .settings-menu-version { display:none; }
  .settings-nav-list { flex-direction:row; min-width:0; overflow-x:auto; }
  .settings-nav-link { white-space:nowrap; }
  .settings-content { padding:20px 18px 40px; }
}
@media (max-width:480px) {
  .settings-content { padding-right:12px; padding-left:12px; }
  .settings-row, .settings-provider-row { padding-right:12px; padding-left:12px; }
  .settings-card > .settings-provider-row { flex-wrap:wrap; }
  .settings-update-actions { width:100%; justify-content:flex-start; }
  .settings-shortcut { gap:10px; }
  .settings-shortcut-description { display:none; }
}
@media (prefers-reduced-motion:reduce) {
  .settings-update-progress-fill { transition:none; }
}
${UPDATE_LOG_PANEL_CSS}
`;

export { CUSTOMIZATION_GROUPS } from "../task-inspector-view";

/** The providers whose customization Settings can read. */
const CUSTOMIZATION_PROVIDERS: readonly (AppSelectOption & { value: CustomizationProvider })[] = [
  { value: "claude", label: "Claude" }, { value: "codex", label: "Codex" }, { value: "grok", label: "Grok" },
  { value: "qwen", label: "Qwen" }, { value: "opencode", label: "OpenCode" },
];

/** A folder's own name, the last part of its path. */
const pathLeaf = (path: string): string => path.replace(/[/\\]+$/u, "").split(/[/\\]/u).pop() || path;
const SCOPE_LABELS = CUSTOMIZATION_SCOPE_LABELS;

export interface CustomizationSectionProps {
  id: string;
  /** The folder the inventory was read for; "" before one is known. */
  root: string;
  entries: readonly CustomizationEntry[];
  truncated?: boolean;
  loading?: boolean;
  error?: string | null;
  /** Another settings page is showing. */
  hidden?: boolean;
  onRefresh: () => void;
  provider?: CustomizationProvider;
  userRoot?: string;
  onProviderChange?: (provider: CustomizationProvider) => void;
  onSource?: (entry: CustomizationEntry) => void;
  onCheck?: (entry: CustomizationEntry) => void;
  checking?: boolean;
  checkingName?: string | null;
  health?: ProviderCustomization["health"];
}

/**
 * Read-only: what a session started in this project inherits, by name. There is
 * no enable/disable here and there should not be — Interface does not own these
 * files, and a toggle would claim authority over a vendor CLI's own
 * configuration. The point is that the list exists at all: until now nothing in
 * the app said which rules, skills, hooks or servers a run would pick up.
 */
export function CustomizationSection({
  id, root, entries, truncated = false, loading = false, error = null, hidden = false, onRefresh,
  provider = "claude", userRoot, onProviderChange, onSource, onCheck, checking = false, checkingName = null, health,
}: CustomizationSectionProps) {
  const groups = CUSTOMIZATION_GROUPS
    .map(group => ({ ...group, rows: entries.filter(entry => entry.kind === group.kind) }))
    .filter(group => group.rows.length > 0);
  const empty = loading ? "Reading project and user customization…"
    : error !== null ? "Customization could not be read."
      : root.length === 0 ? "No project folder is selected."
        : `Nothing found in ${pathLeaf(root)} or in your own settings.`;

  const folders = useSettingsDisclosure();
  const providerName = CUSTOMIZATION_PROVIDERS.find(option => option.value === provider)?.label ?? "the provider";

  return <section id={id} className="settings-section" hidden={hidden} aria-labelledby={`${id}-title`}>
    <div className="settings-section-heading">
      <div>
        <h2 id={`${id}-title`} className="settings-section-title">Agent customization</h2>
        <p className="settings-section-copy">
          The rules, skills, commands, hooks and MCP servers a new session picks up.
        </p>
      </div>
      <button type="button" className="settings-button" disabled={loading}
        aria-busy={loading || undefined}
        aria-label={loading ? "Refreshing agent customization" : "Refresh agent customization"}
        onClick={onRefresh}>
        {loading ? "Refreshing…" : "Refresh"}
      </button>
    </div>

    <SettingsGroup label="Where it looks">
      <SettingsRow labelId={`${id}-provider`} label="Provider"
        description={<>{root.length === 0 ? "No project folder is selected."
          : `In ${pathLeaf(root)} and your own ${providerName} settings.`}
          {root.length > 0 && <>{" "}{folders.link("Show folders", "Hide folders")}</>}</>}
        detail={root.length > 0 && folders.panel(<>
          <span className="settings-row-more-line">Project: <code title={root}>{homeRelativePath(root)}</code></span>
          <span className="settings-row-more-line">Yours: {userRoot === undefined ? "the provider's own folder"
            : <code title={userRoot}>{homeRelativePath(userRoot)}</code>}</span>
        </>)}>
        {onProviderChange !== undefined && <AppSelect label="Customization provider" value={provider}
          disabled={checking} options={CUSTOMIZATION_PROVIDERS}
          onChange={value => { onProviderChange(value as CustomizationProvider); }} />}
      </SettingsRow>
    </SettingsGroup>
    {health !== undefined && <p className="settings-foot" role="status">{health.name}: {health.state}. {health.message}</p>}
    <div className="settings-inventory" aria-live="polite" aria-busy={loading || undefined}>
      {groups.length === 0
        ? <div className="settings-card settings-empty">{empty}</div>
        : groups.map(group => <SettingsGroup key={group.kind}
          label={<>{group.label} <span className="settings-inventory-count">{group.rows.length}</span></>}>
          {group.rows.map((entry, index) => <div
            key={`${entry.sourcePath}:${entry.name}:${index}`}
            className="settings-row settings-inventory-row" title={entry.sourcePath}>
            <span className="settings-row-copy">
              <span className="settings-row-label">{entry.name}</span>
              {entry.detail.length > 0 && <span className="settings-row-description settings-inventory-detail">
                {entry.detail}
              </span>}
              {entry.error !== undefined && <span className="settings-inventory-error">
                {entry.error}
              </span>}
            </span>
            <span className="settings-row-action">
              <span className="settings-scope-badge">{SCOPE_LABELS[entry.scope]}</span>
              {onSource !== undefined && <button type="button" className="settings-button" disabled={loading || checking}
                aria-label={`Reveal source for ${entry.name}`} onClick={() => { onSource(entry); }}>Source</button>}
              {entry.kind === "mcp" && entry.error === undefined && onCheck !== undefined
                && <button type="button" className="settings-button" disabled={loading || checking}
                  aria-label={`Check MCP server ${entry.name}`} onClick={() => { onCheck(entry); }}>{checking && (checkingName === null || checkingName === entry.name) ? "Checking…" : "Check"}</button>}
            </span>
          </div>)}
        </SettingsGroup>)}
    </div>
    {truncated && <p className="settings-foot">
      Long lists stop at 200 entries per kind; some are not shown.
    </p>}
    <p className="settings-foot">
      Listed from the files only: which ones a tool loads, and in what order, is up to the tool, and whether a
      running session has loaded them is unknown. Check starts an MCP server for a moment to see that it
      answers; it never calls its tools.
    </p>
    {error !== null && <p className="settings-error" role="alert">
      Customization could not be read: {error}
    </p>}
  </section>;
}

/** Keep account context useful without exposing a full identity during screen sharing. */
export function maskProviderIdentity(identity: string): string {
  const trimmed = identity.trim();
  const at = trimmed.lastIndexOf("@");
  if (at > 0 && at < trimmed.length - 1) {
    const local = Array.from(trimmed.slice(0, at));
    const shown = local.slice(0, Math.min(2, local.length)).join("");
    return `${shown}${local.length > 2 ? "•••" : ""}@${trimmed.slice(at + 1)}`;
  }
  const characters = Array.from(trimmed);
  if (characters.length <= 4) return trimmed;
  return `${characters.slice(0, 2).join("")}•••${characters.slice(-2).join("")}`;
}

/**
 * Settings › Updates' status card (owner, 2026-11-15): one title and one
 * sentence for each state the updater is really in, in plain words.
 */
export function updateCard(state: UpdateState, summary: UpdateSettingsSummary): { title: string; description: string } {
  const product = productLabel();
  const current = summary.currentVersion;
  const next = summary.availableVersion === null ? "An update" : `${product} ${summary.availableVersion}`;
  switch (state.phase) {
    case "ready":
      return { title: `${next} is ready`, description: state.canRestart
        ? `Downloaded and checked. You are on ${current}; your open panes come back after the restart.`
        : `Downloaded and checked. Restart ${product} to finish installing it.` };
    case "downloading":
      return { title: `Downloading ${next}`, description: `It downloads in the background. You are on ${current}.` };
    case "available":
      return { title: `${next} is available`, description: `It downloads by itself. You are on ${current}.` };
    case "installing":
      return { title: `Installing ${next}`, description: `${product} reopens when macOS has finished.` };
    case "checking":
      return { title: "Checking for updates…", description: `You are on ${product} ${current}.` };
    case "up-to-date":
      return { title: "You are up to date", description: `${product} ${current} is the newest version.` };
    case "idle":
      return { title: `You are on ${product} ${current}`, description: "Check now to look for a newer version." };
    case "disabled":
      return { title: "This build has no update channel",
        description: `It does not update itself, as in Dev and the test app. You are on ${current}.` };
    case "error":
      return { title: "Update check failed", description: `${product} could not check for updates. You are on ${current}.` };
  }
}

export function SettingsPage({
  initialSection,
  onRemoteAccess,
  appearance,
  appearancePending = false,
  appearanceError = null,
  onAppearanceChange,
  onSystemThemesChange,
  onTranslucencyChange,
  onWorkspacePresentationChange,
  preferences,
  preferencesError = null,
  onPreferencesChange,
  engineHealth,
  providersRefreshing = false,
  providerRefreshError = null,
  onRefreshProviders,
  acpProfiles = [],
  acpProfilesPending = false,
  acpProfilesError = null,
  acpProfilesRestartRequired = false,
  acpProfilesSavedRevision = 0,
  onSaveAcpProfiles,
  customizationRoot,
  customizationApi,
  threadSettings = null,
  threadSettingsPending = false,
  threadSettingsError = null,
  onThreadSettingsChange,
  threadSchedules,
  threadSecrets,
  updateState = null,
  onCheckForUpdates,
  onRestartToUpdate,
  onSignIn,
  providerAccounts,
  onProviderAccountsChange,
  onAccountSignIn,
  onProvidersShown,
  onRevealDataFolder,
  terminalTools = null,
  onPaneToolsChange,
  paneToolsPending = null,
  paneToolsError = null,
  onClose,
}: SettingsPageProps) {
  const baseId = useId();
  const pageRef = useRef<HTMLElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [activeSection, setActiveSection] = useState<SettingsSectionId>(() => initialSection
    ?? (updateState?.phase === "ready" && updateState.canRestart ? "updates" : "general"));
  const normalized = normalizeAppPreferences(preferences);
  const configuredProviderLabels = useMemo<Record<string, string>>(() =>
    Object.fromEntries(acpProfiles.map(profile => [`acp:${profile.slug}`, profile.displayName])),
  [acpProfiles]);
  const providers = useMemo(() => summarizeProviderProbes(engineHealth,
    configuredProviderLabels), [configuredProviderLabels, engineHealth]);
  // Review C4: the launcher's tools on the same page, under the same Refresh.
  const tools = useMemo(() => terminalTools === null ? null
    : summarizeTerminalTools(terminalTools, engineHealth), [terminalTools, engineHealth]);
  const updateSummary = updateState === null ? null : summarizeUpdateState(updateState);
  const [customization, setCustomization] = useState<CustomizationInventory | null>(null);
  const [customizationRootLabel, setCustomizationRootLabel] = useState(customizationRoot ?? "");
  const [customizationLoading, setCustomizationLoading] = useState(true);
  const [customizationError, setCustomizationError] = useState<string | null>(null);
  const [customizationRevision, setCustomizationRevision] = useState(0);
  const [customizationProvider, setCustomizationProvider] = useState<CustomizationProvider>("claude");
  const [providerCustomization, setProviderCustomization] = useState<ProviderCustomization | null>(null);
  const [customizationChecking, setCustomizationChecking] = useState(false);
  const [customizationCheckingName, setCustomizationCheckingName] = useState<string | null>(null);
  const customizationContext = useRef("");
  customizationContext.current = `${customizationProvider}:${customizationRoot ?? ""}`;
  const customizationAction = async (entry: CustomizationEntry, check: boolean): Promise<void> => {
    if (customizationChecking || customizationApi.customizationControl === undefined) return;
    const context = customizationContext.current;
    setCustomizationChecking(true);
    setCustomizationCheckingName(check ? entry.name : null);
    setCustomizationError(null);
    try {
      const command = { root: customizationRootLabel, provider: customizationProvider, path: entry.sourcePath };
      const result = await customizationApi.customizationControl(check
        ? { kind: "checkMcp", ...command, name: entry.name } : { kind: "source", ...command });
      if (context === customizationContext.current) setProviderCustomization(result);
    } catch (error) {
      if (context === customizationContext.current) setCustomizationError(error instanceof Error ? withoutIpcErrorShape(error.message) : String(error));
    } finally { setCustomizationChecking(false); setCustomizationCheckingName(null); }
  };

  /* The inventory is read here rather than passed in because it is this page's
     own content: nothing else in the app needs it, and it must re-read whenever
     the user asks — the files change outside Interface, between two openings of
     this page. */
  useEffect(() => {
    let cancelled = false;
    setCustomizationLoading(true);
    setCustomizationError(null);
    setProviderCustomization(null);
    void readCustomizationInventory(customizationApi, customizationRoot, () => cancelled, {
      root: setCustomizationRootLabel,
      provider: setProviderCustomization,
      inventory: inventory => {
        setCustomization(inventory);
        setCustomizationRootLabel(inventory.root);
      },
      failed: message => {
        setCustomization(null);
        setCustomizationError(message);
      },
      settled: () => { setCustomizationLoading(false); },
    }, customizationProvider);
    return () => { cancelled = true; };
  }, [customizationApi, customizationRoot, customizationRevision, customizationProvider]);

  useEffect(() => { pageRef.current?.focus(); }, []);

  const changePreferences = (patch: AppPreferencePatch): void => {
    onPreferencesChange(updateAppPreferences(normalized, patch));
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLElement>): void => {
    if (settingsPageKeyAction(event.key) !== "close") return;
    event.preventDefault();
    event.stopPropagation();
    onClose();
  };

  const ids = {
    title: `${baseId}-title`,
    general: `${baseId}-general`,
    providers: `${baseId}-providers`,
    customization: `${baseId}-customization`,
    threads: `${baseId}-threads`,
    notifications: `${baseId}-notifications`,
    updates: `${baseId}-updates`,
    privacy: `${baseId}-privacy`,
    remote: `${baseId}-remote`,
    shortcuts: `${baseId}-shortcuts`,
  };

  // One section is a page at a time; the rest stay mounted, hidden, so drafts,
  // open disclosures and the remote-access poll survive switching pages.
  const goToSection = (section: SettingsSectionId): void => {
    setActiveSection(section);
    contentRef.current?.scrollTo({ top: 0 });
  };

  // Updates and remote access are absent from the rail when this build has no
  // bridge for them; a page that loses its entry falls back to Appearance.
  const rail = RAIL.filter(entry => (entry.id !== "updates" || updateState !== null)
    && (entry.id !== "remote" || onRemoteAccess !== undefined)
    && (entry.id !== "threads" || onThreadSettingsChange !== undefined));
  // Every change sends the whole object; nothing moves until Rust has answered once.
  const threadsLocked = threadSettings === null || threadSettingsPending;
  const outsideHelp = useSettingsDisclosure();
  const [developerLog, setDeveloperLog] = useState(false);
  const changeThreadSettings = (patch: Partial<ThreadSettings>): void => {
    if (threadSettings !== null) onThreadSettingsChange?.({ ...threadSettings, ...patch });
  };
  const shown = rail.some(entry => entry.id === activeSection) ? activeSection : "general";
  // What the menu says beside a page that wants you: providers that need
  // setting up, an update waiting on a restart.
  const needsSetup = providers.filter(provider =>
    provider.health === "unavailable" || provider.health === "signedOut").length;
  const railBadge = (id: SettingsSectionId): { text: string; tone: "attention" | "ready" } | null =>
    id === "providers" && needsSetup > 0 ? { text: `${needsSetup} need${needsSetup === 1 ? "s" : ""} setup`, tone: "attention" }
      : id === "updates" && updateState?.phase === "ready" && updateState.canRestart ? { text: "1 ready", tone: "ready" }
        : null;
  // Accounts are read, and each sign-in checked, whenever Models & providers is shown.
  const providersShownRef = useRef(onProvidersShown);
  providersShownRef.current = onProvidersShown;
  useEffect(() => {
    if (shown === "providers") providersShownRef.current?.();
  }, [shown]);
  // Protocol 67: the held secrets are read whenever Threads is shown.
  const secretsShownRef = useRef(threadSecrets?.onShown);
  secretsShownRef.current = threadSecrets?.onShown;
  useEffect(() => {
    if (shown === "threads") secretsShownRef.current?.();
  }, [shown]);

  return <section ref={pageRef} className="settings-page" tabIndex={-1}
    aria-labelledby={ids.title} aria-keyshortcuts="Escape" onKeyDown={handleKeyDown}>
    <style>{SETTINGS_PAGE_CSS}</style>

    <nav className="settings-menu" aria-label="Settings sections">
      <button type="button" className="settings-back" onClick={onClose}>
        <span className="codicon codicon-chevron-left" aria-hidden="true" />Back to app
      </button>
      <h1 id={ids.title} className="settings-title">Settings</h1>
      <ul className="settings-nav-list">
        {rail.map(entry => {
          const badge = railBadge(entry.id);
          return <li key={entry.id}>
            <button type="button" className="settings-nav-link"
              aria-current={shown === entry.id ? "page" : undefined}
              onClick={() => { goToSection(entry.id); }}>
              {entry.label}
              {badge !== null && <span className={`settings-nav-badge is-${badge.tone}`}>{badge.text}</span>}
            </button>
          </li>;
        })}
      </ul>
      {updateState !== null && updateState.currentVersion.trim() !== "" && (
        <span className="settings-menu-version">{productLabel()} {updateState.currentVersion.trim()}</span>
      )}
    </nav>

    <div ref={contentRef} className="settings-content">
      <div className="settings-column">
        <AppearanceSection id={ids.general} hidden={shown !== "general"} appearance={appearance}
          pending={appearancePending} error={appearanceError} onAppearanceChange={onAppearanceChange}
          onSystemThemesChange={onSystemThemesChange} onTranslucencyChange={onTranslucencyChange}
          onWorkspacePresentationChange={onWorkspacePresentationChange} preferences={normalized}
          preferencesError={preferencesError} changePreferences={changePreferences}
          onRevealDataFolder={onRevealDataFolder} />

        <ProvidersSection id={ids.providers} hidden={shown !== "providers"} providers={providers}
          refreshing={providersRefreshing} refreshError={providerRefreshError}
          onRefresh={() => { onRefreshProviders(); onProvidersShown?.(); }} onSignIn={onSignIn}
          providerAccounts={providerAccounts} onProviderAccountsChange={onProviderAccountsChange}
          onAccountSignIn={onAccountSignIn} labels={configuredProviderLabels} mask={maskProviderIdentity}
          tools={tools} onPaneToolsChange={onPaneToolsChange} paneToolsPending={paneToolsPending}
          paneToolsError={paneToolsError} acpProfiles={acpProfiles} acpProfilesPending={acpProfilesPending}
          acpProfilesError={acpProfilesError} acpProfilesRestartRequired={acpProfilesRestartRequired}
          acpProfilesSavedRevision={acpProfilesSavedRevision} onSaveAcpProfiles={onSaveAcpProfiles} />

        <CustomizationSection id={ids.customization} root={customizationRootLabel}
          entries={customization?.entries ?? []}
          truncated={customization?.truncated ?? false}
          loading={customizationLoading} error={customizationError}
          hidden={shown !== "customization"}
          provider={customizationProvider} userRoot={providerCustomization?.userRoot}
          onProviderChange={setCustomizationProvider} checking={customizationChecking}
          checkingName={customizationCheckingName}
          health={providerCustomization?.health}
          onSource={entry => { void customizationAction(entry, false); }}
          onCheck={entry => { void customizationAction(entry, true); }}
          onRefresh={() => { setCustomizationRevision(revision => revision + 1); }} />

        {onThreadSettingsChange !== undefined && (
          <section id={ids.threads} className="settings-section" hidden={shown !== "threads"}
            aria-labelledby={`${ids.threads}-title`}>
            <div className="settings-section-heading">
              <div>
                <h2 id={`${ids.threads}-title`} className="settings-section-title">Threads</h2>
                <p className="settings-section-copy">
                  {threadSettings === null ? "Reading thread settings…"
                    : "How threads tidy themselves up, and what agents may do with them."}
                </p>
              </div>
            </div>
            <SettingsGroup label="Tidying up" busy={threadSettingsPending}>
              <SettingsRow labelId={`${ids.threads}-settle`} label="Settle quiet threads"
                description="Moves a thread out of Active after this long without a run. Pinned and snoozed threads stay.">
                <AppSelect label="Settle idle threads after"
                  value={String(threadSettings?.autoSettleDays ?? 3)}
                  options={threadSettleDayOptions(threadSettings?.autoSettleDays ?? 3)}
                  disabled={threadsLocked}
                  onChange={value => { changeThreadSettings({ autoSettleDays: Number(value) }); }} />
              </SettingsRow>
              <SettingsRow labelId={`${ids.threads}-merge`} label="Settle when the pull request merges"
                description={threadSettings?.trackPullRequests === true
                  ? "A thread is done once its work is merged."
                  : "A thread is done once its work is merged. Needs Follow pull requests on GitHub."}>
                <SettingsSwitch labelId={`${ids.threads}-merge`} checked={threadSettings?.settleOnMerge ?? false}
                  disabled={threadsLocked || threadSettings?.trackPullRequests !== true}
                  onChange={on => { changeThreadSettings({ settleOnMerge: on }); }} />
              </SettingsRow>
              <SettingsRow labelId={`${ids.threads}-prs`} label="Follow pull requests on GitHub"
                description="Checks each thread's pull request every 10 minutes, with the GitHub app (gh).">
                <SettingsSwitch labelId={`${ids.threads}-prs`} checked={threadSettings?.trackPullRequests ?? false}
                  disabled={threadsLocked}
                  onChange={on => { changeThreadSettings({ trackPullRequests: on }); }} />
              </SettingsRow>
              <SettingsRow labelId={`${ids.threads}-titles`} label="Name threads automatically"
                description="Claude Haiku 5.5 names a thread from your first message. Uses a little of your Claude plan.">
                <SettingsSwitch labelId={`${ids.threads}-titles`} checked={threadSettings?.generatedTitles ?? false}
                  disabled={threadsLocked}
                  onChange={on => { changeThreadSettings({ generatedTitles: on }); }} />
              </SettingsRow>
            </SettingsGroup>
            <SettingsGroup label="Agents working together" busy={threadSettingsPending}>
              <SettingsRow labelId={`${ids.threads}-team`} label="Let agents start helper threads"
                description="An agent can start, read and message helpers in its own project, never with more permission than it has. Running agents keep this until they restart.">
                <SettingsSwitch labelId={`${ids.threads}-team`} checked={threadSettings?.teamTools !== false}
                  disabled={threadsLocked}
                  onChange={on => { changeThreadSettings({ teamTools: on }); }} />
              </SettingsRow>
              <SettingsRow labelId={`${ids.threads}-pane-tools`} label="Pane tools without asking"
                description="Let an agent in one pane type into another pane, or start one, without asking you first.">
                <SettingsSwitch labelId={`${ids.threads}-pane-tools`} checked={threadSettings?.paneToolsWithoutAsking === true}
                  disabled={threadsLocked}
                  onChange={on => { changeThreadSettings({ paneToolsWithoutAsking: on }); }} />
              </SettingsRow>
              <SettingsRow labelId={`${ids.threads}-outside`} label={`Agents outside ${productLabel()}`}
                description={<>Lets Claude Code or Codex in any terminal read or start your threads.
                  {" "}{outsideHelp.link("How to connect one", "Hide how to connect")}</>}
                detail={outsideHelp.panel(<>
                  <p className="settings-row-more-copy">{OUTSIDE_AGENTS_DESCRIPTION}</p>
                  {OUTSIDE_AGENT_SETUP.map(({ tool, command }) => (
                    <span key={tool} className="settings-row-more-line" data-outside-agents-setup="">
                      {tool}: <InstallHint hint={`\`${command}\``} />
                    </span>
                  ))}
                </>)}>
                <AppSelect label="Outside agents"
                  value={outsideAgentsLevel(threadSettings)}
                  options={OUTSIDE_AGENT_OPTIONS}
                  disabled={threadsLocked}
                  onChange={value => {
                    const level = OUTSIDE_AGENT_OPTIONS.find(option => option.value === value)?.value;
                    if (level !== undefined) changeThreadSettings({ outsideAgents: level });
                  }} />
              </SettingsRow>
            </SettingsGroup>
            {threadSettingsError !== null && <p className="settings-error" role="alert">
              Thread settings could not be saved: {threadSettingsError}
            </p>}
            {(threadSchedules !== undefined || threadSecrets !== undefined) && (
              <SettingsGroup label="Kept for your threads">
                {threadSchedules !== undefined && (
                  <ThreadSchedulesSettings rows={threadSchedules.rows}
                    locked={threadSchedules.pending} error={threadSchedules.error}
                    onEnable={threadSchedules.onEnable} onDelete={threadSchedules.onDelete} />
                )}
                {threadSecrets !== undefined && (
                  <ThreadSecretsSettings rows={threadSecrets.rows} locked={threadSecrets.pending}
                    error={threadSecrets.error} onForget={threadSecrets.onForget} />
                )}
              </SettingsGroup>
            )}
          </section>
        )}

        <section id={ids.notifications} className="settings-section"
          hidden={shown !== "notifications"} aria-labelledby={`${ids.notifications}-title`}>
          <div className="settings-section-heading">
            <div>
              <h2 id={`${ids.notifications}-title`} className="settings-section-title">Notifications</h2>
              <p className="settings-section-copy">
                A macOS notification when work you are not watching finishes or needs you.
              </p>
            </div>
          </div>
          <SettingsGroup label="Tell me">
            {NOTIFICATION_ROWS.map(row => <SettingsRow key={row.key}
              labelId={`${ids.notifications}-${row.key}`} label={row.label} description={row.description}>
              <SettingsSwitch labelId={`${ids.notifications}-${row.key}`} checked={normalized[row.key]}
                onChange={on => { changePreferences({ [row.key]: on }); }} />
            </SettingsRow>)}
          </SettingsGroup>
          <p className="settings-foot">Focus modes and System Settings › Notifications still decide what reaches you.</p>
        </section>

        {updateState !== null && updateSummary !== null && (() => {
          const card = updateCard(updateState, updateSummary);
          return <section id={ids.updates} className="settings-section" hidden={shown !== "updates"}
            aria-labelledby={`${ids.updates}-title`}>
            <div className="settings-section-heading">
              <div>
                <h2 id={`${ids.updates}-title`} className="settings-section-title">Updates</h2>
                <p className="settings-section-copy">
                  New versions download by themselves. You choose when to restart.
                </p>
              </div>
            </div>

            {/* One card for the state the updater is really in (owner, 2026-11-15). */}
            <div className={`settings-update-card is-${updateState.phase}`} data-update-state={updateState.phase}>
              <div className="settings-update-status" role="status" aria-live="polite"
                aria-atomic="true" aria-busy={updateSummary.busy || undefined}>
                <span className="settings-update-title">{card.title}</span>
                <span className="settings-update-description">{card.description}</span>
              </div>
              {updateSummary.ready && updateState.canRestart && onRestartToUpdate !== undefined && (
                <div className="settings-update-actions">
                  <button type="button" className="settings-button is-quiet" onClick={onClose}>Later</button>
                  <button type="button" className="settings-button is-primary" onClick={onRestartToUpdate}>
                    Restart to update
                  </button>
                </div>
              )}
              {updateState.phase === "downloading" && (
                <div className="settings-update-progress">
                  <div className="settings-update-progress-track" role="progressbar"
                    aria-label="Update download progress" aria-valuemin={0} aria-valuemax={100}
                    aria-valuenow={updateSummary.progressPercent ?? undefined}
                    aria-valuetext={updateSummary.progressLabel ?? undefined}>
                    <div className="settings-update-progress-fill"
                      style={{ width: `${updateSummary.progressPercent ?? 0}%` }} />
                  </div>
                  <span className="settings-update-progress-copy">{updateSummary.progressLabel}</span>
                </div>
              )}
            </div>

            {updateSummary.error !== null && (
              <p className="settings-error" role="alert">
                Updates could not be completed: {updateSummary.error}
              </p>
            )}

            {shown === "updates" && (
              <Suspense fallback={null}>
                <WhatsNew currentVersion={updateSummary.currentVersion} />
              </Suspense>
            )}

            <div className="settings-update-foot">
              <span>{updateState.lastCheckedAt === null ? "Not checked yet"
                : <>Last checked <time dateTime={updateState.lastCheckedAt}>{updateSummary.lastCheckedLabel}</time></>}</span>
              <span className="settings-update-links">
                {onCheckForUpdates !== undefined && updateState.phase !== "disabled" && (
                  <button type="button" className="settings-link"
                    disabled={!updateState.canCheck || updateSummary.busy}
                    aria-busy={updateState.phase === "checking" || undefined}
                    onClick={onCheckForUpdates}>
                    {updateState.phase === "checking" ? "Checking…" : "Check now"}
                  </button>
                )}
                <button type="button" className="settings-link" aria-expanded={developerLog}
                  onClick={() => { setDeveloperLog(open => !open); }}>
                  {developerLog ? "Hide developer log" : "Developer log"}
                </button>
              </span>
            </div>

            {/* Every landing with its kind, hashes, branches and the tests that
                verify it: the log as it was, for whoever wants the detail. */}
            {shown === "updates" && developerLog && (
              <Suspense fallback={null}>
                <UpdateLogPanel currentVersion={updateSummary.currentVersion} />
              </Suspense>
            )}
          </section>;
        })()}

        <PrivacySection id={ids.privacy} hidden={shown !== "privacy"} pages={rail} onGo={goToSection} />

        {onRemoteAccess && <RemoteAccessPanel id={ids.remote} manage={onRemoteAccess}
          hidden={shown !== "remote"} />}

        <section id={ids.shortcuts} className="settings-section" hidden={shown !== "shortcuts"}
          aria-labelledby={`${ids.shortcuts}-title`}>
          <div className="settings-section-heading">
            <div>
              <h2 id={`${ids.shortcuts}-title`} className="settings-section-title">Shortcuts</h2>
              <p className="settings-section-copy">The keys that get you around {productLabel()} fastest.</p>
            </div>
          </div>
          {SHORTCUT_GROUPS.map(group => <SettingsGroup key={group.label} label={group.label}>
            <ul className="settings-shortcut-list">
              {SETTINGS_SHORTCUTS.slice(group.from, group.to).map(shortcut => <li key={shortcut.label}
                className="settings-shortcut">
                <span className="settings-row-copy">
                  <span className="settings-shortcut-label">{shortcut.label}</span>
                  <span className="settings-shortcut-description">{shortcut.description}</span>
                </span>
                <span className="settings-shortcut-keys" role="img"
                  aria-label={`Shortcut: ${shortcut.keys.join(" plus ")}`}>
                  {shortcut.keys.map((key, index) => <kbd key={`${key}-${index}`}
                    className="settings-shortcut-key" aria-hidden="true">{key}</kbd>)}
                </span>
              </li>)}
            </ul>
          </SettingsGroup>)}
        </section>
      </div>
    </div>
  </section>;
}
