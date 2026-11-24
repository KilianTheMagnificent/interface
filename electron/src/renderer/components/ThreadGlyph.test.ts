import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { THREAD_GLYPH_NAMES, ThreadGlyph } from "./ThreadGlyph";
import { ThinkingMark } from "./ThinkingMark";

describe("the Threads icons (owner, 2026-10-23, slicker icons)", () => {
  it("draws every icon on the one 16px grid with a 1.5 round stroke, hidden from assistive tech", () => {
    expect(THREAD_GLYPH_NAMES.length).toBeGreaterThan(30);
    for (const name of THREAD_GLYPH_NAMES) {
      const html = renderToStaticMarkup(createElement(ThreadGlyph, { name }));
      expect(html, name).toContain('viewBox="0 0 16 16"');
      expect(html, name).toContain('stroke-width="1.5"');
      expect(html, name).toContain('stroke-linecap="round"');
      expect(html, name).toContain('stroke-linejoin="round"');
      expect(html, name).toContain('aria-hidden="true"');
      expect(html, name).toMatch(/<(?:path|rect|circle)\b/u);
    }
  });

  it("sizes on request and keeps a caller's class beside its own", () => {
    const html = renderToStaticMarkup(createElement(ThreadGlyph, { name: "send", size: 14, className: "x" }));
    expect(html).toContain('width="14"');
    expect(html).toContain('class="thread-glyph x"');
  });
});

describe("the Threads thinking mark (owner, 2026-10-23, Pair)", () => {
  it("is two beads, decorative, and holds still when asked", () => {
    const moving = renderToStaticMarkup(createElement(ThinkingMark, {}));
    expect(moving.match(/<circle\b/gu)).toHaveLength(2);
    expect(moving).toContain('aria-hidden="true"');
    expect(moving).toContain('class="thinking-mark"');
    expect(renderToStaticMarkup(createElement(ThinkingMark, { still: true, size: 36 })))
      .toContain('class="thinking-mark is-still"');
  });

  it("moves at the app's stepped rate, rests in the background and holds under reduced motion", () => {
    const css = readFileSync(join(__dirname, "ThinkingMark.css"), "utf8");
    expect(css).toContain("animation:thinking-mark-a 1.6s steps(5) infinite");
    expect(css).toContain("animation:thinking-mark-b 1.6s steps(5) infinite");
    expect(css.match(/animation-play-state:var\(--ambient-play-state\)/gu)).toHaveLength(2);
    expect(css).toMatch(/@media \(prefers-reduced-motion: reduce\)[\s\S]*animation:none/u);
    expect(css).toContain("color:var(--status-working)");
  });
});
