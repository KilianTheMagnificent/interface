import { describe, expect, it } from "vitest";
import { appearanceState } from "./appearance-state.js";
import { DEFAULT_WORKSPACE_PRESENTATION, SOLID_SURFACES } from "../shared/appearance.js";

const base = { preference: "system", resolved: "dark" } as const;
const surfaces = {
  translucency: SOLID_SURFACES,
  panePresentation: { version: 1, paneStyle: "clean" },
  workspacePresentation: DEFAULT_WORKSPACE_PRESENTATION,
  projectPresentation: { version: 1, pinnedProjects: ["/work/app"] },
};

describe("host appearance state", () => {
  it("keeps the preference and every present optional surface, and nothing else", () => {
    expect(appearanceState({ ...base, unrelated: true })).toEqual(base);
    expect(appearanceState({ ...base, ...surfaces })).toEqual({ ...base, ...surfaces });
    expect(appearanceState({ ...base, panePresentation: surfaces.panePresentation }))
      .toEqual({ ...base, panePresentation: surfaces.panePresentation });
  });

  it("refuses a malformed state or preference", () => {
    for (const value of [null, [], "dark", { ...base, preference: "sepia" }, { preference: "dark" }]) {
      expect(() => appearanceState(value)).toThrow("The Rust host returned an invalid appearance state.");
    }
  });

  it("names the first invalid optional surface, checked in a fixed order", () => {
    const invalid = { version: 2 };
    expect(() => appearanceState({ ...base, translucency: invalid }))
      .toThrow("The Rust host returned invalid translucency settings.");
    expect(() => appearanceState({ ...base, panePresentation: invalid }))
      .toThrow("The Rust host returned invalid pane presentation settings.");
    expect(() => appearanceState({ ...base, workspacePresentation: invalid }))
      .toThrow("The Rust host returned invalid workspace presentation settings.");
    expect(() => appearanceState({ ...base, projectPresentation: invalid }))
      .toThrow("The Rust host returned invalid project presentation settings.");
    expect(() => appearanceState({ ...base, projectPresentation: invalid, translucency: invalid }))
      .toThrow("The Rust host returned invalid translucency settings.");
    expect(() => appearanceState({ ...base, projectPresentation: invalid, panePresentation: invalid }))
      .toThrow("The Rust host returned invalid pane presentation settings.");
  });

  it("keeps the v5 theme-file fields and refuses malformed ones", () => {
    const v5 = {
      preference: "custom:Moss", resolved: "custom:Moss",
      customThemes: [{ id: "custom:Moss", name: "Moss", kind: "dark", revision: "ab", preview: {} }],
      systemThemes: { version: 1, light: "light", dark: "custom:Moss" },
      themeCss: ":root { --theme-accent: #9fd4a8; }",
    };
    expect(appearanceState(v5)).toEqual(v5);
    expect(() => appearanceState({ ...v5, customThemes: [{ id: "custom:Moss" }] }))
      .toThrow("The Rust host returned invalid theme file settings.");
    expect(() => appearanceState({ ...v5, systemThemes: { version: 1, light: "system", dark: "dark" } }))
      .toThrow("The Rust host returned invalid Follow macOS theme settings.");
    expect(() => appearanceState({ ...v5, themeCss: 1 }))
      .toThrow("The Rust host returned invalid theme CSS settings.");
    expect(() => appearanceState({ ...v5, resolved: "system" }))
      .toThrow("The Rust host returned an invalid appearance state.");
    expect(() => appearanceState({ ...v5, preference: "custom:../x" }))
      .toThrow("The Rust host returned an invalid appearance state.");
  });
});
