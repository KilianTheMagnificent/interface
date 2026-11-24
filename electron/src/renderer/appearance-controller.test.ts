import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AppearanceController, SLIDER_COMMIT_DELAY_MS, SliderCommit, type AppearanceSnapshot,
} from "./appearance-controller";
import type { AppearanceChange, AppearanceState, SurfaceTranslucency } from "../shared/appearance";
const surfaces = (terminal: number, sidebar = 0): SurfaceTranslucency => ({ version: 1, terminal, sidebar });
const state = (terminal = 0, sidebar = 0): AppearanceState => ({ preference: "dark", resolved: "dark", translucency: surfaces(terminal, sidebar) });
const deferred = <T,>() => {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};
describe("appearance save ordering", () => {
  it("keeps independent pane and translucency choices when writes overlap", async () => {
    const first = deferred<AppearanceState>();
    let stored: AppearanceState = { ...state(), panePresentation: { version: 1, paneStyle: "terminal" } };
    const changes: AppearanceChange[] = [];
    let latest!: AppearanceSnapshot;
    const controller = new AppearanceController({
      getAppearance: async () => stored,
      setAppearance: async change => {
        changes.push(change);
        if (changes.length === 1) return first.promise;
        stored = typeof change === "string" ? { ...stored, preference: change }
          : "pinnedProjects" in change ? { ...stored, projectPresentation: change }
          : "terminalFontSize" in change ? { ...stored, workspacePresentation: change }
          : "paneStyle" in change ? { ...stored, panePresentation: change }
          : "light" in change ? { ...stored, systemThemes: change } : { ...stored, translucency: change };
        return stored;
      },
    }, value => { latest = value; });
    controller.refresh(); await controller.settled();
    controller.change(surfaces(20));
    controller.change({ version: 1, paneStyle: "clean" });
    controller.change(surfaces(60));
    expect(latest.appearance.panePresentation?.paneStyle).toBe("clean");
    expect(latest.appearance.translucency?.terminal).toBe(60);
    first.resolve({ ...stored, translucency: surfaces(20) });
    await controller.settled();
    expect(changes).toEqual([surfaces(20), { version: 1, paneStyle: "clean" }, surfaces(60)]);
    expect(latest.appearance.panePresentation?.paneStyle).toBe("clean");
    expect(latest.appearance.translucency?.terminal).toBe(60);
  });
  it("coalesces rapid drags, keeps theme changes and never repaints an older slider position", async () => {
    const first = deferred<AppearanceState>();
    const changes: AppearanceChange[] = [];
    let latest!: AppearanceSnapshot;
    let stored = state();
    const controller = new AppearanceController({
      getAppearance: async () => stored,
      setAppearance: async change => {
        changes.push(change);
        if (changes.length === 1) return first.promise;
        stored = typeof change === "string" ? { ...stored, preference: change, resolved: "light" }
          : "pinnedProjects" in change ? { ...stored, projectPresentation: change }
          : "terminalFontSize" in change ? { ...stored, workspacePresentation: change }
          : "paneStyle" in change ? { ...stored, panePresentation: change }
          : "light" in change ? { ...stored, systemThemes: change } : { ...stored, translucency: change };
        return stored;
      },
    }, value => { latest = value; });
    controller.refresh(); await controller.settled();
    controller.change(surfaces(10));
    controller.change(surfaces(40));
    controller.change("light");
    controller.change(surfaces(85, 60));
    expect(latest.appearance.translucency).toEqual(surfaces(85, 60));
    expect(changes).toEqual([surfaces(10)]);
    first.resolve(state(10)); await controller.settled();
    expect(changes).toEqual([surfaces(10), "light", surfaces(85, 60)]);
    expect(latest).toEqual({ appearance: { preference: "light", resolved: "light", translucency: surfaces(85, 60) }, pending: false, error: null });
  });
  it("rolls back a failed save and allows a later retry without losing the other slider", async () => {
    let latest!: AppearanceSnapshot;
    let fail = true;
    const controller = new AppearanceController({
      getAppearance: async () => state(35, 70),
      setAppearance: async change => { if (fail) throw new Error("Disk unavailable"); return { ...state(), translucency: change as SurfaceTranslucency }; },
    }, value => { latest = value; });
    controller.refresh(); await controller.settled();
    controller.change(surfaces(90, 70));
    expect(latest.appearance.translucency).toEqual(surfaces(90, 70));
    await controller.settled();
    expect(latest.appearance.translucency).toEqual(surfaces(35, 70));
    expect(latest.error).toBe("Disk unavailable");
    fail = false;
    controller.change(surfaces(90, 70)); await controller.settled();
    expect(latest.error).toBeNull();
    expect(latest.appearance.translucency).toEqual(surfaces(90, 70));
  });
  it("preserves pins through overlapping text-size and pane changes and rolls back a failed save", async () => {
    const first = deferred<AppearanceState>();
    const original = { version: 2 as const, terminalFontSize: 20, pinnedWorkspaces: ["workspace-1"] };
    let stored: AppearanceState = { ...state(), workspacePresentation: original };
    let latest!: AppearanceSnapshot;
    let writes = 0;
    let fail = false;
    const controller = new AppearanceController({
      getAppearance: async () => stored,
      setAppearance: async change => {
        if (++writes === 1) return first.promise;
        if (fail) throw new Error("Disk unavailable");
        if (typeof change === "object") stored = "pinnedProjects" in change
          ? { ...stored, projectPresentation: change } : "terminalFontSize" in change
          ? { ...stored, workspacePresentation: change } : "paneStyle" in change
          ? { ...stored, panePresentation: change } : "light" in change
          ? { ...stored, systemThemes: change } : { ...stored, translucency: change };
        return stored;
      },
    }, value => { latest = value; });
    controller.refresh(); await controller.settled();
    controller.change({ ...original, terminalFontSize: 22 });
    controller.change({ version: 1, paneStyle: "clean" });
    controller.change({ ...original, terminalFontSize: 24, pinnedWorkspaces: ["workspace-1", "workspace-2"] });
    expect(latest.appearance.workspacePresentation?.terminalFontSize).toBe(24);
    first.resolve({ ...stored, workspacePresentation: { ...original, terminalFontSize: 22 } });
    await controller.settled();
    expect(latest.appearance.panePresentation?.paneStyle).toBe("clean");
    expect(latest.appearance.workspacePresentation?.pinnedWorkspaces).toEqual(["workspace-1", "workspace-2"]);
    fail = true;
    controller.change({ ...original, terminalFontSize: 26 });
    await controller.settled();
    expect(latest.appearance.workspacePresentation).toEqual({ ...original, terminalFontSize: 24, pinnedWorkspaces: ["workspace-1", "workspace-2"] });
    expect(latest.error).toBe("Disk unavailable");
  });

});

it("keeps project pins separate from workspace writes and rolls back failed pin saves", async () => {
  const first = deferred<AppearanceState>();
  const empty = { version: 1 as const, pinnedProjects: [] };
  const pins = { version: 1 as const, pinnedProjects: ["/projects/Example"] };
  const workspace = { version: 2 as const, terminalFontSize: 18, pinnedWorkspaces: ["workspace-a"] };
  let stored: AppearanceState = { ...state(), projectPresentation: empty };
  let latest!: AppearanceSnapshot;
  let fail = false;
  const changes: AppearanceChange[] = [];
  const controller = new AppearanceController({
    getAppearance: async () => stored,
    setAppearance: async change => {
      changes.push(change);
      if (changes.length === 1) return first.promise;
      if (fail) throw new Error("Disk unavailable");
      if (typeof change === "object" && "pinnedProjects" in change) stored = { ...stored, projectPresentation: change };
      if (typeof change === "object" && "terminalFontSize" in change) stored = { ...stored, workspacePresentation: change };
      return stored;
    },
  }, value => { latest = value; });
  controller.refresh(); await controller.settled();
  controller.change(surfaces(20));
  controller.change(pins);
  controller.change(workspace);
  expect(latest.appearance.projectPresentation).toEqual(pins);
  first.resolve(stored); await controller.settled();
  expect(changes).toEqual([surfaces(20), pins, workspace]);
  fail = true;
  controller.change(empty); await controller.settled();
  expect(latest.appearance.projectPresentation).toEqual(pins);
  expect(latest.appearance.workspacePresentation).toEqual(workspace);
  expect(latest.error).toBe("Disk unavailable");
});

describe("a slider drag", () => {
  afterEach(() => { vi.useRealTimers(); });

  it("saves once, where the slider comes to rest, however many steps the drag took", async () => {
    vi.useFakeTimers();
    const changes: AppearanceChange[] = [];
    const controller = new AppearanceController({
      getAppearance: async () => state(),
      setAppearance: async change => { changes.push(change); return state(change as unknown as number); },
    }, () => undefined);
    const drag = new SliderCommit(terminal => { controller.change(surfaces(terminal)); });
    for (let terminal = 1; terminal <= 60; terminal++) {
      drag.input(terminal);
      vi.advanceTimersByTime(16);
    }
    expect(changes).toEqual([]);
    vi.advanceTimersByTime(SLIDER_COMMIT_DELAY_MS);
    await controller.settled();
    expect(changes).toEqual([surfaces(60)]);
  });

  it("saves at once on release, and nothing when nothing moved", () => {
    vi.useFakeTimers();
    const saved: number[] = [];
    const drag = new SliderCommit(value => { saved.push(value); });
    drag.flush();
    drag.input(12);
    drag.input(30);
    drag.flush();
    expect(saved).toEqual([30]);
    vi.advanceTimersByTime(10 * SLIDER_COMMIT_DELAY_MS);
    expect(saved).toEqual([30]);
  });
});
