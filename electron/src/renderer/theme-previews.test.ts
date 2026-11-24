import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { THEME_PREFERENCES, type ResolvedTheme } from "../shared/appearance";
import { THEME_PREVIEW_TOKENS, THEME_PREVIEWS, customThemeMoves, type ThemePreview } from "./theme-previews";

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const rootBlock = css.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";

/** A token as the theme paints it: its own block first, then :root, with var() followed. */
function token(theme: ResolvedTheme, name: string): string {
  const block = css.match(new RegExp(`:root\\[data-theme="${theme}"\\] \\{([\\s\\S]*?)\\n\\}`, "u"))?.[1] ?? "";
  for (const source of [block, rootBlock]) {
    const value = source.match(new RegExp(`\\n\\s*${name}:\\s*([^;]+);`, "u"))?.[1]?.trim();
    if (value === undefined) continue;
    const reference = value.match(/^var\((--[a-z0-9-]+)\)$/u)?.[1];
    return reference === undefined ? value : token(theme, reference);
  }
  throw new Error(`${theme} does not declare ${name}`);
}

describe("a theme file's look", () => {
  it("moves only when the host says its backdrop can", () => {
    expect(customThemeMoves({ preview: { "--theme-look": "moving" } })).toBe(true);
    expect(customThemeMoves({ preview: { "--theme-look": "still" } })).toBe(false);
    expect(customThemeMoves({ preview: { "--theme-panel": "#121713" } })).toBe(false);
    expect(customThemeMoves({ preview: {} })).toBe(false);
  });

  it("is said by the host for a look that moves, in the word the renderer reads", () => {
    const host = readFileSync(new URL("../../../crates/interface-settings/src/custom_themes.rs", import.meta.url), "utf8");
    expect(host).toContain('pub const CUSTOM_THEME_LOOK_PROPERTY: &str = "--theme-look";');
    // What moves is decided in Rust (custom_themes.rs look_moves and its tests).
    expect(host).toMatch(/let look = if look_moves\(&css, &declared\) \{\s*"moving"/u);
    expect(host).toContain("preview.insert(CUSTOM_THEME_LOOK_PROPERTY.to_owned(), look.to_owned());");
  });
});

describe("theme previews", () => {
  it("draws every theme from its own tokens", () => {
    const themes = THEME_PREFERENCES.filter((theme): theme is ResolvedTheme => theme !== "system");
    expect(Object.keys(THEME_PREVIEWS).sort()).toEqual([...themes].sort());
    for (const theme of themes) {
      for (const [field, name] of Object.entries(THEME_PREVIEW_TOKENS) as [keyof ThemePreview, string][]) {
        expect(THEME_PREVIEWS[theme][field], `${theme} ${field} (${name})`).toBe(token(theme, name));
      }
    }
  });
});
