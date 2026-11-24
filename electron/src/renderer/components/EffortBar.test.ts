import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  EFFORT_PICKER_CSS,
  EffortBar,
  EffortPopover,
  positionEffortPopover,
} from "./EffortBar";
import {
  effortChangeCommitsImmediately,
  effortDisplayName,
  effortLevelCelebrates,
  orderEffortLevels,
  shouldCelebrateEffortChange,
  topTierLevel,
} from "../effort-levels";

describe("topTierLevel", () => {
  it("names only the ladder's last rung, and only when it is a top tier", () => {
    expect(topTierLevel(["low", "high", "max", "ultracode"])).toBe("ultracode");
    expect(topTierLevel(["minimal", "low", "max", "ultra"])).toBe("ultra");
    expect(topTierLevel(["low", "medium", "max"])).toBe("max");
    expect(topTierLevel(["low", "medium", "xhigh"])).toBeNull();
    expect(topTierLevel([])).toBeNull();
  });
});

describe("shouldCelebrateEffortChange", () => {
  const grok = ["low", "medium", "high", "xhigh"];
  const claude = ["low", "max", "ultracode"];

  it("fires only when landing on the ladder's highest celebratory rung", () => {
    expect(effortLevelCelebrates("xhigh", grok)).toBe(true);
    expect(effortLevelCelebrates("high", grok)).toBe(false);
    expect(effortLevelCelebrates("ultracode", claude)).toBe(true);
    expect(effortLevelCelebrates("max", claude)).toBe(false);
    expect(effortLevelCelebrates("xhigh", ["low", "xhigh", "max"])).toBe(false);

    expect(shouldCelebrateEffortChange(null, "xhigh", grok)).toBe(false);
    expect(shouldCelebrateEffortChange("high", "xhigh", grok)).toBe(true);
    expect(shouldCelebrateEffortChange("xhigh", "xhigh", grok)).toBe(false);
    expect(shouldCelebrateEffortChange("xhigh", "high", grok)).toBe(false);
    expect(shouldCelebrateEffortChange("max", "ultracode", claude)).toBe(true);
    expect(shouldCelebrateEffortChange("ultracode", "max", claude)).toBe(false);
  });
});

describe("effortDisplayName", () => {
  it("humanizes known and provider-defined values without changing their keys", () => {
    expect(effortDisplayName("xhigh")).toBe("Extra high");
    expect(effortDisplayName("max")).toBe("Max");
    expect(effortDisplayName("very_deep")).toBe("Very deep");
    expect(effortDisplayName("")).toBe("Effort");
  });
});

describe("orderEffortLevels", () => {
  it("normalizes Grok's smartest-first catalogue to faster→smarter", () => {
    expect(orderEffortLevels(["xhigh", "high", "medium", "low"]))
      .toEqual(["low", "medium", "high", "xhigh"]);
  });

  it("leaves an already-ascending ladder untouched", () => {
    expect(orderEffortLevels(["minimal", "low", "medium", "high", "xhigh"]))
      .toEqual(["minimal", "low", "medium", "high", "xhigh"]);
  });

  it("keeps unknown provider vocabulary in its advertised order, after the ladder", () => {
    expect(orderEffortLevels(["deep", "low", "turbo", "high"]))
      .toEqual(["low", "high", "deep", "turbo"]);
  });

  it("ranks case-insensitively without rewriting the provider's exact values", () => {
    expect(orderEffortLevels(["HIGH", "Low"])).toEqual(["Low", "HIGH"]);
  });

  it("places the ultracode rung above every effort value", () => {
    expect(orderEffortLevels(["ultracode", "low", "max", "xhigh"]))
      .toEqual(["low", "xhigh", "max", "ultracode"]);
    expect(effortDisplayName("ultracode")).toBe("Ultracode");
  });
});

describe("positionEffortPopover", () => {
  it("right-aligns the pop-out and still clamps it inside the viewport", () => {
    const position = positionEffortPopover(
      { top: 100, right: 440, bottom: 124, left: 380, width: 60, height: 24 },
      { width: 224, height: 116 },
      { width: 900, height: 700 },
    );
    expect(position).toMatchObject({ top: 128, left: 216, width: 224, side: "below" });

    const edge = positionEffortPopover(
      { top: 100, right: 80, bottom: 124, left: 20, width: 60, height: 24 },
      { width: 224, height: 116 },
      { width: 240, height: 300 },
    );
    expect(edge.left).toBe(8);
    expect(edge.width).toBe(224);
  });
});

describe("EffortBar", () => {
  it("rests as a compact current-value trigger, not an exposed slider", () => {
    const html = renderToStaticMarkup(createElement(EffortBar, {
      levels: ["low", "medium", "high", "xhigh"],
      value: "xhigh",
      className: "composer-app-picker composer-effort-picker",
      onChange: () => undefined,
    }));
    expect(html).toContain('aria-label="Effort: Extra high"');
    expect(html).toContain("effort-picker-xhigh");
    expect(html).toMatch(/class="[^"]*effort-picker-xhigh[^"]*"/);
    expect(html).not.toMatch(/class="[^"]*effort-picker-maximum[^"]*"/);
    expect(html).not.toMatch(/class="[^"]*effort-picker-celebrate[^"]*"/);
    expect(html).toContain('aria-haspopup="dialog"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain("composer-effort-picker");
    expect(html).toContain('class="effort-value-current">Extra high</span>');
    // The pill pre-reserves its ladder's widest label as invisible ghosts, so
    // switching levels can never widen it and shift the row's other controls.
    expect(html.match(/class="effort-value-ghost"/g)).toHaveLength(3);
    expect(html).toContain('class="effort-value-ghost">Low</span>');
    expect(html).toContain("codicon-chevron-down");
    expect(html).not.toContain('type="range"');
    expect(html).not.toContain('role="slider"');
    expect(html).not.toContain("effort-bar-track");
  });

  it("resolves the current value against a provider catalogue in any order", () => {
    const html = renderToStaticMarkup(createElement(EffortBar, {
      levels: ["xhigh", "high", "medium", "low"],
      value: "low",
      onChange: () => undefined,
    }));
    expect(html).toContain('aria-label="Effort: Low"');
  });

  it("preserves pending, disabled, and session-only max state", () => {
    const html = renderToStaticMarkup(createElement(EffortBar, {
      levels: ["low", "max"],
      value: "max",
      notes: { max: "Applies to this session only." },
      pending: true,
      disabled: true,
      onChange: () => undefined,
    }));
    expect(html).toContain("app-select-pending");
    expect(html).toContain("effort-picker-maximum");
    expect(html).not.toMatch(/class="[^"]*effort-picker-celebrate[^"]*"/);
    expect(html).toContain('aria-disabled="true"');
    expect(html).not.toContain(' disabled=""');
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain('title="Max — Applies to this session only."');

    const locked = renderToStaticMarkup(createElement(EffortBar, {
      levels: ["low", "high"],
      value: "high",
      disabled: true,
      onChange: () => undefined,
    }));
    expect(locked).toContain(' disabled=""');
  });

  it("renders a one-level catalogue as an informational value, not a fake picker", () => {
    const html = renderToStaticMarkup(createElement(EffortBar, {
      levels: ["medium"],
      value: "default",
      onChange: () => undefined,
    }));
    expect(html).toContain("effort-picker-single");
    expect(html).toContain('aria-label="Effort: Medium"');
    expect(html).toContain('aria-disabled="true"');
    expect(html).not.toContain("aria-haspopup");
    expect(html).not.toContain("codicon-chevron-down");
    expect(EFFORT_PICKER_CSS)
      .toContain(".effort-picker-single .app-select-trigger:hover");
  });
});

describe("EffortPopover", () => {
  const html = renderToStaticMarkup(createElement(EffortPopover, {
    id: "effort-demo",
    levels: ["low", "medium", "high", "max"],
    index: 3,
    notes: { max: "Applies to this session only." },
    position: { top: 40, left: 80, width: 224, maxHeight: 140, side: "above" },
    onPreview: () => undefined,
    onCommit: () => undefined,
    onClose: () => undefined,
    onExit: () => undefined,
  }));

  it("renders a dot track with a thumb, faster on the left", () => {
    expect(html).toContain('role="dialog"');
    expect(html).toContain('data-side="above"');
    expect(html).toContain('type="range"');
    expect(html).toContain('aria-label="Effort level"');
    expect(html).toContain('aria-valuetext="Max"');
    const order = ["low", "medium", "high", "max"]
      .map(level => html.indexOf(`data-effort-dot="${level}"`));
    expect([...order].sort((a, b) => a - b)).toEqual(order);
    expect(order.every(at => at >= 0)).toBe(true);
    expect(html.match(/data-effort-dot=/g)).toHaveLength(4);
    expect(html).toContain('class="effort-thumb"');
    expect(html).toContain("Faster");
    expect(html).toContain("Smarter");
  });

  it("accents only the top-tier dot and narrates the value in the header", () => {
    expect(html.match(/effort-dot is-maximum/g)).toHaveLength(1);
    expect(html).toContain('class="effort-popover is-floating is-maximum"');
    expect(html).toContain('role="status"');
    expect(html).toContain("Applies to this session only.");
    expect(html).toContain('aria-describedby="effort-demo-level-note"');
  });

  it("keeps max plain when the ladder continues past it", () => {
    const maxBelowUltracode = renderToStaticMarkup(createElement(EffortPopover, {
      id: "effort-demo",
      levels: ["low", "high", "max", "ultracode"],
      index: 2,
      position: { top: 40, left: 80, width: 224, maxHeight: 140, side: "below" },
      onPreview: () => undefined,
      onCommit: () => undefined,
      onClose: () => undefined,
      onExit: () => undefined,
    }));
    expect(maxBelowUltracode).not.toContain('class="effort-popover is-maximum"');
    expect(maxBelowUltracode).not.toContain("effort-track-fill");
    // The accent dot marks the ladder's top rung, which is ultracode here.
    expect(maxBelowUltracode.match(/effort-dot is-maximum/g)).toHaveLength(1);
    expect(maxBelowUltracode.lastIndexOf('data-effort-dot="ultracode"'))
      .toBeGreaterThan(maxBelowUltracode.lastIndexOf("effort-dot is-maximum") - 80);
  });

  it("lights the starfield fill only while a top-tier level is previewed", () => {
    expect(html).toContain('data-effort-fill="max"');
    expect(html.match(/class="effort-spark"/g)).toHaveLength(9);

    const plain = renderToStaticMarkup(createElement(EffortPopover, {
      id: "effort-demo",
      levels: ["low", "medium", "high", "max"],
      index: 1,
      position: { top: 40, left: 80, width: 224, maxHeight: 140, side: "below" },
      onPreview: () => undefined,
      onCommit: () => undefined,
      onClose: () => undefined,
      onExit: () => undefined,
    }));
    expect(plain).not.toContain("effort-track-fill");
    expect(plain).not.toContain("effort-spark");
    // The top-tier DOT keeps its accent at any selection; only the surface
    // itself must drop the maximum treatment.
    expect(plain).not.toContain('class="effort-popover is-maximum"');
  });

  it("offers no labeled segments to compact and no separate apply step", () => {
    expect(html).not.toContain("effort-segment");
    expect(html).not.toContain('role="radio"');
    expect(html).not.toContain(">Apply<");
    expect(html).not.toContain("Arrows adjust");
  });

  it("marks an xhigh preview without calling it a top-tier celebration", () => {
    const xhigh = renderToStaticMarkup(createElement(EffortPopover, {
      id: "effort-demo",
      levels: ["low", "medium", "high", "xhigh"],
      index: 3,
      position: { top: 40, left: 80, width: 224, maxHeight: 140, side: "above" },
      onPreview: () => undefined,
      onCommit: () => undefined,
      onClose: () => undefined,
      onExit: () => undefined,
    }));
    expect(xhigh).toContain("effort-popover is-floating is-xhigh");
    expect(xhigh).not.toContain("is-maximum");
    expect(xhigh).not.toContain("effort-track-fill");
  });

  it("ships a named-level ladder for CLI chrome, current row marked", () => {
    // Refined still uses the track. The ladder is in the markup so CLI can
    // show names instead of a slider without a second popover component.
    expect(html).toContain('class="effort-cli-ladder"');
    expect(html).toContain('class="effort-cli-opt is-current"');
    expect(html).toContain('data-effort-opt="max"');
    expect(html).toContain('data-effort-opt="low"');
    expect(EFFORT_PICKER_CSS).toContain(".effort-cli-ladder { display:none;");
  });

  it("commits assistive range changes while batching pointer and key adjustment", () => {
    expect(effortChangeCommitsImmediately(null)).toBe(true);
    expect(effortChangeCommitsImmediately("pointer")).toBe(false);
    expect(effortChangeCommitsImmediately("keyboard")).toBe(false);
  });

  it("uses app tokens, a short entrance, and a complete reduced-motion fallback", () => {
    expect(EFFORT_PICKER_CSS).toContain("min-width:224px");
    expect(EFFORT_PICKER_CSS).toContain("var(--dropdown-bg)");
    expect(EFFORT_PICKER_CSS).toContain("var(--overlay-shadow)");
    expect(EFFORT_PICKER_CSS).toContain("animation:effort-pop-in 140ms ease-out");
    expect(EFFORT_PICKER_CSS).toContain("prefers-reduced-motion: reduce");
    expect(EFFORT_PICKER_CSS)
      .toContain(".effort-picker.app-select-pending .app-select-value");
    expect(EFFORT_PICKER_CSS)
      .toContain(".effort-picker .app-select-trigger:disabled .app-select-value");
    // The starfield ramp mixes the theme's own accent — never a fixed palette —
    // and reduced motion stills both the sweep and the twinkle.
    expect(EFFORT_PICKER_CSS)
      .toContain("color-mix(in srgb, var(--accent) 28%, var(--surface-raised))");
    expect(EFFORT_PICKER_CSS).toContain("animation:effort-fill-sweep 380ms ease-out");
    expect(EFFORT_PICKER_CSS).toContain("animation:effort-twinkle 2.6s ease-in-out infinite");
    expect(EFFORT_PICKER_CSS)
      .toContain(".effort-spark { animation:none; opacity:.55; transform:none; }");
  });

  it("holds its three loops paused until the reader is actually on the popover", () => {
    // Three infinite animations used to start the moment the popover opened and
    // run for as long as it stayed open — a continuous frame cost on a surface
    // nobody was necessarily looking at, in an app whose stated rule is that
    // loops are reserved for real provider activity. Each is declared paused;
    // one rule starts all three on hover or focus-within.
    const paused = EFFORT_PICKER_CSS.match(/animation-play-state:paused/gu) ?? [];
    expect(paused).toHaveLength(3);
    for (const loop of [
      "animation:effort-twinkle 2.6s ease-in-out infinite",
      "animation:effort-sheen 4s ease-in-out infinite",
      "animation:effort-thumb-glow 2.6s ease-in-out infinite",
    ]) {
      const at = EFFORT_PICKER_CSS.indexOf(loop);
      expect(at, loop).toBeGreaterThan(-1);
      // The pause belongs to the same declaration block as the loop it gates.
      const block = EFFORT_PICKER_CSS.slice(at, EFFORT_PICKER_CSS.indexOf("}", at));
      expect(block, loop).toContain("animation-play-state:paused");
    }
    // And exactly one rule turns them back on, off the popover's own state.
    expect(EFFORT_PICKER_CSS).toContain(
      ".effort-popover:hover .effort-spark,\n"
      + ".effort-popover:focus-within .effort-spark,\n"
      + ".effort-popover:hover .effort-track-fill::after,\n"
      + ".effort-popover:focus-within .effort-track-fill::after,\n"
      + ".effort-popover.is-maximum:hover .effort-thumb,\n"
      + ".effort-popover.is-maximum:focus-within .effort-thumb {\n"
      + "  animation-play-state:running; }",
    );
    expect(EFFORT_PICKER_CSS.match(/animation-play-state:running/gu) ?? [])
      .toHaveLength(1);
    // The one-shot entrance sweep is NOT gated: it plays once as the fill
    // arrives and is over before a pointer could reach the surface.
    const sweep = EFFORT_PICKER_CSS.indexOf("animation:effort-fill-sweep 380ms ease-out");
    expect(EFFORT_PICKER_CSS.slice(sweep, EFFORT_PICKER_CSS.indexOf("}", sweep)))
      .not.toContain("animation-play-state");
  });

  /* The top rung's extra polish: a luster band crossing the ramp and a thumb
     that breathes on the twinkle's cadence. Reduced motion must REMOVE the
     band — a parked highlight reads as a smudge, not a flourish — while the
     thumb keeps a still halo so the tier is still legible without motion. */
  it("adds a sheen and a breathing thumb at the top tier, both stilled", () => {
    expect(EFFORT_PICKER_CSS).toMatch(
      /\.effort-track-fill::after \{[^}]*animation:effort-sheen 4s ease-in-out/);
    expect(EFFORT_PICKER_CSS)
      .toContain("color-mix(in srgb, var(--accent-fg) 32%, transparent)");
    expect(EFFORT_PICKER_CSS).toMatch(
      /\.effort-popover\.is-maximum \.effort-thumb \{\s*animation:effort-thumb-glow 2\.6s/);
    // Every glow frame leads with the elevation shadow, so the disc never
    // flattens onto the track mid-breath.
    expect(EFFORT_PICKER_CSS).toMatch(
      /@keyframes effort-thumb-glow \{[^@]*box-shadow:var\(--overlay-shadow\),/);

    const reduced = EFFORT_PICKER_CSS.slice(
      EFFORT_PICKER_CSS.indexOf("@media (prefers-reduced-motion: reduce)"));
    expect(reduced)
      .toContain(".effort-track-fill::after { content:none; animation:none; }");
    expect(reduced)
      .toContain(".effort-popover.is-maximum .effort-thumb { animation:none;");
    expect(reduced).toContain(
      "0 0 8px 1px color-mix(in srgb, var(--accent) 40%, transparent); }");
  });

  /* Every colour in this surface stays a token — the accent dot, the thumb's
     ground and elevation, the track — so a retheme remains a single-file edit
     in theme.css. */
  it("draws dots, thumb, and track from tokens, with no colour literal left", () => {
    expect(EFFORT_PICKER_CSS).not.toMatch(/#[0-9a-f]{3,8}\b/i);
    expect(EFFORT_PICKER_CSS).not.toMatch(/\brgba?\(/i);
    expect(EFFORT_PICKER_CSS).toMatch(
      /\.effort-dot\.is-maximum \{[^}]*background:var\(--accent-text\); \}/);
    expect(EFFORT_PICKER_CSS).toContain(".effort-thumb");
    expect(EFFORT_PICKER_CSS).toContain("box-shadow:var(--overlay-shadow)");
    expect(EFFORT_PICKER_CSS).toContain("border-radius:var(--radius-pill)");
  });
});
