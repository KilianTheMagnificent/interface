import { describe, expect, it } from "vitest";
import { fallbackEffortLevels } from "./app-session-projection";

/* The rung this pins is the one a restored pane loses. `sessionConfigFor` reads
   the effort ladder off the session model's catalogue row, and a pane restored
   at boot has no `models.available` event yet; when the probe catalogue is also
   absent the ladder falls back to a hardcoded list. That fallback has to mirror
   the Claude adapter's own `withUltracode` rule or the top rung silently
   disappears until the session goes live.

   The contract asserted here is MEMBERSHIP, not position: EffortBar's
   `orderEffortLevels` canonicalizes the rungs before they are drawn, so a list
   that merely contains "ultracode" already renders it last. The helper still
   appends it at the end so its shape matches the adapter output it stands in
   for. */
describe("fallbackEffortLevels", () => {
  it("gives a cold Claude pane the ultracode rung, above xhigh", () => {
    const levels = fallbackEffortLevels("claude");
    expect(levels).toContain("ultracode");
    expect(levels[levels.length - 1]).toBe("ultracode");
    expect(levels.indexOf("xhigh")).toBeGreaterThanOrEqual(0);
    expect(levels.indexOf("xhigh")).toBeLessThan(levels.indexOf("ultracode"));
  });

  it("hands back a fresh array each call, sharing no mutable state", () => {
    const first = fallbackEffortLevels("claude");
    const second = fallbackEffortLevels("claude");
    expect(first).not.toBe(second);
    expect(first).toEqual(second);
    first.push("bogus");
    expect(fallbackEffortLevels("claude")).not.toContain("bogus");
  });

  it("never guesses ultracode for an engine that advertises its own top rung", () => {
    // Codex's "ultra" is advertised per-model by app-server, so guessing it
    // here would put a rung in front of the user the CLI can only reject.
    // Grok's live menu tops out at xhigh.
    expect(fallbackEffortLevels("codex")).not.toContain("ultracode");
    expect(fallbackEffortLevels("acp:grok")).not.toContain("ultracode");
  });
});
