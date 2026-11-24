import { createElement, createRef } from "react";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { buildUsageView } from "../viewmodels";
import { UsageMenu } from "./UsageMenu";
import type { EngineUsage } from "../../shared/engine";

const NOW = Date.parse("2026-08-12T12:00:00.000Z");
const ENGINES = [
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "acp:grok", label: "Grok" },
];

const ready = {
  status: "ready" as const,
  views: buildUsageView([
    { engine: "claude", available: true, plan: "max", windows: [
      { label: "5-hour", utilization: 42, resetsAt: "2026-08-12T16:00:00.000Z" },
    ] },
    { engine: "codex", available: true, plan: "pro", windows: [
      { label: "weekly", utilization: 18, resetsAt: null },
    ] },
    { engine: "acp:grok", available: true, plan: "SuperGrok Plus",
      note: "Shared account usage across Grok products.", windows: [
        { label: "Shared weekly pool", utilization: 16,
          resetsAt: "2026-08-19T12:00:00.000Z" },
      ] },
  ]),
  error: null,
  readAt: NOW,
};

describe("UsageMenu", () => {
  it("shows OpenCode Go's three account windows, resets, and fullest-window warning", () => {
    const usage = JSON.parse(readFileSync(new URL("../../fixtures/opencode-go-usage-projection-v1.json", import.meta.url), "utf8")) as EngineUsage;
    const now = Date.parse("2026-09-07T19:00:00.000Z");
    const html = renderToStaticMarkup(createElement(UsageMenu, {
      state: { status: "ready", views: buildUsageView([usage]), error: null, readAt: now },
      now, open: true, engines: [], onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    expect(html).toContain("OpenCode 100%");
    expect(html).toContain("Go plan");
    expect(html).toContain("5-hour");
    expect(html).toContain("Weekly");
    expect(html).toContain("Monthly");
    expect(html).toContain("13% used");
    expect(html).toContain("34% used");
    expect(html).toContain("100% used");
    expect(html).toContain("resets in 1h");
    expect(html).toContain('class="usage-bar-fill is-full"');
    expect(html).not.toContain("this session");
  });

  it("keeps unavailable OpenCode Go usage unknown and marks stale known values", () => {
    const usage = JSON.parse(readFileSync(new URL("../../fixtures/opencode-go-usage-projection-v1.json", import.meta.url), "utf8")) as EngineUsage;
    const render = (value: EngineUsage) => renderToStaticMarkup(createElement(UsageMenu, {
      state: { status: "ready", views: buildUsageView([value]), error: null, readAt: NOW - 20 * 60_000 },
      now: NOW, open: true, engines: [], onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    expect(render(usage)).toContain("OpenCode ~100%");
    const unavailable = render({ engine: "acp:opencode", available: false, plan: "Go", windows: [],
      note: "OpenCode Go usage could not be read." });
    expect(unavailable).toContain("Not reported.");
    expect(unavailable).not.toContain("0%");
    expect(unavailable).not.toContain('class="usage-bar"');
  });

  it("shows a single usage trigger without per-provider chips", () => {
    const html = renderToStaticMarkup(createElement(UsageMenu, {
      state: ready, now: NOW, open: false, engines: ENGINES,
      onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    expect(html).toContain('class="usage-menu"');
    expect(html).toContain('aria-label="Provider usage"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain('class="codicon codicon-dashboard"');
    expect(html).toContain('class="usage-menu-label">Usage<');
    expect(html).toContain("Claude Code 42%");
    expect(html).toContain("Codex 18%");
    expect(html).toContain("Grok 16%");
    expect(html).not.toContain("usage-chip");
    expect(html).not.toContain("data-provider-logo");
    expect(html).not.toContain('class="usage-menu-panel"');
    expect(html).not.toContain("Asking each engine");
    const inner = /class="usage-menu-trigger"[^>]*>([\s\S]*?)<\/button>/.exec(html)?.[1] ?? "";
    expect(inner).not.toContain("42%");
    expect(inner).not.toContain("Claude");
  });

  it("opens a flat provider summary with every provider's windows instead of nested cards", () => {
    const html = renderToStaticMarkup(createElement(UsageMenu, {
      state: ready, now: NOW, open: true, engines: ENGINES,
      onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    expect(html).toContain('aria-expanded="true"');
    expect(html).toContain('class="usage-menu-panel is-floating"');
    expect(html).toContain('aria-label="Provider usage details"');
    expect(html).toContain("5-hour");
    expect(html).toContain("weekly");
    expect(html).toContain("Shared weekly pool");
    expect(html).toContain("max plan");
    expect(html).toContain("SuperGrok Plus plan");
    expect(html).toContain("42% used");
    expect(html).toContain("16% used");
    expect(html).not.toContain("data-provider-logo");
    expect((html.match(/class="usage-provider"/g) ?? [])).toHaveLength(3);
    expect(html).not.toContain('class="usage-card"');
    expect(html).not.toContain("usage-card-head");
    expect(html).toContain("usage-provider-head");
    expect(html).toMatch(/<section class="usage-provider"[^>]+aria-labelledby="[^"]+"/);
    expect(html).toContain("Shared account usage across Grok products.");
    expect(html).toContain("Refresh");
    expect(html).not.toContain(">Close<");
  });

  /* The panel announced itself as a dialog while behaving as a disclosure: it
     never received focus and trapped nothing, so a screen-reader user was told
     a dialog had opened and then left standing outside it. The honest shape is
     the one the component implements — an expanded region owned by its
     trigger, with Refresh as the next thing in tab order. */
  it("announces the panel as the disclosure it is, not as a dialog", () => {
    const render = (open: boolean): string => renderToStaticMarkup(createElement(UsageMenu, {
      state: ready, now: NOW, open, engines: ENGINES,
      onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    const closed = render(false);
    expect(closed).not.toContain("aria-haspopup");
    expect(closed).not.toContain("aria-controls");
    expect(closed).not.toContain('role="dialog"');

    const html = render(true);
    expect(html).not.toContain("aria-haspopup");
    expect(html).not.toContain('role="dialog"');
    expect(html).toContain('role="group"');
    const controls = /aria-controls="([^"]+)"/.exec(html);
    expect(controls).not.toBeNull();
    expect(html).toContain(`id="${controls?.[1] ?? ""}" class="usage-menu-panel is-floating"`);
    // Refresh stays inside that panel, so Tab from the trigger reaches it.
    expect(html.indexOf("Refresh")).toBeGreaterThan(html.indexOf('class="usage-menu-panel is-floating"'));
  });

  it("keeps provider names in the trigger title before the first read", () => {
    const html = renderToStaticMarkup(createElement(UsageMenu, {
      state: { status: "idle", views: [], error: null, readAt: null },
      now: NOW, open: false, engines: ENGINES,
      onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    expect(html).toContain("title=\"Claude Code · Codex · Grok\"");
    expect(html).not.toContain("usage-chip");
    expect(html).not.toContain("%");
    const inner = /class="usage-menu-trigger"[^>]*>([\s\S]*?)<\/button>/.exec(html)?.[1] ?? "";
    expect(inner).not.toContain("Claude");
  });

  it("says an old reading is old in words, and never in the attention hue", () => {
    // Amber means one thing in this app: an agent is blocked on you. A snapshot
    // read twenty minutes ago is not that, so the stale state is carried by the
    // sentence and by the chips dropping their percents — there is no
    // `is-stale` class left to colour.
    const html = renderToStaticMarkup(createElement(UsageMenu, {
      state: { ...ready, readAt: NOW - 20 * 60_000 },
      now: NOW, open: true, engines: ENGINES,
      onOpenChange: () => undefined, onRefresh: () => undefined,
    }));
    expect(html).toContain("may have moved since");
    expect(html).toContain('class="usage-read"');
    expect(html).not.toContain("is-stale");
  });

  it("opens from the ··· menu without a trigger of its own (owner, 2026-09-26)", () => {
    const html = renderToStaticMarkup(createElement(UsageMenu, {
      state: { status: "ready", views: [], error: null, readAt: null },
      now: 0, open: true, engines: [], onOpenChange: () => undefined, onRefresh: () => undefined,
      anchorRef: createRef<HTMLButtonElement>(),
    }));
    expect(html).not.toContain("usage-menu-trigger");
    expect(html).toContain('aria-label="Provider usage details"');
  });
});
