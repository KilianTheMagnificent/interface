import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { compositeHex, contrastRatio, evaluateCssColor } from "./contrast";

/* ==========================================================================
   A THREADS PANE AND ITS PROJECT'S HUE. The "hued glow" (owner, 2026-10-06)
   lit the aurora, the prompt bubble, inline code, the work line's chips,
   Review, the live trail and the composer's edge and Send. The Threads look
   (owner, 2026-10-23, the Claude app's Code mode) took the hue off all of it:
   a project keeps its hue only as the small square before its name, and the
   pane's accents are the theme's own. Held here: the transcript and the
   composer read no hue, their neutral rules draw what the hued ones did, the
   composer's lifts stay off in Bare-CLI chrome, and the one tally every work
   chip shares stays legible in every palette.
   ========================================================================== */

const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");
const theme = read("./theme.css");
const transcriptCss = read("./components/ThreadTranscript.css");
const composerCss = read("./components/ThreadComposer.css");

const stripComments = (source: string): string => source.replace(/\/\*[\s\S]*?\*\//gu, "");
const rules = (source: string): { selector: string; body: string }[] =>
  [...stripComments(source).matchAll(/([^{}]+)\{([^{}]*)\}/gu)]
    .map(([, selector, body]) => ({ selector: selector.trim(), body }));

/** Every palette block with its colour scheme: :root is the light default. */
const rootBlock = theme.match(/:root \{([\s\S]*?)\n\}/u)?.[1] ?? "";
const palettes = [
  { id: ":root", body: rootBlock },
  ...[...theme.matchAll(/:root\[data-theme="([^"]+)"\] \{([\s\S]*?)\n\}/gu)]
    .filter(([, id]) => !id.startsWith("custom-"))
    .map(([, id, body]) => ({ id, body })),
].map(({ id, body }) => ({
  id, body, dark: /(?<![-\w])color-scheme:\s*dark;/u.test(body),
}));

function token(body: string, name: string, seen = new Set<string>()): string {
  if (seen.has(name)) throw new Error(`Circular --${name}`);
  const value = (body.match(new RegExp(`--${name}:\\s*([^;]+);`, "u"))
    ?? rootBlock.match(new RegExp(`--${name}:\\s*([^;]+);`, "u")))?.[1].trim();
  if (value === undefined) throw new Error(`Missing --${name}`);
  const reference = /^var\(--([a-z0-9-]+)\)$/u.exec(value)?.[1];
  return reference === undefined ? value : token(body, reference, new Set([...seen, name]));
}

/** A colour expression over an opaque backdrop, as painted. */
const over = (expression: string, backdrop: string): string => {
  const colour = evaluateCssColor(expression);
  return colour.length === 9 ? compositeHex(colour, backdrop) : colour;
};

describe("a Threads pane and its project's hue", () => {
  it("reads no hue in the transcript or the composer: the theme's own accents draw them", () => {
    for (const source of [transcriptCss, composerCss]) {
      expect(rules(source).filter(rule => rule.body.includes("--project-h"))).toEqual([]);
      expect(source).not.toMatch(/project-hue|--project-h/u);
    }
    // The theme's own tokens are what draw them now: the bubble on the
    // selection colour laid over the ground, the card under a hairline ring
    // (the TN Code look, owner 2026-11-16; theme.css works both out).
    expect(rules(transcriptCss).find(rule => rule.selector === ".is-thread-layout .conv-turn-user")?.body)
      .toContain("background:var(--tn-bubble); border:0;");
    expect(composerCss).toMatch(/\.thread-composer \{[^}]*border:var\(--border-w\) solid var\(--border\)/u);
    expect(composerCss).toMatch(/\.thread-composer \{[^}]*box-shadow:0 0 0 \.5px var\(--tn-separator\), var\(--composer-lift\),/u);
    expect(theme).toMatch(/--tn-bubble: light-dark\(color-mix\(in srgb, var\(--selection-accent\) 10%, var\(--editor-bg\)\),\s*color-mix\(in srgb, var\(--selection-accent\) 20%, var\(--editor-bg\)\)\);/u);
  });

  it("keeps every lift off in Bare-CLI chrome", () => {
    // The composer's soft second lift and its waiting ring.
    for (const marker of ["0 8px 24px", "0 0 0 3px"]) {
      const lift = rules(composerCss).find(rule => rule.selector.startsWith(":where")
        && rule.body.includes(marker));
      expect(lift?.selector, marker).toMatch(/^:where\(:root:not\(\[data-chrome="cli"\]\)\) /u);
    }
  });

  it("draws every work chip as one neutral tally, legibly in every palette", () => {
    // The Threads look (owner, 2026-10-23): a count on a quiet chip, the same
    // for every kind of work — no kind's colour, no glyph.
    expect(palettes.filter(palette => !palette.dark).length).toBeGreaterThanOrEqual(4);
    expect(palettes.filter(palette => palette.dark).length).toBeGreaterThanOrEqual(4);
    const chips = rules(transcriptCss).filter(rule => rule.selector.includes(".thread-work-chip"));
    expect(chips.map(rule => rule.selector)).toEqual([".thread-work-chip"]);
    expect(chips[0].body).toContain("color:var(--fg-dim)");
    // The TN Code look (owner, 2026-11-16): the chip is a soft fill, 7% of
    // the strongest ink over the pane's ground.
    expect(chips[0].body).toContain("background:var(--tn-fill)");
    expect(theme).toContain("--tn-fill: color-mix(in srgb, var(--fg-strong) 7%, transparent);");
    expect(chips[0].body).toContain("height:18px");
    const failures: string[] = [];
    for (const palette of palettes) {
      const editor = token(palette.body, "editor-bg");
      // 7% is 0x12 of 255: the strongest ink at that alpha, over the ground.
      const ink = evaluateCssColor(token(palette.body, "fg-strong"));
      expect(ink, palette.id).toMatch(/^#[0-9a-f]{6}$/iu);
      const chip = compositeHex(`${ink}12`, editor);
      const text = over(token(palette.body, "fg-dim"), chip);
      const ratio = contrastRatio(text, chip);
      if (ratio < 4.5) failures.push(`${palette.id}: ${text} on ${chip} is ${ratio.toFixed(2)}`);
    }
    expect(failures).toEqual([]);
  });
});
