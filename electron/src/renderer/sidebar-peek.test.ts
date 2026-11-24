import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { SIDEBAR_PEEK_CLOSE_MS, SIDEBAR_PEEK_KEEP_SELECTOR, SIDEBAR_PEEK_OPEN_MS, SidebarPeek,
  type PeekClock } from "./sidebar-peek";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const theme = readFileSync(new URL("./theme.css", import.meta.url), "utf8");

/** A clock that only moves when the test says so. */
function fakeClock(): PeekClock & { advance(ms: number): void; pending(): number } {
  let now = 0, next = 1;
  const timers = new Map<number, { at: number; run: () => void }>();
  return {
    set(run, ms) { timers.set(next, { at: now + ms, run }); return next++; },
    clear(handle) { timers.delete(handle); },
    advance(ms) {
      const until = now + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, timer]) => timer.at <= until)
          .sort(([, a], [, b]) => a.at - b.at)[0];
        if (due === undefined) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].run();
      }
      now = until;
    },
    pending: () => timers.size,
  };
}

function peekWith(held = (): boolean => false) {
  const clock = fakeClock();
  const changes: boolean[] = [];
  const peek = new SidebarPeek(shown => { changes.push(shown); }, () => held(), clock);
  return { peek, clock, changes };
}

describe("hidden sidebar hover reveal", () => {
  it("slides the hidden sidebar out after the pointer rests on its toggle, and away once it leaves", () => {
    const { peek, clock, changes } = peekWith();
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS - 1);
    expect(changes).toEqual([]);
    clock.advance(1);
    expect(changes).toEqual([true]);
    peek.enter();
    peek.leave();
    clock.advance(SIDEBAR_PEEK_CLOSE_MS - 1);
    expect(changes).toEqual([true]);
    clock.advance(1);
    expect(changes).toEqual([true, false]);
  });

  it("ignores a pointer that only crosses the toggle, or arrives dragging", () => {
    const { peek, clock, changes } = peekWith();
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS / 2);
    peek.triggerLeave();
    clock.advance(SIDEBAR_PEEK_OPEN_MS * 4);
    // A pane selection dragged up across the rail holds the primary button.
    peek.triggerEnter(1);
    clock.advance(SIDEBAR_PEEK_OPEN_MS * 4);
    expect(changes).toEqual([]);
    peek.triggerLeave();
    // Released, the next arrival counts.
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS);
    expect(changes).toEqual([true]);
  });

  it("does nothing while the sidebar is docked, so hiding it under the pointer never brings it straight back", () => {
    const { peek, clock, changes } = peekWith();
    peek.setEnabled(false);
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS * 4);
    // The click hid it with the pointer still on the toggle: no new arrival.
    peek.setEnabled(true);
    clock.advance(SIDEBAR_PEEK_OPEN_MS * 4);
    expect(changes).toEqual([]);
    peek.triggerLeave();
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS);
    expect(changes).toEqual([true]);
    // Docking it again slides the reveal away at once.
    peek.setEnabled(false);
    expect(changes).toEqual([true, false]);
    expect(clock.pending()).toBe(0);
  });

  it("stays out when the pointer comes back within the grace", () => {
    const { peek, clock, changes } = peekWith();
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS);
    peek.leave();
    clock.advance(SIDEBAR_PEEK_CLOSE_MS - 10);
    peek.enter();
    clock.advance(SIDEBAR_PEEK_CLOSE_MS * 4);
    expect(changes).toEqual([true]);
    // Back on the toggle counts as inside too.
    peek.leave();
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_CLOSE_MS * 4);
    expect(changes).toEqual([true]);
  });

  it("waits for an open menu, confirmation or search to finish before sliding away", () => {
    let held = true;
    const { peek, clock, changes } = peekWith(() => held);
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS);
    peek.leave();
    clock.advance(SIDEBAR_PEEK_CLOSE_MS * 5);
    expect(changes).toEqual([true]);
    held = false;
    clock.advance(SIDEBAR_PEEK_CLOSE_MS);
    expect(changes).toEqual([true, false]);
    expect(clock.pending()).toBe(0);
  });

  it("goes at once when dismissed, and reports nothing after it is disposed", () => {
    const { peek, clock, changes } = peekWith();
    peek.triggerEnter(0);
    clock.advance(SIDEBAR_PEEK_OPEN_MS);
    peek.dismiss();
    expect(changes).toEqual([true, false]);
    peek.dismiss();
    expect(changes).toEqual([true, false]);
    peek.triggerEnter(0);
    peek.dispose();
    clock.advance(SIDEBAR_PEEK_OPEN_MS * 4);
    expect(changes).toEqual([true, false]);
    expect(clock.pending()).toBe(0);
  });

  it("counts the panel's portaled menus, hover card and the rail toggle as inside", () => {
    const keep = SIDEBAR_PEEK_KEEP_SELECTOR.split(", ");
    for (const owned of [".sidebar", ".row-menu", ".session-peek", "[data-sidebar-peek-keep]"]) {
      expect(keep).toContain(owned);
    }
    // The window's left edge no longer reveals it (owner, 2026-10-20).
    expect(keep).not.toContain(".sidebar-peek-edge");
    expect(app).not.toContain("sidebar-peek-edge");
    expect(theme).not.toContain("sidebar-peek-edge");
    // The rail toggle is the trigger and opts in, so a press on it pins
    // rather than dismisses.
    expect(app).toMatch(/ref=\{sidebarToggleRef\} type="button"\n\s+data-sidebar-peek-keep\n[^>]*\{\.\.\.sidebarPeek\.trigger\}>/u);
  });

  it("puts the toggle just right of the traffic lights, over the panel it brings back", () => {
    const leading = app.slice(app.indexOf('<div className="workspace-toolbar-leading">'));
    const order = ['className="workspace-toolbar-traffic"', "<button ref={sidebarToggleRef}", 'className="workspace-brand"']
      .map(mark => leading.indexOf(mark));
    expect(order.every(at => at >= 0)).toBe(true);
    expect(order).toEqual([...order].sort((a, b) => a - b));
    // It no longer rides to the sidebar's right edge.
    expect(theme).not.toMatch(/\.workspace-sidebar-toggle\s*\{[^}]*margin-left/u);
  });

  it("floats the revealed panel on the hidden column's track, so no pane moves", () => {
    // Desktop widths only, and only while the toggle has it hidden.
    expect(app).toContain("useSidebarPeek(!sidebarOpen && !mobileLayout && !overlayDialogOpen)");
    expect(app).toContain('+ (sidebarPeek.shown ? " sidebar-peek" : "")');
    const desktop = theme.slice(theme.indexOf("@media (min-width: 841px) {\n  .app-grid > .sidebar {"));
    const rule = desktop.slice(desktop.indexOf(".app-grid.no-sidebar.sidebar-peek > .sidebar {"));
    expect(rule.slice(0, rule.indexOf("}"))).toMatch(/visibility: visible;[\s\S]*translate: var\(--pane-left\) 0;/u);
    // The columns are the hidden ones: the shell keeps its place under it.
    expect(theme).not.toMatch(/\.sidebar-peek[^{]*>\s*\.workspace-shell/u);
    // Working marks in the revealed panel run again.
    expect(theme).toContain(".app-grid.no-sidebar:not(.sidebar-peek) > .sidebar .dot-pulse,");
  });

  it("remembers the toggle across launches on desktop widths", () => {
    expect(app).toContain('const SIDEBAR_KEY = "interface.sidebar.v1"');
    expect(app).toContain('useState(() => readStored(SIDEBAR_KEY) !== "hidden")');
    expect(app).toContain('if (!mobileLayout) writeStored(SIDEBAR_KEY, next ? "shown" : "hidden");');
  });
});
