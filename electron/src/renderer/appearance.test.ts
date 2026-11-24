import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import type { AppearanceState, ResolvedTheme, SurfaceTranslucency } from "../shared/appearance";
import type { AppearanceApi, AppearanceSnapshot } from "./appearance-controller";

/* The launch sequence without a DOM: main.tsx paints the host's appearance
   before it renders, App's first render takes useAppearance's initial snapshot,
   and the hook's effect starts the controller, which publishes and then
   confirms with a second read. Vitest here is node-only, so the document is a
   recorder and each step is driven directly; `useAppearance` is pinned below
   to be nothing more than those two calls. */

const rendered = vi.hoisted(() => ({
  firstSnapshot: null as unknown,
  readFirstSnapshot: null as null | (() => unknown),
}));

vi.mock("./App", () => ({ App: () => null }));
vi.mock("react-dom/client", () => ({
  createRoot: () => ({
    // What App's useState initialiser reads when main.tsx renders it.
    render: () => { rendered.firstSnapshot = rendered.readFirstSnapshot?.() ?? null; },
  }),
}));

interface Paints {
  /** Every value written to each root attribute or style property, in order. */
  writes: Map<string, string[]>;
  /** How many times mounted terminals were told to re-read their theme. */
  themeChanges: number;
}

function recordingDocument(): Paints {
  const paints: Paints = { writes: new Map(), themeChanges: 0 };
  const record = (name: string, value: string): void => {
    paints.writes.set(name, [...paints.writes.get(name) ?? [], value]);
  };
  const dataset = new Proxy<Record<string, string>>({}, {
    set(target, key, value: string) {
      record(`dataset.${String(key)}`, value);
      target[String(key)] = value;
      return true;
    },
  });
  const style = {
    set colorScheme(value: string) { record("color-scheme", value); },
    setProperty(name: string, value: string) { record(name, value); },
  };
  const documentElement = {
    dataset, style,
    dispatchEvent(event: Event) {
      if (event.type === "interface-theme-change") paints.themeChanges++;
      return true;
    },
  };
  const document = Object.assign(new EventTarget(), { documentElement, getElementById: () => ({}) });
  vi.stubGlobal("document", document);
  vi.stubGlobal("window", Object.assign(new EventTarget(), { document }));
  return paints;
}

const surfaces = (terminal: number, sidebar = 0): SurfaceTranslucency => ({ version: 1, terminal, sidebar });
const stored = (resolved: ResolvedTheme): AppearanceState => ({
  preference: resolved, resolved, translucency: surfaces(40, 25),
  workspacePresentation: { version: 2, terminalFontSize: 18, pinnedWorkspaces: ["workspace-a"] },
  projectPresentation: { version: 1, pinnedProjects: ["/projects/Example"] },
});
const host = (state: AppearanceState): AppearanceApi => ({
  getAppearance: async () => structuredClone(state),
  setAppearance: async () => { throw new Error("no writes in this test"); },
});

beforeEach(() => { vi.resetModules(); rendered.firstSnapshot = null; rendered.readFirstSnapshot = null; });
afterEach(() => { vi.unstubAllGlobals(); });

describe("launch appearance", () => {
  for (const theme of ["dark", "catppuccin-mocha", "grok-build", "anthropic-light"] as const) {
    it(`paints ${theme} before mount and never repaints anything else on the way to the confirmed read`, async () => {
      const paints = recordingDocument();
      const state = stored(theme);
      Object.assign(window, { iface: host(state) });
      // The instance main.tsx imports, since the registry was just reset.
      const { firstAppearanceSnapshot, startAppearance } = await import("./appearance");
      rendered.readFirstSnapshot = firstAppearanceSnapshot;
      await import("./main");
      await vi.waitFor(() => { expect(rendered.firstSnapshot).not.toBeNull(); });

      // main.tsx: the host's answer is on the document before App renders, and
      // App's first render starts from it rather than from the light sentinel.
      expect(paints.writes.get("dataset.theme")).toEqual([theme]);
      expect(rendered.firstSnapshot).toEqual({ appearance: state, pending: true, error: null });

      // useAppearance's effect: the first publish and the confirming read.
      const published: AppearanceSnapshot[] = [];
      const controller = startAppearance(host(state), snapshot => { published.push(snapshot); });
      await controller.settled();

      expect(published.map(snapshot => snapshot.appearance)).toEqual([state, state]);
      expect(published.map(snapshot => snapshot.pending)).toEqual([true, false]);
      // Three paints (pre-mount, first publish, confirmation), all the same:
      // no light frame, no solid-material frame, no default text size.
      expect(paints.writes.get("dataset.theme")).toEqual([theme, theme, theme]);
      expect(paints.writes.get("dataset.terminalGlass")).toEqual(["true", "true", "true"]);
      expect(paints.writes.get("dataset.sidebarGlass")).toEqual(["true", "true", "true"]);
      expect(paints.writes.get("--workspace-terminal-font-size")).toEqual(["18", "18", "18"]);
      // Terminals mount after the first paint and read their own theme then;
      // repeating the same appearance re-themes none of them.
      expect(paints.themeChanges).toBe(1);
    });
  }

  it("starts from the light sentinel only when the pre-mount read failed", async () => {
    const paints = recordingDocument();
    const { firstAppearanceSnapshot, startAppearance } = await import("./appearance");
    const { INITIAL_APPEARANCE } = await import("./appearance-controller");
    expect(firstAppearanceSnapshot()).toBe(INITIAL_APPEARANCE);
    const controller = startAppearance(host(stored("dark")), () => undefined);
    await controller.settled();
    expect(paints.writes.get("dataset.theme")).toEqual(["light", "dark"]);
    expect(paints.themeChanges).toBe(2);
  });

  it("publishes a snapshot equal to the last one to nobody", async () => {
    recordingDocument();
    const { startAppearance } = await import("./appearance");
    const published: AppearanceSnapshot[] = [];
    const controller = startAppearance(host(stored("dark")), snapshot => { published.push(snapshot); });
    await controller.settled();
    expect(published.map(snapshot => snapshot.pending)).toEqual([true, false]);
    // Two refreshes in a row (focus, then a colour-scheme change): the queued
    // and in-between states repeat the one already shown and render nothing.
    controller.refresh();
    controller.refresh();
    await controller.settled();
    expect(published.map(snapshot => snapshot.pending)).toEqual([true, false, true, false]);
  });

  it("is what useAppearance runs: its first snapshot and its mount", () => {
    const source = readFileSync(new URL("./appearance.ts", import.meta.url), "utf8");
    expect(source).toContain("useState(firstAppearanceSnapshot)");
    expect(source).toContain("const current = startAppearance(api, setSnapshot);");
    const main = readFileSync(new URL("./main.tsx", import.meta.url), "utf8");
    expect(main).toContain("window.iface.getAppearance().then(applyAppearance,");
  });
});

describe("terminal re-theme", () => {
  it("is sent only when the palette, terminal glass or text size changes", async () => {
    const paints = recordingDocument();
    const { applyAppearance } = await import("./appearance");
    const base: AppearanceState = {
      preference: "dark", resolved: "dark", translucency: surfaces(0),
      workspacePresentation: { version: 2, terminalFontSize: 14, pinnedWorkspaces: [] },
      projectPresentation: { version: 1, pinnedProjects: [] },
    };
    applyAppearance(base);
    expect(paints.themeChanges).toBe(1);

    // Nothing a terminal reads: pins, sidebar material, System resolving the same.
    applyAppearance({ ...base, projectPresentation: { version: 1, pinnedProjects: ["/projects/Example"] } });
    applyAppearance({ ...base, workspacePresentation: { version: 2, terminalFontSize: 14, pinnedWorkspaces: ["w"] } });
    applyAppearance({ ...base, translucency: surfaces(0, 60) });
    applyAppearance({ ...base, preference: "system" });
    expect(paints.themeChanges).toBe(1);
    // The document itself is still painted every time.
    expect(paints.writes.get("--sidebar-surface-opacity")).toEqual(["100%", "100%", "100%", "49%", "100%"]);

    applyAppearance({ ...base, translucency: surfaces(20) });
    expect(paints.themeChanges).toBe(2);
    // A later slider position keeps glass on: new opacity, same xterm theme.
    applyAppearance({ ...base, translucency: surfaces(60) });
    expect(paints.themeChanges).toBe(2);
    const [lighter, clearer] = paints.writes.get("--terminal-surface-opacity")!.slice(-2);
    expect(lighter).toBe("84.4%");
    expect(clearer).not.toBe(lighter);

    applyAppearance({ ...base, translucency: surfaces(60), workspacePresentation: { version: 2, terminalFontSize: 16, pinnedWorkspaces: [] } });
    expect(paints.themeChanges).toBe(3);
    applyAppearance({ ...base, resolved: "catppuccin-mocha", translucency: surfaces(60), workspacePresentation: { version: 2, terminalFontSize: 16, pinnedWorkspaces: [] } });
    expect(paints.themeChanges).toBe(4);
    applyAppearance({ ...base, resolved: "catppuccin-mocha", workspacePresentation: { version: 2, terminalFontSize: 16, pinnedWorkspaces: [] } });
    expect(paints.themeChanges).toBe(5);
    expect(paints.writes.get("dataset.theme")).toHaveLength(10);
  });
});
