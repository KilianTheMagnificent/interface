import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { nextPresencePhase, presenceClass } from "./presence";

const theme = readFileSync(new URL("./theme.css", import.meta.url), "utf8");
const terminal = readFileSync(new URL("./terminal.css", import.meta.url), "utf8");
const preview = readFileSync(new URL("./components/OutputPreviewPanel.css", import.meta.url), "utf8");
const classesIn = (text: string): string[] => [...text.matchAll(/\.([a-z][a-z0-9-]*)/gu)].map(([, name]) => name);

describe("presence phases", () => {
  it("only animates a change of open, never the first render", () => {
    expect(nextPresencePhase("still", true, true)).toBe("still");
    expect(nextPresencePhase("still", false, false)).toBe("still");
  });

  it("enters on open and leaves on close, including mid-animation reversals", () => {
    expect(nextPresencePhase("still", false, true)).toBe("entering");
    expect(nextPresencePhase("still", true, false)).toBe("leaving");
    expect(nextPresencePhase("entering", true, false)).toBe("leaving");
    expect(nextPresencePhase("leaving", false, true)).toBe("entering");
  });

  it("shows or hides an instant change in place", () => {
    expect(nextPresencePhase("still", false, true, true)).toBe("still");
    expect(nextPresencePhase("entering", true, false, true)).toBe("still");
  });

  it("keeps the phase while open is unchanged", () => {
    expect(nextPresencePhase("entering", true, true)).toBe("entering");
    expect(nextPresencePhase("leaving", false, false)).toBe("leaving");
  });

  it("names the stylesheet hook for each phase", () => {
    expect(presenceClass("still")).toBe("");
    expect(presenceClass("entering")).toBe(" is-entering");
    expect(presenceClass("leaving")).toBe(" is-leaving");
  });
});

describe("arrival motion", () => {
  const start = theme.indexOf("/* Floating surfaces arrive rather than blink.");
  const arrivals = theme.slice(start, theme.indexOf("@keyframes popover-drop-in", start));
  const reduced = theme.slice(theme.indexOf("/* Arrivals appear in place:"));
  const reducedList = reduced.slice(0, reduced.indexOf("{ animation: none; }"));

  it("gives every animated arrival a reduced-motion counterpart that appears in place", () => {
    expect(start).toBeGreaterThan(0);
    const animated = [...arrivals.matchAll(/:where\(([^)]*(?:\([^)]*\)[^)]*)*)\)\s*\{\s*animation:/gu)]
      .flatMap(([, list]) => classesIn(list.replace(/:not\([^)]*\)/gu, "")));
    expect(animated.length).toBeGreaterThan(20);
    const still = new Set(classesIn(reducedList));
    expect(animated.filter(name => !still.has(name))).toEqual([]);
  });

  it("slides the side panels in and out, and snaps them under reduced motion", () => {
    expect(terminal).toContain("@keyframes agent-overview-in { from { margin-right: calc(-1 * var(--agent-overview-width) - var(--tile-gap));");
    expect(terminal).toMatch(/prefers-reduced-motion: reduce\) \{\s*\.terminal-center\.has-agent-overview > \.agent-overview:is\(\.is-entering, \.is-leaving\) \{ animation: none; \}/u);
    expect(preview).toContain("@keyframes output-preview-in { from { margin-right: calc(-1 * clamp(360px, 43%, 720px) - var(--tile-gap));");
    expect(preview).toMatch(/prefers-reduced-motion: reduce\) \{\s*\.terminal-center\.has-output-preview > \.output-preview:is\(\.is-entering, \.is-leaving\) \{ animation: none; \}/u);
    expect(theme).toContain(".app-grid.is-drawer-leaving .sidebar {");
    expect(theme).toContain("@keyframes detail-pane-out");
  });
});
