import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { compositeHex, contrastRatio, evaluateCssColor } from "./contrast";
import { THEME_PREVIEW_TOKENS, customThemePreview, type ThemePreview } from "./theme-previews";
import { terminalAnsiPalette } from "./terminal-palettes";

/* Theme files (owner, 2026-10-02) paint over the custom-dark and custom-light
   palettes in theme.css, which work out every token from eight --theme-* key
   colours. These tests measure what a browser would paint: the bare :root
   block, then the shared custom block, then the kind's block, with a sample
   file's declarations last — every var() resolved and every color-mix
   evaluated. An empty file must clear the same bars the shipped palettes do. */

const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const rules = css.replace(/\/\*[\s\S]*?\*\//gu, comment => comment.replace(/[^\n]/gu, " "));

/** The rule whose whole prelude is `selector` (not one part of a list). */
function block(selector: string): string {
  let at = rules.indexOf(`${selector} {`);
  while (at > 0 && rules.slice(0, at).trimEnd().endsWith(",")) at = rules.indexOf(`${selector} {`, at + 1);
  if (at < 0) throw new Error(`theme.css has no ${selector} block`);
  return rules.slice(at + selector.length + 2, rules.indexOf("\n}", at));
}

function declarations(body: string): Map<string, string> {
  const found = new Map<string, string>();
  for (const [, name, value] of body.matchAll(/(--[a-z0-9-]+)\s*:\s*([^;]+);/gu)) {
    found.set(name!, value!.trim());
  }
  return found;
}

const BASE = declarations(block(":root"));
const SHARED = declarations(block(':root[data-theme="custom-dark"],\n:root[data-theme="custom-light"]'));
const KIND = {
  dark: declarations(block(':root[data-theme="custom-dark"]')),
  light: declarations(block(':root[data-theme="custom-light"]')),
};

type Kind = keyof typeof KIND;

/** The token map for a theme file of `kind` that declares `file`. */
function paint(kind: Kind, file: Record<string, string> = {}): (name: string) => string {
  const declared = new Map([...BASE, ...SHARED, ...KIND[kind], ...Object.entries(file)]);
  const resolve = (value: string, seen: ReadonlySet<string>): string =>
    value.replace(/var\(\s*(--[a-z0-9-]+)\s*(?:,([^()]*))?\)/gu, (_match, name: string, fallback?: string) => {
      const raw = declared.get(name);
      if (raw === undefined || seen.has(name)) return fallback?.trim() ?? "";
      return resolve(raw, new Set([...seen, name]));
    });
  return name => {
    const raw = declared.get(name);
    if (raw === undefined) throw new Error(`${kind}: ${name} is not declared`);
    return evaluateCssColor(resolve(raw, new Set([name])));
  };
}

/** A wash (#rrggbbaa) composited over its ground. */
const over = (colour: string, ground: string): string => colour.length === 9 ? compositeHex(colour, ground) : colour;

const MOSS = {
  "--theme-background": "#0f1310", "--theme-panel": "#121713", "--theme-raised": "#171d18",
  "--theme-text": "#dde5d9", "--theme-accent": "#9fd4a8", "--theme-selection": "#8fb4f0",
  "--theme-screen": "#0a0d0b", "--theme-screen-text": "#cdd6c9",
};
const PAPER = {
  "--theme-background": "#f6f1e7", "--theme-panel": "#eee7da", "--theme-raised": "#fffcf6",
  "--theme-text": "#2f2a24", "--theme-accent": "#8a3b1c", "--theme-selection": "#2f5fb8",
  "--theme-screen": "#fffcf6", "--theme-screen-text": "#2f2a24",
};

const CASES: readonly { name: string; kind: Kind; file: Record<string, string> }[] = [
  { name: "empty dark file", kind: "dark", file: {} },
  { name: "empty light file", kind: "light", file: {} },
  { name: "Moss (keys only)", kind: "dark", file: MOSS },
  { name: "Paper (keys only)", kind: "light", file: PAPER },
];

describe("theme-file palettes", () => {
  it("sit inside the interface-palette layer, so a file's plain :root rule wins", () => {
    const open = css.indexOf("@layer interface-palette {");
    expect(open).toBeGreaterThan(0);
    let depth = 0;
    let close = -1;
    for (let at = css.indexOf("{", open); at < css.length; at += 1) {
      if (css[at] === "{") depth += 1;
      else if (css[at] === "}" && --depth === 0) { close = at; break; }
    }
    for (const selector of [':root[data-theme="custom-dark"] {', ':root[data-theme="custom-light"] {', ":root {"]) {
      const at = css.indexOf(selector);
      expect(at, selector).toBeGreaterThan(open);
      expect(at, selector).toBeLessThan(close);
    }
    // !important inside the layer would beat a theme file's normal declarations.
    expect(css.slice(open, close)).not.toMatch(/!important/u);
  });

  it("declare every colour token a shipped palette declares", () => {
    const shipped = new Set<string>();
    for (const [, id, body] of rules.matchAll(/:root\[data-theme="([^"]+)"\] \{([\s\S]*?)\n\}/gu)) {
      if (id!.startsWith("custom-")) continue;
      for (const name of declarations(body!).keys()) shipped.add(name);
    }
    expect(shipped.size).toBeGreaterThan(60);
    for (const kind of ["dark", "light"] as const) {
      const declared = new Set([...SHARED.keys(), ...KIND[kind].keys()]);
      const missing = [...shipped].filter(name => !declared.has(name)).sort();
      expect(missing, `custom-${kind}`).toEqual([]);
    }
  });

  it("default to Porcelain Night and Porcelain's key colours", () => {
    expect(paint("dark")("--editor-bg")).toBe("#0e0e0d");
    expect(paint("dark")("--fg")).toBe("#e4e2de");
    expect(paint("dark")("--accent")).toBe("#edebe8");
    expect(paint("light")("--editor-bg")).toBe("#f0f0ee");
    expect(paint("light")("--fg")).toBe("#2a2b2e");
    expect(paint("dark", MOSS)("--accent")).toBe("#9fd4a8");
    expect(paint("dark", { ...MOSS, "--accent": "#ff00aa" })("--accent")).toBe("#ff00aa");
  });

  for (const { name, kind, file } of CASES) {
    it(`keeps text legible: ${name}`, () => {
      const token = paint(kind, file);
      const chrome = token("--chrome-bg");
      const grounds = ["--editor-bg", "--chrome-bg", "--surface", "--surface-raised", "--workbench-panel",
        "--terminal-header-bg", "--terminal-header-bg-focused"].map(ground => [ground, token(ground)] as const);
      const rows = ["--row-hover", "--row-selected", "--sidebar-selected-fill"]
        .map(row => [`${row} over --chrome-bg`, over(token(row), chrome)] as const);
      for (const text of ["--fg", "--fg-strong", "--fg-muted", "--fg-dim", "--accent-text", "--input-fg"]) {
        for (const [ground, colour] of [...grounds, ...rows]) {
          expect(contrastRatio(token(text), colour), `${name}: ${text} on ${ground}`).toBeGreaterThanOrEqual(4.5);
        }
      }
      for (const status of ["--status-working", "--status-waiting", "--status-failed", "--danger-fg"]) {
        for (const ground of ["--chrome-bg", "--surface"]) {
          expect(contrastRatio(token(status), token(ground)), `${name}: ${status} on ${ground}`)
            .toBeGreaterThanOrEqual(4.5);
        }
      }
      expect(contrastRatio(token("--accent-fg"), token("--accent")), `${name}: --accent-fg on --accent`)
        .toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(token("--badge-fg"), token("--badge-bg")), `${name}: --badge-fg on --badge-bg`)
        .toBeGreaterThanOrEqual(4.5);
      const screen = token("--terminal-content-bg");
      for (const text of ["--terminal-fg", "--terminal-fg-strong", "--terminal-fg-muted", "--terminal-fg-dim"]) {
        expect(contrastRatio(token(text), screen), `${name}: ${text} on the screen`).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio(token("--input-placeholder"), token("--editor-bg")), `${name}: placeholder`)
        .toBeGreaterThanOrEqual(4.5);
      // Settled look: tile edges and the focus ring are a neutral grey.
      for (const edge of ["--terminal-tile-edge", "--terminal-focus-edge"]) {
        const [r, g, b] = [1, 3, 5].map(at => Number.parseInt(token(edge).slice(at, at + 2), 16));
        expect(Math.max(r!, g!, b!) - Math.min(r!, g!, b!), `${name}: ${edge} ${token(edge)} is neutral`)
          .toBeLessThanOrEqual(1);
      }
      // Settled look: the focused pane's head is lifted above the idle one.
      expect(token("--terminal-header-bg-focused"), `${name}: focused head`).not.toBe(token("--terminal-header-bg"));
    });
  }
});

describe("evaluateCssColor", () => {
  it("mixes as a browser does", () => {
    expect(evaluateCssColor("#abc")).toBe("#aabbcc");
    expect(evaluateCssColor("color-mix(in srgb, #ffffff 50%, #000000)")).toBe("#808080");
    expect(evaluateCssColor("color-mix(in srgb, #ff0000 20%, transparent)")).toBe("#ff000033");
    expect(evaluateCssColor("color-mix(in oklab, #000000, #ffffff 100%)")).toBe("#ffffff");
    expect(evaluateCssColor("color-mix(in oklab, #0e0e0d, #e4e2de 0%)")).toBe("#0e0e0d");
    // oklab's midpoint between black and white is L 0.5: #636363, as Chromium paints it.
    expect(evaluateCssColor("color-mix(in oklab, #000000, #ffffff)")).toBe("#636363");
  });
});

describe("theme-file swatches and terminal colours", () => {
  it("draw an empty file's swatch from what its palette paints", () => {
    for (const kind of ["dark", "light"] as const) {
      const token = paint(kind);
      const preview = customThemePreview({ kind, preview: {} });
      for (const [field, name] of Object.entries(THEME_PREVIEW_TOKENS) as [keyof ThemePreview, string][]) {
        expect(preview[field], `${kind} ${field} (${name})`).toBe(token(name));
      }
    }
  });

  it("prefer the most specific colour a file declares and skip what is not a colour", () => {
    const preview = customThemePreview({ kind: "dark", preview: {
      "--theme-panel": "#121713", "--chrome-bg": "var(--theme-panel)",
      "--theme-accent": "#9fd4a8", "--accent": "url(https://example.invalid/x.png)",
      "--status-working": "var(--missing, #f2a65a)", "--theme-screen": "#0a0d0b",
    } });
    expect(preview.chrome).toBe("#121713");
    expect(preview.frame).toBe("#121713");
    expect(preview.accent).toBe("#9fd4a8");
    expect(preview.working).toBe("#f2a65a");
    expect(preview.screen).toBe("#0a0d0b");
    expect(preview.done).toBe(customThemePreview({ kind: "dark", preview: {} }).done);
  });

  it("give terminals the file's ANSI colours, the rest from a set made for its screen", () => {
    const styles = (values: Record<string, string>) => ({
      getPropertyValue: (name: string) => values[name] ?? "",
    });
    const dark = terminalAnsiPalette("custom-dark", styles({
      "--terminal-content-bg": "#0a0d0b", "--ansi-red": " #f07e8c", "--ansi-bright-black": "#6d786b",
    }));
    expect(dark.red).toBe("#f07e8c");
    expect(dark.brightBlack).toBe("#6d786b");
    expect(dark.white).toBe(terminalAnsiPalette("dark").white);
    const light = terminalAnsiPalette("custom-light", styles({ "--terminal-content-bg": "#fffcf6" }));
    expect(light.yellow).toBe(terminalAnsiPalette("hermes-light").yellow);
    const scheme = terminalAnsiPalette("custom-dark", styles({
      "--terminal-content-bg": "color-mix(in oklab, #fff, #eee)", "--terminal-color-scheme": "light",
    }));
    expect(scheme.yellow).toBe(terminalAnsiPalette("hermes-light").yellow);
    expect(terminalAnsiPalette("custom-dark")).toEqual({});
  });
});

