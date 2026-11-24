import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { TerminalLaunchProfileId, TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import type { TerminalMenuModel } from "../components/TerminalWorkspace";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import { attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE, serializeTerminalWorkspaceState } from "../terminal-workspace";
import { assert, noop, wait } from "./fixture-support";

const OVERVIEW_KEY = "interface.integrated-terminals.agent-overview.v1";
const TAB_KEY = "interface.integrated-terminals.agent-overview-tab.v1";
let toolbarSlot: HTMLElement | null = null;

/**
 * The agent overview against the real terminal center: two saved workspaces,
 * three agents and a plain shell. Opens from the toolbar toggle, lists only the
 * agents, and choosing one in the other workspace switches to it, focuses its
 * pane and pulses that pane once. Beside the panes on wide windows; over them
 * (closing after a choice) at 1080px and below. Its Browser tab shows a
 * Browser in the column, kept loaded behind Agents; `tab` is the one left
 * showing for the screenshot.
 */
export async function agentOverviewFixture(root: Root, theme: string,
    chrome: string, tab: "agents" | "browser" = "agents"): Promise<Record<string, unknown>> {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  try {
    window.localStorage.removeItem(OVERVIEW_KEY);
    window.localStorage.removeItem(TAB_KEY);
  } catch { /* fixture profile */ }
  const stamp = "2026-09-23T00:00:00Z";
  const terminal = (index: number, title: string, profileId: TerminalLaunchProfileId): TerminalSummary => ({
    id: `00000000-0000-4000-8000-${String(951 + index).padStart(12, "0")}`,
    generation: 1, title, root: "/fixture/overview", cwd: "/fixture/overview",
    state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId }, createdAt: stamp, updatedAt: stamp,
  });
  const terminals = [
    terminal(0, "Security review", "claude"), terminal(1, "Codex refactor", "codex"),
    terminal(2, "Docs pass", "claude"), terminal(3, "zsh", "shell"),
  ];
  const groups = [[terminals[0], terminals[1]], [terminals[2], terminals[3]]];
  const records: TerminalWorkspaceRecord[] = groups.map((members, index) => ({
    id: `overview-workspace-${index}`, name: index === 0 ? "bridge-api" : "bridge-site",
    createdAt: stamp, updatedAt: stamp, terminals: members,
    layout: serializeTerminalWorkspaceState(attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE,
      members.map(member => member.id), (() => { let pane = 0; return () => `overview-pane-${index}-${pane++}`; })(),
      () => `overview-split-${index}`)),
  }));
  let switcher: TerminalWorkspaceSwitcher | null = null;
  const errors: string[] = [];
  const ringDelays: number[] = [];
  const resizes: string[] = [];
  const unexpected = (): never => { throw new Error("The overview performed a provider operation"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals }),
    listTerminalLaunchProfiles: async () => [
      { id: "claude", label: "Claude Code", available: true },
      { id: "codex", label: "Codex", available: true },
    ],
    getTerminalWorkspaces: async () => records, setTerminalWorkspaces: async value => value,
    onTerminalData: () => noop, onTerminalLifecycle: () => noop, onTerminalDataFailure: () => noop,
    sendTerminalInput: noop,
    resizeTerminal: async request => {
      resizes.push(request.id);
      return { ...terminals.find(entry => entry.id === request.id)!, size: request.size };
    },
    createTerminal: unexpected, closeTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const until = async (ready: () => boolean, message: string, ms = 4000): Promise<void> => {
    const deadline = performance.now() + ms;
    while (!ready()) { assert(performance.now() < deadline, `${message}: ${errors.join("; ")}`); await wait(20); }
  };
  const slot = document.createElement("span");
  toolbarSlot = slot;
  // A closing overview stays mounted, inert, until its exit animation ends.
  const overview = (): HTMLElement | null => document.querySelector(".agent-overview:not(.is-leaving)");
  const gone = (): boolean => document.querySelector(".agent-overview") === null;
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const floats = window.matchMedia("(max-width: 1080px)").matches;
  // The toggle is the ··· menu's Agent overview row (owner, 2026-09-26); it
  // runs the published menu model's action, as this fixture does.
  let menu: TerminalMenuModel | null = null;
  const toggle = (): NonNullable<TerminalMenuModel["overview"]> | null => menu?.overview ?? null;
  const row = (id: string): HTMLButtonElement | null =>
    document.querySelector<HTMLButtonElement>(`.agent-overview-row[data-terminal-id="${id}"]`);
  const paneFor = (id: string): HTMLElement | null =>
    document.querySelector(`.terminal-tab[data-terminal-id="${id}"]`)?.closest<HTMLElement>(".terminal-pane") ?? null;
  const visible = (element: Element | null): boolean =>
    element !== null && element.getBoundingClientRect().height > 0;
  try {
    document.body.appendChild(slot);
    flushSync(() => root.render(<div style={{ height: "100vh", display: "flex" }}>
      <IntegratedTerminalCenter api={api} activeCwd="/fixture/overview"
        visible toolbarSlot={slot} onSwitcherChange={value => { switcher = value; }}
        onMenuChange={value => { menu = value; }}
        onError={message => errors.push(message)} />
    </div>));
    await until(() => switcher !== null && toggle() !== null, "Workspaces or the overview toggle did not load");
    flushSync(() => switcher!.onSelect(records[0].id));
    await until(() => visible(paneFor(terminals[0].id)), "The first workspace did not show its panes");
    assert(overview() === null && !toggle()!.open,
      "The overview opened before it was asked for");

    const centerElement = document.querySelector<HTMLElement>(".terminal-center")!;
    // Let the workspace switch's own fits land first, so any resize from here
    // on belongs to the overview.
    for (let quiet = resizes.length; ; quiet = resizes.length) { await wait(150); if (resizes.length === quiet) break; }
    const resizesBeforeOpen = resizes.length;
    flushSync(() => toggle()!.onToggle());
    await until(() => overview() !== null, "The toggle did not open the overview");
    assert(window.localStorage.getItem(OVERVIEW_KEY) === "open", "Opening the overview was not remembered");
    let slide: Record<string, unknown> = { reduced };
    if (!reduced) {
      // It slides in from the right edge; the panes keep their terminal grids
      // until it settles, then refit once.
      await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame);
      const moving = overview()!;
      const midRight = moving.getBoundingClientRect().right;
      assert(moving.classList.contains("is-entering"), `The overview did not animate in: "${moving.className}"`);
      assert(midRight > centerElement.getBoundingClientRect().right + 1 || floats,
        `The overview was not mid-slide two frames in (${midRight} vs ${centerElement.getBoundingClientRect().right})`);
      if (!floats) {
        assert(centerElement.hasAttribute("data-layout-sliding"), "The panes were not held while the overview slid");
        assert(resizes.length === resizesBeforeOpen, "A terminal resized mid-slide");
      }
      await until(() => !overview()!.classList.contains("is-entering"), "The overview slide never settled", 1500);
      assert(!centerElement.hasAttribute("data-layout-sliding"), "The panes stayed held after the slide");
      if (!floats) await until(() => resizes.length > resizesBeforeOpen, "The panes did not refit after the slide", 1500);
      slide = { reduced, midRight: Math.round(midRight), refits: resizes.length - resizesBeforeOpen };
    }
    const listed = [...document.querySelectorAll<HTMLElement>(".agent-overview-row")].map(item => item.dataset.terminalId);
    assert(JSON.stringify(listed) === JSON.stringify(terminals.slice(0, 3).map(item => item.id)),
      `The overview listed ${JSON.stringify(listed)}, not the three agents in saved order`);
    const heading = overview()!.querySelector(".agent-overview-head")!.textContent ?? "";
    assert(heading.includes("3") && heading.includes("live panes") && heading.includes("3 idle"),
      `The overview heading read "${heading}"`);
    const meta = row(terminals[2].id)!.querySelector(".agent-overview-row-meta")!.textContent;
    assert(meta === "Claude Code · bridge-site", `A row's second line read "${meta ?? ""}"`);

    // Beside the panes, or over them when the window is too narrow.
    const panel = overview()!.getBoundingClientRect();
    const body = document.querySelector(".terminal-center-body")!.getBoundingClientRect();
    const center = document.querySelector(".terminal-center")!.getBoundingClientRect();
    assert(Math.abs(panel.right - center.right) <= 1 && panel.height >= center.height - 1,
      `The overview did not run the full height of the right edge (${JSON.stringify({ panel, center, reduced, floats, cls: overview()!.className, anim: overview()!.getAnimations().map(a => (a as CSSAnimation).animationName) })})`);
    if (floats) {
      assert(getComputedStyle(overview()!).position === "absolute" && panel.left < body.right,
        "A narrow window did not float the overview over the panes");
    } else {
      assert(Math.round(panel.width) === 360 && panel.left >= body.right + 5,
        "A wide window did not give the overview its own 360px column beside the panes");
    }
    const list = overview()!.querySelector(".agent-overview-list")!;
    assert(list.scrollWidth <= list.clientWidth + 1, "The overview list scrolls sideways");

    // Filters keep one group; All brings every group back.
    const chip = (label: string): HTMLButtonElement => [...overview()!
      .querySelectorAll<HTMLButtonElement>(".agent-overview-filter")].find(item => item.textContent?.startsWith(label))!;
    flushSync(() => chip("Working").click());
    assert(overview()!.querySelectorAll(".agent-overview-row").length === 0
      && overview()!.querySelector(".agent-overview-empty") !== null, "The Working filter did not empty the list");
    flushSync(() => chip("All").click());
    assert(overview()!.querySelectorAll(".agent-overview-row").length === 3, "All did not restore every agent");

    // Go to an agent in the other workspace: switch, focus, one pulse.
    const target = terminals[2];
    flushSync(() => row(target.id)!.click());
    await until(() => switcher?.activeId === records[1].id && visible(paneFor(target.id)),
      "Choosing an agent did not switch to its workspace");
    await until(() => paneFor(target.id)?.classList.contains("is-locating") === true,
      "The chosen pane did not pulse");
    const pulsing = paneFor(target.id)!;
    assert(pulsing.classList.contains("is-focused"), "The chosen pane was not focused");
    assert(document.querySelectorAll(".terminal-pane.is-locating").length === 1, "More than one pane pulsed");
    const ringDrawn = (): boolean => {
      const ring = getComputedStyle(pulsing, "::after");
      return reduced ? ring.animationName === "none" && ring.borderTopWidth === "2px"
        : ring.animationName === "terminal-pane-locate";
    };
    const firstLook = ringDrawn();
    const lookedAt = performance.now();
    await until(ringDrawn, `The pulse ring never drew on "${pulsing.className}"`, 500);
    ringDelays.push(Math.round(performance.now() - lookedAt) * (firstLook ? 0 : 1));
    await until(() => !pulsing.classList.contains("is-locating"), "The pulse did not end", 2500);
    if (floats) {
      assert(overview() === null && window.localStorage.getItem(OVERVIEW_KEY) === "closed",
        "A floating overview stayed over the pane it had just found");
      await until(gone, "The floating overview never finished closing", 1500);
      flushSync(() => toggle()!.onToggle());
      await until(() => overview() !== null, "The overview did not reopen");
    } else {
      assert(overview() !== null, "A docked overview closed after a choice");
    }

    // Choosing the same agent again pulses again.
    flushSync(() => row(target.id)!.click());
    await until(() => paneFor(target.id)?.classList.contains("is-locating") === true,
      "A second choice of the same pane did not pulse again");
    if (floats) {
      flushSync(() => toggle()!.onToggle());
      await until(() => overview() !== null, "The overview did not reopen");
    }

    // Close from the panel; the toggle reflects it and the choice is kept.
    flushSync(() => overview()!.querySelector<HTMLButtonElement>(".agent-overview-close")!.click());
    assert(overview() === null && !toggle()!.open
      && window.localStorage.getItem(OVERVIEW_KEY) === "closed", "Closing the overview was not kept");
    const leaving = document.querySelector<HTMLElement>(".agent-overview.is-leaving");
    assert(reduced || (leaving !== null && leaving.inert), "The overview did not slide out inert");
    await until(gone, "The overview never finished closing", 1500);
    // Leave it open for the screenshot the lane takes next.
    flushSync(() => toggle()!.onToggle());
    await until(() => overview() !== null, "The overview did not reopen for capture");
    await until(() => !overview()!.classList.contains("is-entering"), "The reopened overview never settled", 1500);

    // The Browser tab (owner, 2026-10-14): nothing loads until it is chosen;
    // then its page is anchored over the column's own slot, and survives a
    // trip back to Agents parked rather than reloaded.
    const tabButton = (label: string): HTMLButtonElement | undefined => [...overview()!
      .querySelectorAll<HTMLButtonElement>(".agent-overview-tab")].find(item => item.textContent?.startsWith(label));
    const column = (): HTMLElement | null => overview()?.querySelector<HTMLElement>(".agent-overview-browser") ?? null;
    const columnPage = (): HTMLElement | null =>
      column()?.querySelector<HTMLElement>(":scope > .browser-page-layer > .browser-page") ?? null;
    assert(tabButton("Browser") !== undefined, "The overview offered no Browser tab");
    assert(column() === null && document.querySelectorAll(".browser-page").length === 0,
      "The overview's Browser loaded before it was chosen");
    flushSync(() => tabButton("Browser")!.click());
    await until(() => columnPage()?.querySelector(".browser-page-view") != null,
      "The Browser tab never opened its home page");
    assert(window.localStorage.getItem(TAB_KEY) === "browser", "Choosing Browser was not remembered");
    assert(overview()!.querySelector(".agent-overview-list") === null, "The agents stayed under the Browser");
    const page = columnPage()!;
    const columnSlot = column()!.querySelector<HTMLElement>(".browser-pane-slot")!;
    assert(columnSlot.style.getPropertyValue("anchor-name") === "--browser-page-column",
      "The column's slot carried no page anchor");
    await until(() => !page.classList.contains("is-parked"), "The column's page stayed parked");
    const address = column()!.querySelector<HTMLInputElement>('[aria-label="Address"]')!;
    await until(() => document.activeElement === address, "The new Browser did not take the keyboard in its address bar");
    const near = (a: DOMRect, b: DOMRect): boolean => Math.abs(a.left - b.left) <= 1 && Math.abs(a.top - b.top) <= 1
      && Math.abs(a.width - b.width) <= 1 && Math.abs(a.height - b.height) <= 1;
    const pageBox = page.getBoundingClientRect();
    const slotBox = columnSlot.getBoundingClientRect();
    const columnBox = overview()!.getBoundingClientRect();
    assert(near(pageBox, slotBox) && pageBox.width > 200 && pageBox.height > 200
      && pageBox.left >= columnBox.left && pageBox.right <= columnBox.right,
      `The column's page was not drawn over its slot (${JSON.stringify({ pageBox, slotBox, columnBox })})`);
    flushSync(() => tabButton("Panes")!.click());
    assert(overview()!.querySelector(".agent-overview-list") !== null && column()!.hidden,
      "Panes did not come back over the Browser");
    await until(() => page.classList.contains("is-parked"), "The Browser's page stayed on screen behind Agents");
    flushSync(() => tabButton("Browser")!.click());
    await until(() => !page.classList.contains("is-parked"), "The Browser's page did not come back with its tab");
    assert(columnPage() === page && page.querySelectorAll(".browser-page-view").length === 1,
      "Coming back to Browser replaced its page");
    if (tab === "agents") {
      flushSync(() => tabButton("Panes")!.click());
      await until(() => page.classList.contains("is-parked"), "The Browser's page stayed over the agents");
    } else {
      // A Browser tab in a pane beside the column's for the screenshot: drawn
      // alike, the same field and the page in the same rounded card.
      const newPane = paneFor(target.id)!.querySelector<HTMLButtonElement>('[aria-label="New terminal in this pane"]')!;
      flushSync(() => newPane.click());
      const browserItem = (): HTMLButtonElement | undefined => [...document.querySelectorAll<HTMLButtonElement>(
        '[role="menu"][aria-label="Open in a new pane"] [role="menuitem"]')]
        .find(item => item.textContent?.trim() === "Browser");
      await until(() => browserItem() !== undefined, "The pane menu offered no Browser");
      flushSync(() => browserItem()!.click());
      const paneBrowser = (): HTMLElement | null => paneFor(target.id)?.querySelector<HTMLElement>(".browser-pane") ?? null;
      await until(() => paneBrowser() !== null && document.querySelectorAll(".browser-page-view").length === 2,
        "The Browser tab never opened in the pane beside the column's");
      const field = (scope: Element): DOMRect => scope.querySelector(".browser-pane-field")!.getBoundingClientRect();
      const radius = (scope: Element): string => getComputedStyle(scope.querySelector(".browser-pane-body")!).borderRadius;
      assert(Math.round(field(paneBrowser()!).height) === 34 && Math.round(field(column()!).height) === 34
        && radius(paneBrowser()!) === radius(column()!), "The Browser pane and the column's Browser were drawn differently");
      // A hidden window can hand the capture its previous frame.
      await new Promise(requestAnimationFrame); await new Promise(requestAnimationFrame); await wait(250);
    }
    assert(errors.length === 0, errors.join("; "));
    return { theme, chrome, width: window.innerWidth, floats, slide, ringDelays, agents: listed.length, shellExcluded: true,
      switchedWorkspace: true, pulsed: true, repulsed: true, reducedMotion: reduced, browserTab: true, tab };
  } catch (error) {
    flushSync(() => root.render(null));
    slot.remove();
    throw error;
  }
}

/** Unmount after the lane's screenshot. */
export function clearAgentOverviewFixture(root: Root): void {
  flushSync(() => root.render(null));
  toolbarSlot?.remove();
  toolbarSlot = null;
  try {
    window.localStorage.removeItem(OVERVIEW_KEY);
    window.localStorage.removeItem(TAB_KEY);
  } catch { /* fixture profile */ }
}
