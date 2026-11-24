import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CornerNotices, type CornerNoticeItem } from "./components/CornerNotices";
import {
  CORNER_NOTICE_LIMIT,
  cornerLift,
  foldCornerNotices,
  orderNewestFirst,
  refreshErrorText,
  refreshFailureDetail,
  refreshFailureId,
  refreshFailureSentence,
  type RefreshFailure,
} from "./corner-notices";
import { providerLabel } from "./provider-labels";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const css = readFileSync(new URL("./theme.css", import.meta.url), "utf8");

const codexStale: RefreshFailure = { kind: "provider", engine: "codex", stale: true,
  line: "codex: Interface could not refresh this provider's session list." };

describe("refresh failures in plain words", () => {
  it("names the provider by its display name in one sentence", () => {
    expect(refreshFailureSentence(codexStale, engine => providerLabel(engine), "Interface"))
      .toBe("Codex could not refresh its sessions.");
    expect(refreshFailureSentence({ ...codexStale, stale: false }, providerLabel, "Interface"))
      .toBe("Codex could not list all of its sessions.");
    expect(refreshFailureSentence({ kind: "listing", line: "session refresh: host gone" },
      providerLabel, "Interface Dev")).toBe("Interface Dev could not refresh the session list.");
    expect(refreshFailureSentence({ kind: "health", line: "engine health refresh: x" },
      providerLabel, "Interface")).toBe("Interface could not check the providers.");
    expect(refreshFailureSentence({ kind: "capability", line: "engine capability refresh: x" },
      providerLabel, "Interface")).toBe("Interface could not read what each provider supports.");
  });

  it("keeps the old one-line error for Settings and the raw text as the tooltip", () => {
    const health: RefreshFailure = { kind: "health", line: "engine health refresh: Probe timed out" };
    expect(refreshErrorText([])).toBeNull();
    expect(refreshErrorText([codexStale, health])).toBe(
      "codex: Interface could not refresh this provider's session list. engine health refresh: Probe timed out");
    expect(refreshFailureDetail(health)).toBe("Probe timed out");
    expect(refreshFailureDetail(codexStale)).toBe("Interface could not refresh this provider's session list.");
  });

  it("gives each failure an id that survives its neighbours and is new next refresh", () => {
    const again: RefreshFailure = { ...codexStale };
    expect(refreshFailureId(codexStale)).toBe(refreshFailureId(codexStale));
    expect(refreshFailureId(again)).not.toBe(refreshFailureId(codexStale));
  });
});

describe("the corner stack", () => {
  it("orders newest first, keeps a batch in its own order, and forgets what left", () => {
    let step = orderNewestFirst(["a"], new Map(), 0);
    expect(step.order).toEqual(["a"]);
    step = orderNewestFirst(["a", "b", "c"], step.seen, step.next);
    expect(step.order).toEqual(["b", "c", "a"]);
    step = orderNewestFirst(["b", "c"], step.seen, step.next);
    step = orderNewestFirst(["a", "b", "c"], step.seen, step.next);
    expect(step.order).toEqual(["a", "b", "c"]);
  });

  it("shows three at most and folds the rest into N more", () => {
    expect(CORNER_NOTICE_LIMIT).toBe(3);
    expect(foldCornerNotices([1, 2, 3], false)).toEqual({ shown: [1, 2, 3], folded: [] });
    expect(foldCornerNotices([1, 2, 3, 4, 5], false)).toEqual({ shown: [1, 2, 3], folded: [4, 5] });
    expect(foldCornerNotices([1, 2, 3, 4, 5], true)).toEqual({ shown: [1, 2, 3, 4, 5], folded: [] });
  });

  it("rises above a composer under the corner and ignores one beside it", () => {
    const area = { top: 0, bottom: 800, left: 0, right: 1200 };
    expect(cornerLift(area, 400, [])).toBe(20);
    // A centred composer reaching under the stack's columns.
    expect(cornerLift(area, 400, [{ top: 660, bottom: 780, left: 300, right: 900 }])).toBe(150);
    // One that ends left of the stack does not move it.
    expect(cornerLift(area, 400, [{ top: 660, bottom: 780, left: 100, right: 700 }])).toBe(20);
    // Never off the top of the area.
    expect(cornerLift(area, 400, [{ top: -50, bottom: 790, left: 0, right: 1200 }])).toBe(780);
  });

  const item = (id: string, over: Partial<CornerNoticeItem> = {}): CornerNoticeItem => ({
    id, message: `Message ${id}`, tone: "error", dismissLabel: "Dismiss refresh error",
    onDismiss: () => undefined, ...over,
  });

  it("draws a dot, the words, Try again only with a retry, and Dismiss", () => {
    const markup = renderToStaticMarkup(createElement(CornerNotices, { items: [
      item("r", { message: "Codex could not refresh its sessions.", onRetry: () => undefined,
        detail: "Interface could not refresh this provider's session list." }),
      item("s", { tone: "status", message: "Restored 2 files.", dismissLabel: "Dismiss restore result" }),
    ] }));
    expect(markup).toContain('<section class="corner-notices" aria-label="Notices"');
    expect(markup).toContain('<div class="corner-notice is-error" role="alert" data-corner-notice="r" title="Interface could not refresh this provider&#x27;s session list.">'
      + '<span class="corner-notice-dot" aria-hidden="true"></span><span class="corner-notice-text">Codex could not refresh its sessions.</span>'
      + '<button type="button" class="corner-notice-action">Try again</button>'
      + '<button type="button" class="corner-notice-action corner-notice-dismiss" aria-label="Dismiss refresh error">Dismiss</button></div>');
    expect(markup).toContain('<div class="corner-notice is-status" role="status" data-corner-notice="s">');
    expect(markup.match(/Try again/gu)).toHaveLength(1);
    expect(markup).not.toContain("corner-notice-more");
    expect(renderToStaticMarkup(createElement(CornerNotices, { items: [] }))).toBe("");
  });

  it("folds the oldest past three, unseen and still counting down", () => {
    const markup = renderToStaticMarkup(createElement(CornerNotices, {
      items: ["1", "2", "3", "4", "5"].map(id => item(id)) }));
    expect(markup.match(/class="corner-notice is-error"/gu)).toHaveLength(3);
    expect(markup.match(/class="corner-notice is-error is-folded" role="alert" data-corner-notice="\d" inert=""/gu)).toHaveLength(2);
    expect(markup).toContain('<button type="button" class="corner-notice-more" aria-expanded="false">2 more</button>');
    expect(css).toMatch(/\.corner-notice\.is-folded \{[^}]*clip-path: inset\(50%\);/u);
    expect(css).not.toMatch(/\.corner-notice\.is-folded \{[^}]*display: none/u);
  });

  it("sits in the bottom-right corner on the theme's own surface", () => {
    expect(css).toMatch(/\.corner-notices \{[^}]*position: absolute;[^}]*right: 20px;[^}]*bottom: var\(--corner-notice-lift, 20px\);/u);
    expect(css).toMatch(/\.corner-notice \{[^}]*background: var\(--surface\);[^}]*border: var\(--border-w\) solid var\(--border-strong\);[^}]*border-radius: var\(--radius-overlay\);[^}]*box-shadow: var\(--overlay-shadow\);/u);
    expect(css).toMatch(/\.corner-notice-dot \{[^}]*background: var\(--danger-fg\);/u);
    // Under the composer panel and Settings, like the notices above it.
    expect(app).toContain("<CornerNotices items={cornerNotices}\n          inert={state.composerOpen || settingsOpen} />");
  });

  it("retries a failed refresh the way Settings' Refresh does", () => {
    expect(app).toContain('onRetry: () => { void refreshAll("explicit"); },');
    expect(app).toContain("message: refreshFailureSentence(failure, name, productLabel()),");
    // Only a refresh has a retry today.
    expect(app.match(/onRetry:/gu)).toHaveLength(1);
  });

  it("marks the hidden sidebar's toggle while providers need setup", () => {
    expect(app).toContain("{!sidebarOpen && providerAlert !== undefined && (\n            <span className=\"sidebar-toggle-setup-dot\" aria-hidden=\"true\" />");
    expect(app).toContain("aria-description={!sidebarOpen && providerAlert !== undefined ? providerAlert.label : undefined}");
    expect(css).toMatch(/\.sidebar-toggle-setup-dot \{[^}]*background: var\(--status-waiting\);/u);
  });
});
