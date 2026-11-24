import { describe, expect, it, vi } from "vitest";
import type { EngineCapabilities, EngineInfo } from "../shared/engine";
import type { EngineProbe } from "../shared/ipc";
import type { UpdateState } from "../shared/updater";
import { applyAppearancePreferences, initializeAppPreferences } from "./appearance";
import {
  APP_PREFERENCES_STORAGE_KEY,
  APP_PREFERENCES_VERSION,
  CLEAN_INPUT_PREFERENCES,
  DEFAULT_APP_PREFERENCES,
  formatUpdateLastChecked,
  isCleanInputPreference,
  normalizeAppPreferences,
  notificationPreferencesOf,
  parseAppPreferences,
  providerFaultSentence,
  readAppPreferences,
  serializeAppPreferences,
  signInProfile,
  summarizeProviderProbe,
  summarizeProviderProbes,
  summarizeUpdateState,
  updateAppPreferences,
  writeAppPreferences,
} from "./settings";

const updateState = (overrides: Partial<UpdateState> = {}): UpdateState => ({
  phase: "idle",
  source: "remote",
  currentVersion: "0.1.1",
  availableVersion: null,
  progress: null,
  lastCheckedAt: null,
  error: null,
  canCheck: true,
  canRestart: false,
  ...overrides,
});

const capabilities = (models: EngineCapabilities["models"]): EngineCapabilities => ({
  streaming: true,
  approvals: true,
  resume: true,
  fork: false,
  cloudSessions: false,
  models,
  effortLevels: [],
  permissionModes: [],
});

const info = (
  models: EngineCapabilities["models"],
  modelCatalog?: EngineInfo["modelCatalog"],
): EngineInfo => ({
  binaryPath: "/usr/local/bin/provider",
  version: "1.2.3",
  loginIdentity: null,
  capabilities: capabilities(models),
  ...(modelCatalog === undefined ? {} : { modelCatalog }),
});

describe("app preference persistence", () => {
  it("returns complete defaults for empty, malformed, and non-object input", () => {
    expect(parseAppPreferences(null)).toEqual(DEFAULT_APP_PREFERENCES);
    expect(parseAppPreferences("{not json")).toEqual(DEFAULT_APP_PREFERENCES);
    expect(parseAppPreferences("[]")).toEqual(DEFAULT_APP_PREFERENCES);
    expect(parseAppPreferences("null")).toEqual(DEFAULT_APP_PREFERENCES);
  });

  it("keeps supported valid fields and ignores the retired worktree default", () => {
    expect(parseAppPreferences(JSON.stringify({
      version: 77,
      defaultWorktree: true,
      uiFont: "comic",
      uiChrome: "neon",
      futureSetting: "ignored",
    }))).toEqual({
      uiFont: DEFAULT_APP_PREFERENCES.uiFont,
      uiChrome: DEFAULT_APP_PREFERENCES.uiChrome,
      cleanInput: DEFAULT_APP_PREFERENCES.cleanInput,
      lastCliTheme: DEFAULT_APP_PREFERENCES.lastCliTheme,
      lastWorkspaceTheme: DEFAULT_APP_PREFERENCES.lastWorkspaceTheme,
      notifySound: true,
      notifyFinished: true,
      notifyNeedsInput: true,
    });
  });

  it("keeps the three notification settings, on by default, and ignores non-booleans", () => {
    expect(DEFAULT_APP_PREFERENCES).toMatchObject({
      notifySound: true, notifyFinished: true, notifyNeedsInput: true,
    });
    const read = parseAppPreferences(JSON.stringify({
      notifySound: false, notifyFinished: "no", notifyNeedsInput: false,
    }));
    expect(read).toMatchObject({ notifySound: false, notifyFinished: true, notifyNeedsInput: false });
    expect(notificationPreferencesOf(read)).toEqual({ sound: false, finished: true, needsInput: false });
  });

  it("reads a store that still holds the retired editor settings and drops them", () => {
    // Builds before 2026-10-14 kept editor text size, wrap and minimap for an
    // editor the app no longer has. Their stored values must not break a read,
    // must not change any other setting, and are not written back.
    const stored = JSON.stringify({
      version: APP_PREFERENCES_VERSION,
      uiFont: "serif",
      uiChrome: "refined",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "hermes",
      editorFontSize: 16,
      editorWordWrap: true,
      editorMinimap: false,
    });
    const read = parseAppPreferences(stored);
    expect(read).toEqual({
      ...DEFAULT_APP_PREFERENCES, uiFont: "serif", lastWorkspaceTheme: "hermes",
    });
    expect(Object.keys(read).some(key => key.startsWith("editor"))).toBe(false);
    expect(Object.keys(JSON.parse(serializeAppPreferences(read)) as object)
      .some(key => key.startsWith("editor"))).toBe(false);
    expect(parseAppPreferences(JSON.stringify({ editorFontSize: Infinity })))
      .toEqual(DEFAULT_APP_PREFERENCES);
  });

  it("serializes a normalized versioned envelope in a stable shape", () => {
    const raw = serializeAppPreferences({
      uiFont: "serif",
      uiChrome: "cli",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "system",
      notifySound: false,
      notifyFinished: true,
      notifyNeedsInput: false,
    });
    expect(JSON.parse(raw)).toEqual({
      version: APP_PREFERENCES_VERSION,
      uiFont: "serif",
      uiChrome: "cli",
      cleanInput: "composer",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "system",
      notifySound: false,
      notifyFinished: true,
      notifyNeedsInput: false,
    });
    expect(parseAppPreferences(raw)).toEqual({
      uiFont: "serif",
      uiChrome: "cli",
      cleanInput: "composer",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "system",
      notifySound: false,
      notifyFinished: true,
      notifyNeedsInput: false,
    });
  });

  it("keeps remembered CLI and workspace palettes when they are valid", () => {
    expect(parseAppPreferences(JSON.stringify({
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "hermes",
    }))).toEqual({
      ...DEFAULT_APP_PREFERENCES,
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "hermes",
    });
    // Retired palettes land on their replacements (owner, 2026-09-28).
    expect(parseAppPreferences(JSON.stringify({
      lastCliTheme: "nightshade",
      lastWorkspaceTheme: "warm-editorial-dark",
    }))).toEqual({
      ...DEFAULT_APP_PREFERENCES,
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "anthropic-dark",
    });
    expect(parseAppPreferences(JSON.stringify({
      lastCliTheme: "hermes",
      lastWorkspaceTheme: "not-a-theme",
    }))).toEqual(DEFAULT_APP_PREFERENCES);
  });

  it("updates immutably and normalizes runtime input", () => {
    const current = { ...DEFAULT_APP_PREFERENCES };
    const next = updateAppPreferences(current, {
      uiFont: "serif",
      uiChrome: "neon" as unknown as "cli",
    });
    expect(next).toEqual({ ...DEFAULT_APP_PREFERENCES, uiFont: "serif" });
    expect(current).toEqual(DEFAULT_APP_PREFERENCES);
    expect(next).not.toBe(current);
  });

  it("reads and writes through an injectable storage boundary", () => {
    const getItem = vi.fn(() => JSON.stringify({ uiChrome: "cli" }));
    expect(readAppPreferences({ getItem })).toEqual({
      ...DEFAULT_APP_PREFERENCES,
      uiChrome: "cli",
    });
    expect(parseAppPreferences(JSON.stringify({ uiFont: "serif" }))).toEqual({
      ...DEFAULT_APP_PREFERENCES,
      uiFont: "serif",
    });
    expect(getItem).toHaveBeenCalledWith(APP_PREFERENCES_STORAGE_KEY);

    const setItem = vi.fn();
    expect(writeAppPreferences({ setItem }, { ...DEFAULT_APP_PREFERENCES })).toBe(true);
    expect(setItem).toHaveBeenCalledWith(
      APP_PREFERENCES_STORAGE_KEY,
      serializeAppPreferences({ ...DEFAULT_APP_PREFERENCES }),
    );
  });

  it("falls back safely when storage is blocked", () => {
    expect(readAppPreferences({ getItem: () => { throw new Error("blocked"); } }))
      .toEqual(DEFAULT_APP_PREFERENCES);
    expect(writeAppPreferences({
      setItem: () => { throw new Error("quota"); },
    }, { ...DEFAULT_APP_PREFERENCES })).toBe(false);
  });
});

describe("provider settings summaries", () => {
  it("uses provider model metadata, disambiguates display names, and reports readiness", () => {
    const probe: EngineProbe = {
      engine: "claude",
      ok: true,
      version: " 2.0.0 ",
      info: info("dynamic", [
        { value: "default", displayName: "Default (recommended)", supportsEffort: false,
          effortLevels: [] },
        { value: "sonnet", displayName: "Claude Sonnet", supportsEffort: false,
          effortLevels: [] },
        { value: "sonnet-duplicate", displayName: "Claude Sonnet", supportsEffort: false,
          effortLevels: [] },
        { value: "opus", displayName: "Claude Opus", supportsEffort: true,
          effortLevels: ["high"] },
      ]),
    };
    expect(summarizeProviderProbe(probe)).toEqual({
      engine: "claude",
      label: "Claude Code",
      health: "ready",
      healthLabel: "Ready",
      signIn: false,
      version: "2.0.0",
      modelNames: ["Claude Sonnet", "Claude Sonnet · sonnet-duplicate", "Claude Opus"],
      modelSummary: "3 models available",
      loginIdentity: null,
      permissionModes: [],
      effortLevels: [],
      error: null,
      errorDetail: null,
    });
  });

  it("says what to do about a provider probe failure, keeping the CLI's own words", () => {
    expect(summarizeProviderProbe({
      engine: "codex", ok: false, error: "initialize: Codex app-server closed stdout",
    })).toMatchObject({
      health: "unavailable",
      error: "Codex stopped before it answered. Run Codex in Terminal to see why (often a sign-in or update prompt), then press Refresh.",
      errorDetail: "initialize: Codex app-server closed stdout",
    });
    expect(providerFaultSentence("Claude", "Claude executable could not be started"))
      .toBe("Claude is not installed, or not where Interface looks for it. Install the Claude CLI, then press Refresh.");
    expect(providerFaultSentence("Qwen", "could not start ACP agent: No such file or directory (os error 2)"))
      .toMatch(/^Qwen is not installed/u);
    expect(providerFaultSentence("Grok", "Grok version probe timed out")).toMatch(/did not answer in time/u);
    expect(providerFaultSentence("Codex", "initialize: timed out waiting for initialize response"))
      .toMatch(/did not answer in time/u);
    expect(providerFaultSentence("Codex", "initialize: Codex app-server exited with exit status: 1"))
      .toMatch(/stopped before it answered/u);
    expect(providerFaultSentence("Grok", "ACP agent selected an unsupported protocol version"))
      .toMatch(/version is not supported/u);
    expect(providerFaultSentence("Codex", "initialize: Not logged in. Run codex login."))
      .toMatch(/^Codex is not signed in/u);
    // Unrecognized text is shown as it is.
    expect(providerFaultSentence("Claude", "Claude adapter is disposed")).toBeNull();
    expect(summarizeProviderProbe({ engine: "acp:grok", ok: false, error: " CLI not found " }))
      .toMatchObject({ error: "CLI not found", errorDetail: null });
  });

  it("shows a provider its own CLI says is signed out as Signed out, with Sign in where it has one", () => {
    const signedOut = { ...info("dynamic"), auth: "signedOut" as const };
    expect(summarizeProviderProbe({ engine: "claude", ok: true, info: signedOut })).toMatchObject({
      health: "signedOut",
      healthLabel: "Signed out",
      signIn: true,
      error: "Claude Code is not signed in. Use Sign in, then press Refresh.",
      errorDetail: null,
    });
    expect(summarizeProviderProbe({ engine: "acp:grok", ok: true, info: signedOut }))
      .toMatchObject({ health: "signedOut", signIn: true });
    // No sign-in command Interface can open: say where to sign in instead.
    expect(summarizeProviderProbe({ engine: "acp:custom", ok: true, info: signedOut }))
      .toMatchObject({ health: "signedOut", signIn: false, error: expect.stringMatching(/Run .* in Terminal and sign in/u) });
    // Signed in, or a CLI that cannot say: Ready, as before.
    expect(summarizeProviderProbe({ engine: "codex", ok: true,
      info: { ...info("dynamic"), auth: "signedIn" as const } })).toMatchObject({
      health: "ready", healthLabel: "Ready", signIn: false, error: null,
    });
    expect(signInProfile("codex")).toBe("codex");
    expect(signInProfile("acp:qwen")).toBeNull();
  });

  it("falls back to capability models and distinguishes dynamic and unavailable catalogs", () => {
    expect(summarizeProviderProbe({
      engine: "codex", ok: true, info: info(["default", "gpt-5", "gpt-5-mini"]),
    }).modelSummary).toBe("2 models available");
    expect(summarizeProviderProbe({
      engine: "codex", ok: true, info: info("dynamic"),
    }).modelSummary).toBe("Dynamic model catalog");
    expect(summarizeProviderProbe({
      engine: "acp:grok", ok: false, error: " CLI not found ",
    })).toMatchObject({
      health: "unavailable",
      healthLabel: "Unavailable",
      modelSummary: "Models unavailable",
      error: "CLI not found",
    });
    expect(summarizeProviderProbe({ engine: "claude", ok: false })).toMatchObject({
      health: "checking",
      healthLabel: "Checking",
      modelSummary: "Checking model catalog",
      error: null,
    });
  });

  it("preserves adapter registration order", () => {
    const probes: EngineProbe[] = [
      { engine: "codex", ok: true },
      { engine: "claude", ok: false },
      { engine: "acp:grok", ok: true },
    ];
    expect(summarizeProviderProbes(probes).map(summary => summary.engine))
      .toEqual(["codex", "claude", "acp:grok"]);
  });

  it("uses provider and configured ACP display names without changing engine ids", () => {
    expect(summarizeProviderProbe({
      engine: "acp:local-agent", ok: true,
      info: { ...info("dynamic"), displayName: "Local Agent" },
    })).toMatchObject({ engine: "acp:local-agent", label: "Local Agent" });
    expect(summarizeProviderProbes([
      { engine: "acp:offline", ok: false, error: "not running" },
    ], { "acp:offline": "Offline Agent" })[0]).toMatchObject({
      engine: "acp:offline", label: "Offline Agent", health: "unavailable",
    });
  });
});

describe("update settings summaries", () => {
  it("gives every updater phase concise, distinct product copy", () => {
    const labels = ([
      "disabled", "idle", "checking", "available", "downloading", "ready",
      "installing", "up-to-date", "error",
    ] as const).map(phase => summarizeUpdateState(updateState({ phase })).statusLabel);
    expect(labels).toEqual([
      "Updates unavailable",
      "Ready to check",
      "Checking for updates",
      "Update available",
      "Downloading update",
      "Ready to restart",
      "Installing update",
      "Interface is up to date",
      "Update check failed",
    ]);
  });

  it("normalizes version, error, and download progress for display", () => {
    expect(summarizeUpdateState(updateState({
      phase: "downloading",
      currentVersion: " 0.1.1 ",
      availableVersion: " 0.2.0 ",
      progress: 42.6,
      error: " network paused ",
    }))).toMatchObject({
      currentVersion: "0.1.1",
      availableVersion: "0.2.0",
      progressPercent: 43,
      progressLabel: "43% downloaded",
      error: "network paused",
      busy: true,
      ready: false,
    });
    expect(summarizeUpdateState(updateState({ phase: "downloading", progress: -8 })))
      .toMatchObject({ progressPercent: 0, progressLabel: "0% downloaded" });
    expect(summarizeUpdateState(updateState({ phase: "downloading", progress: 140 })))
      .toMatchObject({ progressPercent: 100, progressLabel: "100% downloaded" });
  });

  it("formats last-check state safely and marks only the ready phase restartable in the UI", () => {
    expect(formatUpdateLastChecked(null)).toBe("Not checked yet");
    expect(formatUpdateLastChecked("not-a-date")).toBe("Unknown");
    const ready = summarizeUpdateState(updateState({
      phase: "ready",
      availableVersion: "0.2.0",
      lastCheckedAt: "2026-08-13T22:00:00.000Z",
    }));
    expect(ready.ready).toBe(true);
    expect(ready.busy).toBe(false);
    expect(ready.lastCheckedLabel).not.toBe("Unknown");
    expect(ready.lastCheckedLabel).not.toBe("Not checked yet");
    expect(summarizeUpdateState(updateState({ phase: "available" })).ready).toBe(false);
  });
});

/** A root-element stand-in; this suite has no DOM and does not want one. */
function withRootElement<T>(run: (dataset: Record<string, string>) => T): T {
  const dataset: Record<string, string> = {};
  const host = globalThis as unknown as { document?: unknown };
  const previous = host.document;
  host.document = { documentElement: { dataset } };
  try {
    return run(dataset);
  } finally {
    if (previous === undefined) delete host.document;
    else host.document = previous;
  }
}

describe("the legacy Clean input preference", () => {
  it("offers exactly two values and defaults to the composer", () => {
    expect(CLEAN_INPUT_PREFERENCES).toEqual(["composer", "native"]);
    expect(DEFAULT_APP_PREFERENCES.cleanInput).toBe("composer");
  });

  it("accepts only composer and native", () => {
    expect(isCleanInputPreference("composer")).toBe(true);
    expect(isCleanInputPreference("native")).toBe(true);
    for (const value of ["native ", "Native", "COMPOSER", "terminal", "clean", "",
      0, 1, true, null, undefined, {}, ["native"]]) {
      expect(isCleanInputPreference(value), JSON.stringify(value) ?? "undefined").toBe(false);
    }
  });

  it("treats an absent or unrecognised value as the composer", () => {
    // Settings written before this axis existed keep the shipped presentation.
    expect(parseAppPreferences('{"uiFont":"serif"}').cleanInput).toBe("composer");
    expect(parseAppPreferences('{"cleanInput":"terminal"}').cleanInput).toBe("composer");
    expect(parseAppPreferences('{"cleanInput":7}').cleanInput).toBe("composer");
    expect(normalizeAppPreferences({}).cleanInput).toBe("composer");
    // Corruption in this field must not take a valid neighbour with it.
    expect(parseAppPreferences('{"cleanInput":"terminal","uiFont":"serif"}'))
      .toEqual({ ...DEFAULT_APP_PREFERENCES, uiFont: "serif" });
  });

  it("keeps the native prompt once it is chosen, across a save and a reload", () => {
    expect(parseAppPreferences('{"cleanInput":"native"}').cleanInput).toBe("native");
    const next = updateAppPreferences({ ...DEFAULT_APP_PREFERENCES }, { cleanInput: "native" });
    expect(next).toEqual({ ...DEFAULT_APP_PREFERENCES, cleanInput: "native" });
    expect(parseAppPreferences(serializeAppPreferences(next)).cleanInput).toBe("native");
  });

  it("retains legacy settings without activating a Clean input surface", () => {
    const dataset = withRootElement(seen => {
      applyAppearancePreferences({ ...DEFAULT_APP_PREFERENCES, cleanInput: "native" });
      expect(seen.cleanInput).toBeUndefined();
      initializeAppPreferences({ getItem: () => JSON.stringify({ cleanInput: "composer", uiFont: "serif" }) });
      return seen;
    });
    expect(dataset.cleanInput).toBeUndefined();
    expect(dataset.font).toBe("serif");
  });
});
