import type { ReactNode } from "react";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { Sidebar, SidebarSessionRow, type SidebarLifecycle, type SidebarSessionRowProps } from "../components/Sidebar";
import type { SessionPeekFacts } from "../components/SessionPeek";
import type { DiffStat } from "../diff-stats";
import type { SidebarGroup, SidebarRow } from "../sidebar-view";
import { assert, frames, wait } from "./fixture-support";

/**
 * The Sidebar's render fan-out, counted per session row with real DOM events:
 * a registry push, a streamed token, a hover, a rename keystroke redraw only
 * the rows whose own props changed, and none of them rebinds the translucent
 * header clipping unless the list's structure changed.
 */
export async function sidebarRowFixture(root: Root): Promise<Record<string, unknown>> {
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.chrome = "refined";
  // React reads a memo's `type` when a row mounts, so patch before mounting.
  const renders = new Map<string, number>();
  const rowType = SidebarSessionRow as unknown as { type: (props: SidebarSessionRowProps) => ReactNode };
  const drawRow = rowType.type;
  rowType.type = props => {
    renders.set(props.row.key, (renders.get(props.row.key) ?? 0) + 1);
    return drawRow(props);
  };
  const Observer = window.ResizeObserver;
  let observers = 0;
  window.ResizeObserver = class extends Observer {
    constructor(callback: ResizeObserverCallback) { super(callback); observers++; }
  };
  const rendered = (key: string): number => renders.get(key) ?? 0;
  const otherRows = (...keys: string[]): number => [...renders]
    .filter(([key]) => !keys.includes(key)).reduce((sum, [, count]) => sum + count, 0);

  const projects = ["Alpha", "Beta", "Gamma"];
  const keys = projects.flatMap(project => Array.from({ length: 6 }, (_, index) => `claude#${project}-${index}`));
  const waitingKey = "claude#Beta-2";
  const titles = new Map(keys.map(key => [key, `Task ${key.slice(7)}`]));
  let stamp = 0;
  let peeks: Record<string, SessionPeekFacts> = {};
  let diffStats: Record<string, DiffStat> = {};
  let generation = 0;
  const selected: string[] = [];
  const renamed: Array<[string, string]> = [];
  const lifecycle: SidebarLifecycle = {
    can: () => true, liveKeys: new Set(), onRename: (key, title) => { renamed.push([key, title]); },
    onDelete: () => undefined, onFork: () => undefined, onEngineArchive: () => undefined,
    onAdopt: () => undefined, onStop: () => undefined, error: null, onDismissError: () => undefined,
  };
  // Every call mints new row objects and new callbacks, as App does on a push.
  const render = (): void => {
    generation++;
    const current = generation;
    const lastActivity = new Date(Date.UTC(2026, 8, 27, 0, 0, stamp)).toISOString();
    const groups: SidebarGroup[] = projects.map(project => {
      const rows: SidebarRow[] = keys.filter(key => key.startsWith(`claude#${project}-`)).map(key => {
        const badge = key === waitingKey ? "waiting" as const : "idle" as const;
        return { key, title: titles.get(key)!, engine: "claude", status: badge, badge,
          cloud: false, selected: false, lastActivity, external: false };
      });
      return { projectName: project, projectPath: `/fixture/sidebar-rows/${project}`, rows,
        counts: { total: rows.length, waiting: rows.filter(row => row.badge === "waiting").length, working: 0 } };
    });
    flushSync(() => root.render(<div style={{ width: 288, height: 360, display: "flex" }}>
      <Sidebar groups={groups} projectCount={groups.length} rateLimits={{}} diffStats={diffStats}
        peeks={peeks} glass lifecycle={lifecycle} hidden={{ external: 0, trivial: 0 }} hideable={0}
        onSelect={key => { selected.push(`${current}:${key}`); }} onNew={() => undefined}
        onNewInProject={() => undefined} onArchive={() => undefined} onToggleFilters={() => undefined} />
    </div>));
  };
  const rowButton = (key: string): HTMLButtonElement => {
    const button = [...document.querySelectorAll<HTMLButtonElement>(".session-row")]
      .find(node => node.querySelector(".session-title")?.textContent === titles.get(key));
    assert(button, `Missing the row for ${key}`); return button;
  };
  const card = (): string => document.querySelector(".session-peek")?.textContent ?? "";
  const key = (target: Element, name: string): void => {
    flushSync(() => target.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true })));
  };
  try {
    render();
    await frames();
    const boundObservers = observers;
    assert(boundObservers > 0, "The translucent header clipping never bound");
    const [first, second, third] = [keys[0], keys[1], keys[7]];

    // A registry push that changes nothing a row draws: new objects, a later
    // activity stamp, new callbacks, new peek and diff records.
    renders.clear();
    stamp++;
    peeks = { [first]: { activity: "Reading the parser" } };
    diffStats = { [first]: { added: 3, removed: 1 } };
    render();
    for (let token = 0; token < 8; token++) {
      peeks = { [first]: { activity: `Reading the parser${".".repeat(token)}` } };
      render();
    }
    await frames();
    const pushRowRenders = otherRows();
    assert(pushRowRenders === 0, `Pushes and streamed tokens redrew ${pushRowRenders} session rows`);

    // A title change redraws that row alone.
    titles.set(second, "Task renamed by the provider");
    render();
    await frames();
    assert(rendered(second) === 1 && otherRows(second) === 0,
      `A title change redrew ${otherRows(second)} other rows (${rendered(second)} for its own)`);

    // Space opens the peek: the row gains its description, the card follows
    // the streamed facts, and no other row is drawn.
    renders.clear();
    rowButton(first).focus();
    key(rowButton(first), " ");
    await frames();
    assert(card().includes("Reading the parser"), `The peek did not open with its facts: ${card()}`);
    assert(rowButton(first).getAttribute("aria-describedby") !== null, "The peeked row lost its description");
    peeks = { [first]: { activity: "Writing the fix" } };
    render();
    await frames();
    assert(card().includes("Writing the fix"), "The open peek did not follow the streamed facts");
    key(rowButton(first), "Escape");
    await frames();
    assert(card() === "", "Escape left the peek open");
    const keyboardPeekOtherRenders = otherRows(first);
    assert(keyboardPeekOtherRenders === 0, `A keyboard peek redrew ${keyboardPeekOtherRenders} other rows`);

    // A pointer rest does the same after its delay, and leaving closes it.
    renders.clear();
    flushSync(() => rowButton(third).dispatchEvent(new PointerEvent("pointerover",
      { bubbles: true, pointerType: "mouse", relatedTarget: document.body })));
    await wait(450);
    await frames();
    assert(rowButton(third).getAttribute("aria-describedby") !== null, "Resting on a row did not open its peek");
    flushSync(() => rowButton(third).dispatchEvent(new PointerEvent("pointerout",
      { bubbles: true, pointerType: "mouse", relatedTarget: document.body })));
    await wait(200);
    await frames();
    assert(card() === "", "Leaving the row left its peek open");
    const hoverOtherRenders = otherRows(third);
    assert(hoverOtherRenders === 0, `A hover redrew ${hoverOtherRenders} other rows`);

    // Rename in place: each keystroke redraws the renamed row, never the list.
    renders.clear();
    const trigger = rowButton(first).parentElement!.querySelector<HTMLButtonElement>(".session-menu")!;
    flushSync(() => trigger.click());
    await frames();
    const rename = [...document.querySelectorAll<HTMLButtonElement>('.row-menu [role="menuitem"]')]
      .find(item => item.textContent?.trim() === "Rename…");
    assert(rename, "The row menu offered no Rename");
    flushSync(() => rename.click());
    await frames();
    const input = document.querySelector<HTMLInputElement>(".session-rename");
    assert(input && document.activeElement === input, "Rename did not open its field");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    for (const next of ["Task A", "Task AB", "Task ABC"]) {
      flushSync(() => { setValue.call(input, next); input.dispatchEvent(new Event("input", { bubbles: true })); });
    }
    await frames();
    const renameOtherRenders = otherRows(first);
    assert(renameOtherRenders === 0, `Opening the menu and typing a rename redrew ${renameOtherRenders} other rows`);
    key(input, "Enter");
    await frames();
    assert(renamed.length === 1 && renamed[0][0] === first && renamed[0][1] === "Task ABC",
      `Rename committed ${JSON.stringify(renamed)}`);

    // A skipped row still calls the caller's current callback.
    flushSync(() => rowButton(third).click());
    assert(selected.at(-1) === `${generation}:${third}`, `The row click reached ${selected.at(-1)}`);
    assert(observers === boundObservers, `Renders that moved no group rebound the clipping ${observers - boundObservers} times`);

    // Collapsing a group is a new structure: the clipping rebinds.
    const header = [...document.querySelectorAll<HTMLButtonElement>(".group-header")]
      .find(node => node.textContent?.includes("Gamma"));
    assert(header, "Missing the Gamma header");
    flushSync(() => header.click());
    await frames();
    assert(observers > boundObservers, "Collapsing a group did not rebind the clipping");
    flushSync(() => header.click());
    await frames();

    // A confirmation opened under a pinned label moves the rows beneath it;
    // the clip follows without waiting for a scroll. Scroll anchoring is off
    // and the confirmation's Cancel is in view, so no scroll event can be what
    // moved the clip: the ResizeObserver on the list's children has to.
    const list = document.querySelector<HTMLElement>(".sidebar .session-list")!;
    list.style.overflowAnchor = "none";
    list.scrollTop = 20;
    await frames();
    const alpha = document.querySelector<HTMLElement>('[data-sidebar-project="/fixture/sidebar-rows/Alpha"]')!;
    const clipError = (): number => {
      const head = alpha.querySelector<HTMLElement>(":scope > .group-header-row")!.getBoundingClientRect();
      const rows = alpha.querySelector<HTMLElement>(":scope > .session-items")!;
      const expected = Math.max(0, head.bottom - rows.getBoundingClientRect().top);
      return Math.abs(Number.parseFloat(rows.style.getPropertyValue("--sticky-header-clip")) - expected);
    };
    assert(clipError() < 0.5, "The pinned Alpha label was not clipping its rows");
    const projectMenu = alpha.querySelector<HTMLButtonElement>(".project-menu-button")!;
    flushSync(() => projectMenu.click());
    await frames();
    const archive = [...document.querySelectorAll<HTMLButtonElement>('.sidebar-aux-menu [role="menuitem"]')]
      .find(item => item.textContent?.trim() === "Archive project…");
    assert(archive && !archive.disabled, "The project menu offered no Archive");
    flushSync(() => archive.click());
    await frames(3);
    assert(alpha.querySelector(".project-confirm"), "The archive confirmation did not open");
    assert(list.scrollTop === 20, `Opening the confirmation scrolled the list to ${list.scrollTop}`);
    const confirmClipError = clipError();
    assert(confirmClipError < 0.5, `The clip lagged the archive confirmation by ${confirmClipError}px`);
    const cancel = [...alpha.querySelectorAll<HTMLButtonElement>(".project-confirm button")]
      .find(button => button.textContent?.trim() === "Cancel")!;
    flushSync(() => cancel.click());
    await frames(3);
    assert(clipError() < 0.5, "The clip lagged the closed confirmation");
    return { rows: keys.length, pushRowRenders, keyboardPeekOtherRenders, hoverOtherRenders,
      renameOtherRenders, confirmClipError, clippingBinds: observers };
  } finally {
    flushSync(() => root.render(null));
    rowType.type = drawRow;
    window.ResizeObserver = Observer;
  }
}
