import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { THEME_PREFERENCES, type ResolvedTheme } from "../shared/appearance";
import { contrastRatio } from "./contrast";
import {
  LIGHT_SCREEN_MINIMUM_CONTRAST, TERMINAL_ANSI_PALETTES, terminalAnsiPalette, terminalMinimumContrast,
} from "./terminal-palettes";

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const rootBlock = css.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";

function screen(theme: ResolvedTheme): string {
  const block = css.match(new RegExp(`:root\\[data-theme="${theme}"\\] \\{([\\s\\S]*?)\\n\\}`, "u"))?.[1];
  expect(block, `missing ${theme} palette`).toBeDefined();
  const token = (source: string) => source.match(/--terminal-content-bg:\s*(#[0-9a-f]{6});/iu)?.[1];
  const value = token(block ?? "") ?? token(rootBlock);
  expect(value, `${theme} screen`).toBeDefined();
  return value ?? "";
}

const themes = THEME_PREFERENCES.filter((theme): theme is ResolvedTheme => theme !== "system");

describe("terminal ANSI palettes", () => {
  it("gives every theme all sixteen colours", () => {
    expect(Object.keys(TERMINAL_ANSI_PALETTES).sort()).toEqual([...themes].sort());
    for (const theme of themes) {
      const palette = TERMINAL_ANSI_PALETTES[theme];
      expect(Object.keys(palette), theme).toHaveLength(16);
      for (const [name, value] of Object.entries(palette)) {
        expect(value, `${theme} ${name}`).toMatch(/^#[0-9a-f]{6}$/u);
      }
    }
  });

  it("keeps every colour legible on its own screen", () => {
    for (const theme of themes) {
      const background = screen(theme);
      for (const [name, value] of Object.entries(TERMINAL_ANSI_PALETTES[theme])) {
        if (name === "black") continue;
        const floor = name === "brightBlack" ? 3 : 4.5;
        expect(contrastRatio(value, background), `${theme} ${name} on ${background}`)
          .toBeGreaterThanOrEqual(floor);
      }
    }
  });

  it("holds text to 7:1 on light screens and leaves dark screens alone", () => {
    const styles = (properties: Record<string, string>) =>
      ({ getPropertyValue: (name: string) => properties[name] ?? "" });
    for (const theme of themes) {
      const light = contrastRatio(screen(theme), "#000000") > contrastRatio(screen(theme), "#ffffff");
      expect(terminalMinimumContrast(styles({ "--terminal-content-bg": screen(theme) })), theme)
        .toBe(light ? LIGHT_SCREEN_MINIMUM_CONTRAST : 1);
    }
    expect(terminalMinimumContrast(styles({ "--terminal-content-bg": "#fdfcf9" }))).toBe(7);
    expect(terminalMinimumContrast(styles({ "--terminal-content-bg": "#18191b" }))).toBe(1);
    // The fixed colours that faded on Clay Light: a status line's orange and Claude's yellow.
    expect(contrastRatio("#e85d13", screen("anthropic-light"))).toBeLessThan(LIGHT_SCREEN_MINIMUM_CONTRAST);
    expect(contrastRatio("#ffc107", screen("anthropic-light"))).toBeLessThan(LIGHT_SCREEN_MINIMUM_CONTRAST);
  });

  it("answers nothing for an unknown or missing theme", () => {
    expect(terminalAnsiPalette(undefined)).toEqual({});
    expect(terminalAnsiPalette("nightshade")).toEqual({});
    expect(terminalAnsiPalette("hermes-light").yellow).toBe(TERMINAL_ANSI_PALETTES["hermes-light"].yellow);
  });
});
