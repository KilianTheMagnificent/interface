import { readFileSync } from "node:fs";
import {
  DEFAULT_WORKSPACE_PRESENTATION, isSurfaceTranslucency, SOLID_SURFACES, isProjectPresentation, isWorkspacePresentation,
} from "./appearance";
import { describe, expect, it } from "vitest";
import {
  canonicalThemePreference,
  DEFAULT_CLI_THEME,
  isCliThemePreference,
  isResolvedTheme,
  isThemePreference,
  nativeThemeSource,
  pairThemeWithChrome,
  resolveThemePreference,
  themeKind,
  CLI_THEME_PREFERENCES,
  THEME_PREFERENCES,
  WINDOW_BACKGROUND,
} from "./appearance";

describe("appearance contract", () => {
  it("keeps System, the Porcelain pair and the eight named palettes", () => {
    expect(THEME_PREFERENCES).toEqual([
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
    ]);
    for (const value of THEME_PREFERENCES) expect(isThemePreference(value)).toBe(true);
    expect(isResolvedTheme("system")).toBe(false);
    expect(isResolvedTheme("catppuccin-mocha")).toBe(true);
    for (const value of ["", "auto", "cursor", "mocha", null, 1]) {
      expect(isThemePreference(value)).toBe(false);
      expect(isResolvedTheme(value)).toBe(false);
    }
  });

  it("resolves System to Interface Light/Dark while named themes stay pinned", () => {
    expect(resolveThemePreference("system", false)).toBe("light");
    expect(resolveThemePreference("system", true)).toBe("dark");
    expect(resolveThemePreference("light", true)).toBe("light");
    expect(resolveThemePreference("dark", false)).toBe("dark");
    expect(resolveThemePreference("anthropic-light", true)).toBe("anthropic-light");
    expect(resolveThemePreference("catppuccin-mocha", false)).toBe("catppuccin-mocha");
  });

  it("maps each named theme to a light/dark kind for native chrome", () => {
    expect(themeKind("light")).toBe("light");
    expect(themeKind("anthropic-light")).toBe("light");
    expect(themeKind("solarized-light")).toBe("light");
    expect(themeKind("dark")).toBe("dark");
    expect(themeKind("anthropic-dark")).toBe("dark");
    expect(themeKind("solarized-dark")).toBe("dark");
    expect(themeKind("catppuccin-mocha")).toBe("dark");
    expect(themeKind("hermes")).toBe("dark");
    expect(themeKind("hermes-light")).toBe("light");

    expect(nativeThemeSource("system")).toBe("system");
    expect(nativeThemeSource("light")).toBe("light");
    expect(nativeThemeSource("anthropic-dark")).toBe("dark");
    expect(nativeThemeSource("catppuccin-mocha")).toBe("dark");
  });

  it("names Carbon as the one Bare CLI palette", () => {
    expect(CLI_THEME_PREFERENCES).toEqual(["grok-build"]);
    expect(DEFAULT_CLI_THEME).toBe("grok-build");
    expect(isCliThemePreference("grok-build")).toBe(true);
    expect(isCliThemePreference("nightshade")).toBe(false);
    expect(isCliThemePreference("hermes")).toBe(false);
    expect(isCliThemePreference("system")).toBe(false);
    expect(isCliThemePreference("solarized-dark")).toBe(false);
  });

  it("pairs Bare CLI with a console palette and restores the workspace theme", () => {
    expect(pairThemeWithChrome("cli", "hermes", "grok-build", "system")).toEqual({
      theme: "grok-build",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "hermes",
    });
    expect(pairThemeWithChrome("refined", "grok-build", "grok-build", "hermes")).toEqual({
      theme: "hermes",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "hermes",
    });
    expect(pairThemeWithChrome("refined", "anthropic-light", "grok-build", "hermes")).toEqual({
      theme: "anthropic-light",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "anthropic-light",
    });
    expect(pairThemeWithChrome("cli", "system", "grok-build", "system")).toEqual({
      theme: "grok-build",
      lastCliTheme: "grok-build",
      lastWorkspaceTheme: "system",
    });
  });

  it("maps the retired palettes to the themes that replaced them", () => {
    expect(THEME_PREFERENCES).not.toContain("nightshade");
    for (const [retired, replacement] of [
      ["sauteed-paper-light", "anthropic-light"], ["warm-editorial-light", "anthropic-light"],
      ["sauteed-paper-dark", "anthropic-dark"], ["warm-editorial-dark", "anthropic-dark"],
      ["nightshade", "grok-build"],
    ] as const) {
      expect(isThemePreference(retired)).toBe(false);
      expect(canonicalThemePreference(retired)).toBe(replacement);
    }
    expect(canonicalThemePreference("hermes")).toBe("hermes");
    expect(canonicalThemePreference("toString")).toBeNull();
    expect(canonicalThemePreference(7)).toBeNull();
  });

  it("gives every resolved theme a window background that matches its editor canvas", () => {
    expect(WINDOW_BACKGROUND.light).toBe("#f0f0ee");
    expect(WINDOW_BACKGROUND.dark).toBe("#0e0e0d");
    expect(WINDOW_BACKGROUND["anthropic-light"]).toBe("#faf9f5");
    expect(WINDOW_BACKGROUND["anthropic-dark"]).toBe("#141413");
    expect(WINDOW_BACKGROUND["solarized-light"]).toBe("#fdf6e3");
    expect(WINDOW_BACKGROUND["solarized-dark"]).toBe("#002b36");
    expect(WINDOW_BACKGROUND["catppuccin-mocha"]).toBe("#1e1e2e");
    expect(WINDOW_BACKGROUND.hermes).toBe("#142762");
    expect(WINDOW_BACKGROUND["hermes-light"]).toBe("#f8fafd");
  });
});

it("shares the versioned translucency acceptance cases with Rust", () => {
  const fixture = JSON.parse(readFileSync(new URL("../../../docs/fixtures/surface-translucency-v1.json", import.meta.url), "utf8"));
  for (const value of fixture.accepted) expect(isSurfaceTranslucency(value)).toBe(true);
  for (const value of fixture.rejected) expect(isSurfaceTranslucency(value)).toBe(false);
  expect(SOLID_SURFACES).toEqual(fixture.accepted[0]);
});

it("validates the shared workspace presentation v2 fixtures and leaves v1 to older surfaces", () => {
  const cases = JSON.parse(readFileSync(new URL("../../../docs/fixtures/workspace-presentation-v2.json", import.meta.url), "utf8"));
  for (const value of cases.accepted) expect(isWorkspacePresentation(value), JSON.stringify(value)).toBe(true);
  for (const value of cases.rejected) expect(isWorkspacePresentation(value), JSON.stringify(value)).toBe(false);
  expect(DEFAULT_WORKSPACE_PRESENTATION).toEqual(cases.default);
  // Surface v6 never sends the whole-pixel v1 shape Rust keeps for v3–v5.
  const v1 = JSON.parse(readFileSync(new URL("../../../docs/fixtures/workspace-presentation-v1.json", import.meta.url), "utf8"));
  for (const value of v1.accepted) expect(isWorkspacePresentation(value), JSON.stringify(value)).toBe(false);
});

it("validates the shared project presentation v1 fixtures", () => {
  const cases = JSON.parse(readFileSync(new URL("../../../docs/fixtures/project-presentation-v1.json", import.meta.url), "utf8"));
  for (const value of cases.accepted) expect(isProjectPresentation(value)).toBe(true);
  for (const value of cases.rejected) expect(isProjectPresentation(value)).toBe(false);
  expect(isProjectPresentation({ version: 1, pinnedProjects: ["/\ud800"] })).toBe(false);
});
