import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  FAST_MODE_CSS, FastModeToggle, fastModeIsEnabled, fastModeReasonCopy,
} from "./FastModeToggle";

describe("FastModeToggle", () => {
  it("treats cooldown as an enabled preference that can still be turned off", () => {
    expect(fastModeIsEnabled("off")).toBe(false);
    expect(fastModeIsEnabled("on")).toBe(true);
    expect(fastModeIsEnabled("cooldown")).toBe(true);

    const html = renderToStaticMarkup(createElement(FastModeToggle, {
      state: "cooldown",
      onToggle: () => undefined,
    }));
    expect(html).toContain('aria-pressed="true"');
    expect(html).toContain("is-cooldown");
    expect(html).toContain("resumes on its own");
  });

  it("renders an off state quietly and an on state with the active treatment", () => {
    const off = renderToStaticMarkup(createElement(FastModeToggle, {
      state: "off", onToggle: () => undefined,
    }));
    const on = renderToStaticMarkup(createElement(FastModeToggle, {
      state: "on", onToggle: () => undefined,
    }));
    expect(off).toContain('aria-pressed="false"');
    expect(off).toContain('class="fastmode-toggle"');
    expect(on).toContain('aria-pressed="true"');
    expect(on).toContain('class="fastmode-toggle is-on"');
  });

  it("is a bolt with no label, named for assistive tech", () => {
    const html = renderToStaticMarkup(createElement(FastModeToggle, {
      state: "on", onToggle: () => undefined,
    }));
    // The same bolt the pane header draws, not the font's zap.
    expect(html).toContain('class="fast-bolt"');
    expect(html).not.toContain("codicon-zap");
    expect(html).toContain('aria-label="Fast mode"');
    expect(html).not.toContain(">Fast<");
    expect(html).not.toContain("fastmode-label");
  });

  it("humanizes provider reasons and has a complete reduced-motion fallback", () => {
    expect(fastModeReasonCopy("extra_usage_disabled"))
      .toBe("Enable extra usage in Claude settings to use fast mode");
    expect(fastModeReasonCopy("future_reason"))
      .toBe("Fast mode is unavailable right now");
    expect(FAST_MODE_CSS).toContain("prefers-reduced-motion: reduce");
    expect(FAST_MODE_CSS).toContain("animation:none");
  });
});
