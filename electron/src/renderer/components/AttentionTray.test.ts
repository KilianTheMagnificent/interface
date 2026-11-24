import { readFileSync } from "node:fs";
import { createElement, createRef } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AttentionTray, type AttentionTrayProps } from "./AttentionTray";
import { WorkspaceDestinationItems, WorkspaceMoreMenu } from "./WorkspaceMoreMenu";
import type { AttentionRow } from "../attention-tray";

const source = readFileSync(new URL("./AttentionTray.tsx", import.meta.url), "utf8");
const codiconCss = readFileSync(new URL("../codicons.css", import.meta.url), "utf8");
const NOW = Date.parse("2026-09-05T12:04:00.000Z");
const noop = (): void => undefined;

const rows: AttentionRow[] = [
  { id: "session:claude#a", kind: "needs-input", target: { kind: "session", key: "claude#a" },
    title: "Approve the migration", where: "interface", at: NOW - 60_000, live: true },
  { id: "terminal:t1", kind: "needs-input", target: { kind: "terminal", terminalId: "t1" },
    title: "gpt-6-astra", where: "Workspace 2", at: NOW - 120_000, live: true },
  { id: "done:claude#b:1", kind: "failed", target: { kind: "session", key: "claude#b" },
    title: "Nightly build", where: "ledger", at: NOW - 3_600_000, live: false },
  { id: "done:claude#c:1", kind: "finished", target: { kind: "session", key: "claude#c" },
    title: "Rewrite the parser", where: "", at: NOW - 7_200_000, live: false },
];

const tray = (over: Partial<AttentionTrayProps> = {}): string =>
  renderToStaticMarkup(createElement(AttentionTray, {
    rows, unread: 0, open: false, onOpenChange: noop, onActivate: noop, onClearAll: noop, now: NOW,
    ...over,
  }));

describe("the attention tray opened from the ··· menu (owner, 2026-09-26)", () => {
  it("draws no bell of its own; the ··· button carries its count and says it", () => {
    const anchored = tray({ unread: 2, anchorRef: createRef<HTMLButtonElement>() });
    expect(anchored).not.toContain("codicon-bell");
    expect(anchored).not.toContain("<button");
    const more = (unread: number, shown = rows): string =>
      renderToStaticMarkup(createElement(WorkspaceMoreMenu, {
        buttonRef: createRef<HTMLButtonElement>(), inert: false, onCommands: noop,
        attention: { rows: shown, unread, onOpen: noop },
      }));
    const live = more(2);
    // The name stays stable; the badge's meaning is its description.
    expect(live).toContain('aria-label="More workspace options"');
    const describedBy = /aria-describedby="([^"]+)"/u.exec(live)?.[1] ?? "";
    expect(describedBy).not.toBe("");
    expect(live).toContain(`id="${describedBy}" class="terminal-sr-only">Attention: 4 items, 2 new<`);
    expect(live).toContain('class="attention-tray-count is-unread is-live" aria-hidden="true">2<');
    // Nothing live but something new: the quiet dot, no number.
    const settled = more(1, rows.filter(row => !row.live));
    expect(settled).toContain('class="attention-tray-dot"');
    expect(settled).not.toContain("attention-tray-count");
    // Nothing at all: a plain ··· button.
    const quiet = more(0, []);
    expect(quiet).not.toContain("attention-tray-dot");
    expect(quiet).not.toContain("attention-tray-count");
  });
});

describe("the attention tray", () => {
  it("is a bell with the count, named for a screen reader by count and novelty", () => {
    const closed = tray({ unread: 2 });
    expect(closed).toContain("codicon-bell");
    expect(closed).toContain('aria-label="Attention: 4 items, 2 new"');
    // The number is the two live needs, in amber; the two finished rows wait
    // inside the tray (owner, 2026-09-12).
    expect(closed).toContain('class="attention-tray-count is-unread is-live" aria-hidden="true">2<');
    expect(closed).not.toContain('class="attention-tray-dot"');
    // Nothing live but something new: a quiet dot, no number.
    const settled = tray({ rows: rows.filter(row => !row.live), unread: 1 });
    expect(settled).not.toContain('class="attention-tray-count');
    expect(settled).toContain('class="attention-tray-dot"');
    expect(closed).toContain('aria-haspopup="menu" aria-expanded="false"');
    expect(closed).not.toContain('role="menu"');
    // Nothing to say: no badge, and the name says so.
    const quiet = tray({ rows: [] });
    expect(quiet).not.toContain('class="attention-tray-count');
    expect(quiet).not.toContain('class="attention-tray-dot"');
    expect(quiet).toContain('aria-label="Attention: nothing needs you"');
    expect(codiconCss).toContain(".codicon-bell::before");
  });

  it("lists every row with its state glyph, title, place and age, and a way to clear the events", () => {
    const html = tray({ open: true });
    expect(html).toContain('role="menu" aria-label="Attention"');
    expect(html.match(/role="menuitem"/g)).toHaveLength(4);
    // Shape before hue: the same three glyphs the sidebar and pane heads use.
    expect(html).toContain("codicon-warning attention-tray-row-glyph is-needs-input");
    expect(html).toContain("codicon-error attention-tray-row-glyph is-failed");
    expect(html).toContain("codicon-pass attention-tray-row-glyph is-finished");
    expect(html).toContain('class="attention-tray-row-title">Approve the migration<');
    expect(html).toContain('class="attention-tray-row-meta">Needs attention · interface · 1m<');
    expect(html).toContain('class="attention-tray-row-meta">Needs attention · Workspace 2 · 2m<');
    expect(html).toContain('class="attention-tray-row-meta">Failed · ledger · 1h<');
    // No place known: no dash, no empty segment.
    expect(html).toContain('class="attention-tray-row-meta">Finished · 2h<');
    // The row's accessible name is its two lines.
    expect(html).toContain('aria-label="Nightly build, Failed · ledger · 1h"');
    expect(html).toContain('aria-label="Clear finished and failed"');
    // One tab stop for the list: the first row.
    expect(html.match(/role="menuitem"[^>]*tabindex="0"/g)).toHaveLength(1);
  });

  it("keeps one tab stop when finished rows offer Inspect result", () => {
    const html = tray({ open: true, onInspect: noop });
    const inspect = html.match(/<button[^>]*>Inspect result for [^<]*<\/button>/g) ?? [];
    // The two event rows, and not the two live ones.
    expect(inspect).toHaveLength(2);
    for (const button of inspect) {
      expect(button).toContain('type="button"');
      expect(button).toContain('role="menuitem"');
      expect(button).toContain('tabindex="-1"');
      expect(button).toContain('class="attention-tray-inspect"');
    }
    expect(html.match(/role="menuitem"[^>]*tabindex="0"/g)).toHaveLength(1);
    // Only menuitems sit inside the menu: each row's wrapper is presentational.
    expect(html.match(/<div role="none">/g)).toHaveLength(rows.length);
    expect(source).toMatch(/\.attention-tray-clear,\n\.attention-tray-inspect \{/u);
  });

  it("says so when nothing needs the user, and offers no Clear for a list of live rows", () => {
    const empty = tray({ rows: [], open: true });
    expect(empty).toContain("Nothing needs you right now.");
    expect(empty).not.toContain("Clear all");
    const liveOnly = tray({ rows: rows.filter(row => row.live), open: true });
    expect(liveOnly).not.toContain("Clear all");
  });

  it("reuses the app's menu popover and its rows, and portals the panel", () => {
    expect(source).toContain("useTerminalMenuPopover({");
    expect(source).toContain('className="terminal-menu-launch attention-tray-row"');
    expect(source).toContain("createPortal(panel, document.body)");
    // Activating a row goes there and closes the tray; the caller decides
    // whether that also dismisses an event.
    expect(source).toMatch(/onActivate\(row\);\s*close\(true\);/u);
    // The bell is the toolbar's own button primitive.
    expect(source).toContain('"workspace-icon-btn attention-tray-button"');
  });
});

describe("the ··· menu's destinations (review C3, 2026-10-14)", () => {
  it("reaches the Coordination canvas, Worktrees and the Context inventory from ···", () => {
    const chosen: string[] = [];
    const html = renderToStaticMarkup(createElement(WorkspaceDestinationItems, {
      choose: action => { action(); },
      onCanvas: () => { chosen.push("canvas"); },
      onWorktrees: () => { chosen.push("worktrees"); },
      onContextInventory: () => { chosen.push("context"); },
    }));
    expect(html.match(/role="menuitem"/gu)).toHaveLength(3);
    expect(html.indexOf("Coordination canvas")).toBeLessThan(html.indexOf("Worktrees"));
    expect(html.indexOf("Worktrees")).toBeLessThan(html.indexOf("Context inventory…"));
    expect(html).toContain("codicon-type-hierarchy-sub");
    // Unwired destinations are not drawn.
    expect(renderToStaticMarkup(createElement(WorkspaceDestinationItems, { choose: noop })))
      .toBe("");
    const app = readFileSync(new URL("../App.tsx", import.meta.url), "utf8");
    expect(app).toContain("onCanvas={showCanvasFromMenu}");
    expect(app).toContain('const openWorktreesFromMenu = useCommittedCallback(() => { openSessionLibrary("worktrees"); });');
    expect(app).toContain('const openContextInventoryFromMenu = useCommittedCallback(() => { openTaskInspector("context"); });');
    expect(chosen).toEqual([]);
  });
});
