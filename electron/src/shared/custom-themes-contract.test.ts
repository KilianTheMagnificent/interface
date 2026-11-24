import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  appearanceThemeSource, isCustomThemeSummary, isSystemThemes, isThemeId, resolveAppearanceTheme,
  resolvedThemeKind, windowBackground, type CustomThemeSummary,
} from "./appearance";

const fixture = JSON.parse(readFileSync(
  new URL("../../../docs/fixtures/custom-themes-v1.json", import.meta.url), "utf8"));

const moss: CustomThemeSummary = {
  id: "custom:Moss", name: "Moss", kind: "dark", background: "#0f1310", revision: "00ff", preview: {},
};
const paper: CustomThemeSummary = {
  id: "custom:Paper", name: "Paper", kind: "light", revision: "0a0a", preview: {},
};
const broken: CustomThemeSummary = {
  id: "custom:Big", name: "Big", kind: "dark", revision: "size-2", preview: {},
  problem: "The file is larger than 1 MB.",
};

describe("theme files on appearance surface v5", () => {
  it("agree with the Rust contract on theme ids and the Follow macOS pair", () => {
    for (const value of fixture.themeIds.accepted) expect(isThemeId(value), String(value)).toBe(true);
    for (const value of fixture.themeIds.rejected) expect(isThemeId(value), String(value)).toBe(false);
    for (const value of fixture.systemThemes.accepted) expect(isSystemThemes(value)).toBe(true);
    for (const value of fixture.systemThemes.rejected) expect(isSystemThemes(value)).toBe(false);
  });

  it("accept the summaries Rust writes and refuse anything else", () => {
    for (const summary of [moss, paper, broken, { ...moss, preview: { "--theme-accent": "#9fd4a8" } }]) {
      expect(isCustomThemeSummary(summary)).toBe(true);
    }
    for (const bad of [
      null, { ...moss, name: "Other" }, { ...moss, kind: "dim" }, { ...moss, background: "red" },
      { ...moss, extra: 1 }, { ...moss, preview: { accent: "#fff" } }, { ...moss, id: "dark" },
    ]) expect(isCustomThemeSummary(bad)).toBe(false);
  });

  it("resolve as the host does when macOS changes between reads", () => {
    const customThemes = [moss, paper, broken];
    const systemThemes = { version: 1, light: "custom:Paper", dark: "custom:Moss" } as const;
    expect(resolveAppearanceTheme({ preference: "custom:Moss", customThemes }, false)).toBe("custom:Moss");
    expect(resolveAppearanceTheme({ preference: "system", systemThemes, customThemes }, false)).toBe("custom:Paper");
    expect(resolveAppearanceTheme({ preference: "system", systemThemes, customThemes }, true)).toBe("custom:Moss");
    expect(resolveAppearanceTheme({ preference: "system", customThemes }, true)).toBe("dark");
    // A missing or unpaintable file paints what Follow macOS would.
    expect(resolveAppearanceTheme({ preference: "custom:Gone", systemThemes, customThemes }, true)).toBe("custom:Moss");
    expect(resolveAppearanceTheme({ preference: "custom:Big", customThemes }, false)).toBe("light");
    expect(resolveAppearanceTheme({ preference: "system", systemThemes, customThemes: [] }, true)).toBe("dark");
  });

  it("give main the native kind and window fill of a theme file", () => {
    const customThemes = [moss, paper, broken];
    expect(resolvedThemeKind({ resolved: "custom:Paper", customThemes })).toBe("light");
    expect(resolvedThemeKind({ resolved: "solarized-dark", customThemes })).toBe("dark");
    expect(windowBackground({ resolved: "custom:Moss", customThemes })).toBe("#0f1310");
    expect(windowBackground({ resolved: "custom:Paper", customThemes })).toBe("#f0f0ee");
    expect(windowBackground({ resolved: "dark", customThemes })).toBe("#0e0e0d");
    expect(appearanceThemeSource({ preference: "custom:Paper", customThemes })).toBe("light");
    expect(appearanceThemeSource({ preference: "custom:Big", customThemes })).toBe("system");
    expect(appearanceThemeSource({ preference: "system", customThemes })).toBe("system");
    expect(appearanceThemeSource({ preference: "hermes", customThemes })).toBe("dark");
  });
});
