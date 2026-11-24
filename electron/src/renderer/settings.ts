import { providerLabel } from "./provider-labels";
import type { EngineId } from "../shared/engine";
import {
  canonicalThemePreference, DEFAULT_CLI_THEME, isCliThemePreference, isCustomThemeId,
  type CliThemePreference, type ThemeId,
} from "../shared/appearance";
import type { EngineProbe, TerminalLaunchProfile } from "../shared/ipc";
import type { UpdateState } from "../shared/updater";
import { asRecord } from "./helpers";
import { productLabel } from "./product";
import type { NotificationPreferences } from "./notify";

import type { ThemeMotionPreference } from "./theme-look";

export const UI_FONT_PREFERENCES = ["sans", "serif"] as const;
export type UiFontPreference = typeof UI_FONT_PREFERENCES[number];

/** How BUILT the chrome looks. Form is still the CSS axis; Bare CLI also pairs a console palette. */
export const UI_CHROME_PREFERENCES = ["refined", "cli"] as const;
export type UiChromePreference = typeof UI_CHROME_PREFERENCES[number];

/** Archived Clean input values retained for preference compatibility. */
export const CLEAN_INPUT_PREFERENCES = ["composer", "native"] as const;
export type CleanInputPreference = typeof CLEAN_INPUT_PREFERENCES[number];

/** Renderer-owned preferences that are useful outside one provider session. */
export interface AppPreferences {
  /** Assistant reply typeface. Serif is the editorial stack. */
  uiFont: UiFontPreference;
  /**
   * Chrome form. `refined` is the shipped harness — rounded, layered, sans.
   * `cli` is a flat terminal console: monospace, square, no elevation or blur.
   * Switching to `cli` also applies `lastCliTheme`; leaving restores
   * `lastWorkspaceTheme`.
   */
  uiChrome: UiChromePreference;
  /** Whether a theme file's backdrop moves: absent means it does, `plugged`
   * only on the power adapter, `off` never. Stored only once changed. */
  themeMotion?: ThemeMotionPreference;
  /** The first-run privacy summary was answered (Got it or Open Privacy).
   * Stored only once true, so it shows once per profile. */
  privacySummarySeen?: true;
  /** Legacy Clean input value, retained for stored preference compatibility only. */
  cleanInput?: CleanInputPreference;
  /** Last console palette used in Bare CLI. Carbon, the one console palette since 2026-09-28. */
  lastCliTheme: CliThemePreference;
  /** Last workspace palette used in Refined. Restored when Bare CLI turns off. */
  lastWorkspaceTheme: ThemeId;
  /** OS notifications play the system sound (Settings › Notifications). */
  notifySound: boolean;
  /** Notify when a turn, task or terminal agent you are not watching finishes or fails. */
  notifyFinished: boolean;
  /** Notify when an agent you are not watching needs an answer or approval. */
  notifyNeedsInput: boolean;
}

export type AppPreferencePatch = Partial<AppPreferences>;

export const APP_PREFERENCES_STORAGE_KEY = "interface.settings.v1";
export const APP_PREFERENCES_VERSION = 1;

export const DEFAULT_APP_PREFERENCES: Readonly<AppPreferences> = Object.freeze({
  uiFont: "sans",
  // The shipped harness stays the default; CLI is opt-in.
  uiChrome: "refined",
  // The custom composer is what production shows; the native prompt is opt-in.
  cleanInput: "composer",
  lastCliTheme: DEFAULT_CLI_THEME,
  lastWorkspaceTheme: "system",
  // Notifications were always on, with a sound, before they had settings.
  notifySound: true,
  notifyFinished: true,
  notifyNeedsInput: true,
});

export function isUiFontPreference(value: unknown): value is UiFontPreference {
  return typeof value === "string"
    && (UI_FONT_PREFERENCES as readonly string[]).includes(value);
}

export function isUiChromePreference(value: unknown): value is UiChromePreference {
  return typeof value === "string"
    && (UI_CHROME_PREFERENCES as readonly string[]).includes(value);
}

export function isCleanInputPreference(value: unknown): value is CleanInputPreference {
  return typeof value === "string"
    && (CLEAN_INPUT_PREFERENCES as readonly string[]).includes(value);
}

interface PreferenceEnvelope extends AppPreferences {
  version: typeof APP_PREFERENCES_VERSION;
}

export interface PreferenceStorageReader {
  getItem(key: string): string | null;
}

export interface PreferenceStorageWriter {
  setItem(key: string, value: string): void;
}

/**
 * Normalize preferences field-by-field. Corruption in one value does not make
 * the other valid values disappear, and unknown future fields are ignored —
 * as are the retired editor fields (`editorFontSize`, `editorWordWrap`,
 * `editorMinimap`) older builds stored before the app had no editor.
 */
export function normalizeAppPreferences(value: unknown): AppPreferences {
  const record = asRecord(value);
  if (record === null) return { ...DEFAULT_APP_PREFERENCES };
  return {
    uiFont: isUiFontPreference(record.uiFont)
      ? record.uiFont : DEFAULT_APP_PREFERENCES.uiFont,
    uiChrome: isUiChromePreference(record.uiChrome)
      ? record.uiChrome : DEFAULT_APP_PREFERENCES.uiChrome,
    ...(record.themeMotion === "off" || record.themeMotion === "plugged"
      ? { themeMotion: record.themeMotion } : {}),
    ...(record.privacySummarySeen === true ? { privacySummarySeen: true as const } : {}),
    cleanInput: isCleanInputPreference(record.cleanInput)
      ? record.cleanInput : DEFAULT_APP_PREFERENCES.cleanInput,
    lastCliTheme: isCliThemePreference(record.lastCliTheme)
      ? record.lastCliTheme : DEFAULT_APP_PREFERENCES.lastCliTheme,
    lastWorkspaceTheme: isCustomThemeId(record.lastWorkspaceTheme) ? record.lastWorkspaceTheme
      : canonicalThemePreference(record.lastWorkspaceTheme) ?? DEFAULT_APP_PREFERENCES.lastWorkspaceTheme,
    notifySound: typeof record.notifySound === "boolean"
      ? record.notifySound : DEFAULT_APP_PREFERENCES.notifySound,
    notifyFinished: typeof record.notifyFinished === "boolean"
      ? record.notifyFinished : DEFAULT_APP_PREFERENCES.notifyFinished,
    notifyNeedsInput: typeof record.notifyNeedsInput === "boolean"
      ? record.notifyNeedsInput : DEFAULT_APP_PREFERENCES.notifyNeedsInput,
  };
}

/** Settings › Notifications, as the notification helpers read them. */
export function notificationPreferencesOf(preferences: AppPreferences): NotificationPreferences {
  return {
    sound: preferences.notifySound,
    finished: preferences.notifyFinished,
    needsInput: preferences.notifyNeedsInput,
  };
}

/** Parse a localStorage value without ever allowing corrupt state to break startup. */
export function parseAppPreferences(raw: string | null | undefined): AppPreferences {
  if (raw === null || raw === undefined || raw.trim() === "") {
    return { ...DEFAULT_APP_PREFERENCES };
  }
  try {
    return normalizeAppPreferences(JSON.parse(raw) as unknown);
  } catch {
    return { ...DEFAULT_APP_PREFERENCES };
  }
}

/** Stable versioned serialization so later settings migrations have an anchor. */
export function serializeAppPreferences(preferences: AppPreferences): string {
  const normalized = normalizeAppPreferences(preferences);
  const envelope: PreferenceEnvelope = {
    version: APP_PREFERENCES_VERSION,
    ...normalized,
  };
  return JSON.stringify(envelope);
}

/** Pure, runtime-safe preference update used by every Settings control. */
export function updateAppPreferences(
  current: AppPreferences,
  patch: AppPreferencePatch,
): AppPreferences {
  return normalizeAppPreferences({ ...current, ...patch });
}

/** Storage access stays explicit and injectable; tests never need a browser global. */
export function readAppPreferences(storage: PreferenceStorageReader): AppPreferences {
  try {
    return parseAppPreferences(storage.getItem(APP_PREFERENCES_STORAGE_KEY));
  } catch {
    return { ...DEFAULT_APP_PREFERENCES };
  }
}

/** Returns false for quota/privacy failures while leaving the in-memory setting usable. */
export function writeAppPreferences(
  storage: PreferenceStorageWriter,
  preferences: AppPreferences,
): boolean {
  try {
    storage.setItem(APP_PREFERENCES_STORAGE_KEY, serializeAppPreferences(preferences));
    return true;
  } catch {
    return false;
  }
}

export type ProviderHealthState = "checking" | "ready" | "signedOut" | "unavailable";

/** The engines whose CLI has a sign-in command Interface can open in a Code
 * terminal (Rust owns the argv: `claude auth login`, `codex login`, `grok login`). */
const SIGN_IN_PROFILES: Readonly<Record<string, "claude" | "codex" | "grok">> = {
  claude: "claude", codex: "codex", "acp:grok": "grok",
};

/** The terminal launch profile that signs `engine` in, when it has one. */
export function signInProfile(engine: EngineId): "claude" | "codex" | "grok" | null {
  return Object.hasOwn(SIGN_IN_PROFILES, engine) ? SIGN_IN_PROFILES[engine] ?? null : null;
}

/** Safe display shape derived from an engine probe; it never contains credentials. */
export interface ProviderSettingsSummary {
  engine: EngineId;
  label: string;
  health: ProviderHealthState;
  healthLabel: "Checking" | "Ready" | "Signed out" | "Unavailable";
  /** Signed out, with a sign-in flow Interface can open (Settings' Sign in). */
  signIn: boolean;
  version: string | null;
  modelNames: readonly string[];
  modelSummary: string;
  loginIdentity: string | null;
  permissionModes: readonly string[];
  effortLevels: readonly string[];
  /** What is wrong and what to do, when the probe's failure is recognized; else its own text. */
  error: string | null;
  /** The probe's own text, kept beside a translated `error`; null when `error` is it. */
  errorDetail: string | null;
}

/**
 * A provider probe's failure as a sentence that says what to do, for the
 * failures that have one. The host passes the CLI's own words through
 * unchanged ("initialize: Codex app-server closed stdout"); they stay in the
 * details. Unrecognized text returns null and is shown as it is. Refresh
 * finds a CLI installed (or updated) since Interface started (protocol 41).
 * A probe that answered says "signed out" itself (`EngineInfo.auth`); these
 * sentences cover a CLI that failed with the words instead.
 */
export function providerFaultSentence(label: string, raw: string): string | null {
  const text = raw.toLowerCase();
  if (/executable could not be started|could not start (acp agent|codex app-server)|no such file or directory|os error 2\b|enoent/u.test(text)) {
    return `${label} is not installed, or not where Interface looks for it. Install the ${label} CLI, then press Refresh.`;
  }
  if (/not logged in|not signed in|log ?in required|sign ?in required|unauthori[sz]ed|authenticat/u.test(text)) {
    return `${label} is not signed in. Run ${label} in Terminal and sign in, then press Refresh.`;
  }
  if (/unsupported protocol version/u.test(text)) {
    return `This ${label} version is not supported. Update the ${label} CLI, then press Refresh.`;
  }
  if (/timed out/u.test(text)) {
    return `${label} did not answer in time. Check that ${label} starts in Terminal, then press Refresh.`;
  }
  if (/closed stdout|exited with|exited unsuccessfully|returned no version|stdout read failed/u.test(text)) {
    return `${label} stopped before it answered. Run ${label} in Terminal to see why (often a sign-in or update prompt), then press Refresh.`;
  }
  return null;
}

const uniqueModelNames = (probe: EngineProbe): string[] => {
  const advertised = probe.info?.modelCatalog;
  if (advertised !== undefined && advertised.length > 0) {
    const seenValues = new Set<string>();
    const usedLabels = new Set<string>();
    const names: string[] = [];
    for (const model of advertised) {
      const value = model.value.trim();
      if (value === "" || value === "default" || seenValues.has(value)) continue;
      seenValues.add(value);
      const displayName = model.displayName.trim() || value;
      const label = usedLabels.has(displayName)
        ? `${displayName} · ${value}` : displayName;
      usedLabels.add(label);
      names.push(label);
    }
    return names;
  }

  const candidates = Array.isArray(probe.info?.capabilities.models)
    ? probe.info.capabilities.models.map(model => model.trim()) : [];
  const seen = new Set<string>();
  const names: string[] = [];
  for (const candidate of candidates) {
    if (candidate === "" || candidate === "default" || seen.has(candidate)) continue;
    seen.add(candidate);
    names.push(candidate);
  }
  return names;
};

/** One truthful provider row, including the best model catalogue the probe supplied. */
export function summarizeProviderProbe(probe: EngineProbe): ProviderSettingsSummary {
  const modelNames = uniqueModelNames(probe);
  const dynamic = probe.info?.capabilities.models === "dynamic";
  const checking = !probe.ok && probe.error === undefined;
  const modelSummary = modelNames.length > 0
    ? `${modelNames.length} model${modelNames.length === 1 ? "" : "s"} available`
    : dynamic
      ? "Dynamic model catalog"
      : probe.ok ? "Model catalog not reported"
        : checking ? "Checking model catalog" : "Models unavailable";
  const raw = probe.ok || checking
    ? null : probe.error?.trim() || "Provider did not respond.";
  const label = providerLabel(probe.engine, probe.info?.displayName);
  // The CLI answered and says, by its own status command, nobody is signed in.
  const signedOut = probe.ok && probe.info?.auth === "signedOut";
  const sentence = signedOut
    ? signedOutSentence(label, signInProfile(probe.engine) !== null)
    : raw === null ? null : providerFaultSentence(label, raw);
  return {
    engine: probe.engine,
    label,
    health: signedOut ? "signedOut" : probe.ok ? "ready" : checking ? "checking" : "unavailable",
    healthLabel: signedOut ? "Signed out" : probe.ok ? "Ready" : checking ? "Checking" : "Unavailable",
    signIn: signedOut && signInProfile(probe.engine) !== null,
    version: probe.version?.trim() || probe.info?.version?.trim() || null,
    modelNames,
    modelSummary,
    loginIdentity: probe.info?.loginIdentity?.trim() || null,
    permissionModes: [...new Set(probe.info?.capabilities.permissionModes ?? [])],
    effortLevels: [...new Set(probe.info?.capabilities.effortLevels ?? [])],
    error: sentence ?? raw,
    errorDetail: sentence === null ? null : raw,
  };
}

/** One launch profile as Settings' Terminal tools shows it (review C4): the
 * same tools the New workspace launcher offers, with what Rust found. */
export interface TerminalToolSummary {
  id: TerminalLaunchProfile["id"];
  label: string;
  /** The engine whose logo and sign-in state it shares; null for Terminal. */
  engine: EngineId | null;
  health: "ready" | "signedOut" | "missing";
  healthLabel: "Found" | "Signed out" | "Not installed";
  /** "Version 2.1.300 (Claude Code)", "Your login shell", or null. */
  detail: string | null;
  /** "Signed in" / "Signed out" where the engine's own status says so. */
  auth: string | null;
  /** Signed out, with a sign-in flow Interface can open. */
  signIn: boolean;
  /** Why it is missing and how to install it, for a missing CLI. */
  missing: string | null;
  /** Protocol 48: the switch for Interface's tools in this CLI's own config
   * (Grok and Antigravity), or null when it keeps no such entry. */
  paneTools: TerminalPaneToolsSwitch | null;
  /** Protocol 51: "Changed since last check: 2.1.300 → 2.1.301. …", or null. */
  changed: string | null;
}

/** Protocol 51: what the provider canary found since it last checked a CLI,
 * as one line. Commands in it are set as code (backticks). */
export function toolChangeText(change: TerminalLaunchProfile["changedSinceLastCheck"]): string | null {
  if (change === undefined) return null;
  const head = change.from === undefined
    ? `Checked ${change.to} against what Interface depends on`
    : `Changed since last check: ${change.from} → ${change.to}`;
  const body = change.findings.length === 0
    ? "What Interface depends on still matches."
    : change.findings.join(" ");
  return `${head}. ${body}`;
}

/** Settings' switch for one CLI's use of Interface's tools. Off is the
 * default: Interface writes the CLI's `iface` entry only when it is on. */
export interface TerminalPaneToolsSwitch {
  /** "Let Grok use Interface's tools". */
  label: string;
  on: boolean;
  /** Taken or unreadable: the switch cannot change anything. */
  locked: boolean;
  note: string;
}

export function paneToolsSwitch(label: string,
    state: TerminalLaunchProfile["paneTools"]): TerminalPaneToolsSwitch | null {
  if (state === undefined) return null;
  const name = `Let ${label} use Interface's tools`;
  if (state === "taken") {
    return { label: name, on: false, locked: true,
      note: `${label} already has its own MCP server named iface, so Interface leaves it alone.` };
  }
  if (state === "unknown") {
    return { label: name, on: false, locked: true,
      note: `Interface could not read ${label}'s MCP servers. Press Refresh to try again.` };
  }
  return { label: name, on: state === "on", locked: false,
    note: `Pass work to a new pane and read or type into the pane at the other end. On adds an iface server to ${label}'s own MCP config; off removes it.` };
}

/** Settings' Terminal tools: every launch profile, joined with the engine
 * probes for sign-in state. Rust found the CLIs and their versions. */
export function summarizeTerminalTools(
  profiles: readonly TerminalLaunchProfile[],
  probes: readonly EngineProbe[],
): TerminalToolSummary[] {
  return profiles.map(profile => {
    const engine = profile.engine ?? null;
    const probe = engine === null ? undefined : probes.find(candidate => candidate.engine === engine);
    const auth = probe?.ok === true ? probe.info?.auth : undefined;
    const signedOut = profile.available && auth === "signedOut";
    const version = profile.version?.trim();
    return {
      id: profile.id,
      label: profile.label,
      engine,
      health: !profile.available ? "missing" : signedOut ? "signedOut" : "ready",
      healthLabel: !profile.available ? "Not installed" : signedOut ? "Signed out" : "Found",
      detail: engine === null ? "Your login shell"
        : profile.available && version !== undefined && version !== "" ? `Version ${version}` : null,
      auth: !profile.available || auth === undefined ? null
        : auth === "signedIn" ? "Signed in" : "Signed out",
      signIn: signedOut && engine !== null && signInProfile(engine) !== null,
      // The pill already says "Not installed": the row says how to fix it.
      missing: profile.available ? null
        : profile.installHint?.trim() || profile.unavailableReason?.trim()
          || "Not found. Install it, then press Refresh.",
      paneTools: profile.available ? paneToolsSwitch(profile.label, profile.paneTools) : null,
      changed: profile.available ? toolChangeText(profile.changedSinceLastCheck) : null,
    };
  });
}

/** What Settings and the sidebar say about a CLI that is signed out. */
export function signedOutSentence(label: string, canSignIn: boolean): string {
  return canSignIn
    ? `${label} is not signed in. Use Sign in, then press Refresh.`
    : `${label} is not signed in. Run ${label} in Terminal and sign in, then press Refresh.`;
}

/** Preserve adapter registration order; that is the app's provider order everywhere else. */
export function summarizeProviderProbes(
  probes: readonly EngineProbe[],
  configuredLabels: Readonly<Record<string, string>> = {},
): ProviderSettingsSummary[] {
  return probes.map(probe => {
    const summary = summarizeProviderProbe(probe);
    const configured = configuredLabels[probe.engine]?.trim();
    return configured ? { ...summary, label: configured } : summary;
  });
}

export interface UpdateSettingsSummary {
  currentVersion: string;
  availableVersion: string | null;
  statusLabel: string;
  description: string;
  progressPercent: number | null;
  progressLabel: string | null;
  lastCheckedLabel: string;
  error: string | null;
  busy: boolean;
  ready: boolean;
}

/** Human-readable wall-clock time while retaining the ISO value in the UI's time element. */
export function formatUpdateLastChecked(value: string | null): string {
  if (value === null || value.trim() === "") return "Not checked yet";
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "Unknown";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(timestamp);
}

/**
 * Turn updater transport state into calm product copy and bounded progress.
 * The renderer still gates actions on the authoritative `canCheck` and
 * `canRestart` flags; this helper never invents updater capabilities.
 */
export function summarizeUpdateState(state: UpdateState): UpdateSettingsSummary {
  const currentVersion = state.currentVersion.trim() || "Unknown";
  const availableVersion = state.availableVersion?.trim() || null;
  const target = availableVersion === null ? "the update" : `version ${availableVersion}`;
  const rawProgress = state.progress;
  const progressPercent = rawProgress === null || !Number.isFinite(rawProgress)
    ? null
    : Math.min(100, Math.max(0, Math.round(rawProgress)));

  let statusLabel: string;
  let description: string;
  switch (state.phase) {
    case "disabled":
      statusLabel = "Updates unavailable";
      description = "This build is not connected to an automatic update channel.";
      break;
    case "idle":
      statusLabel = "Ready to check";
      description = `Check for a newer signed ${productLabel()} release.`;
      break;
    case "checking":
      statusLabel = "Checking for updates";
      description = `Looking for a newer signed ${productLabel()} release…`;
      break;
    case "available":
      statusLabel = "Update available";
      description = `${target} is available and will download automatically.`;
      break;
    case "downloading":
      statusLabel = "Downloading update";
      description = `Downloading ${target} in the background…`;
      break;
    case "ready":
      statusLabel = "Ready to restart";
      description = `${target} is downloaded. Restart ${productLabel()} to finish installing it.`;
      break;
    case "installing":
      statusLabel = "Installing update";
      description = `Handing ${target} to macOS. ${productLabel()} will reopen when installation finishes.`;
      break;
    case "up-to-date":
      statusLabel = `${productLabel()} is up to date`;
      description = "You are running the newest available version.";
      break;
    case "error":
      statusLabel = "Update check failed";
      description = `${productLabel()} could not complete the update check.`;
      break;
  }

  return {
    currentVersion,
    availableVersion,
    statusLabel,
    description,
    progressPercent,
    progressLabel: state.phase === "downloading"
      ? progressPercent === null ? "Preparing download" : `${progressPercent}% downloaded`
      : null,
    lastCheckedLabel: formatUpdateLastChecked(state.lastCheckedAt),
    error: state.error?.trim() || null,
    busy: state.phase === "checking" || state.phase === "downloading"
      || state.phase === "installing",
    ready: state.phase === "ready",
  };
}
