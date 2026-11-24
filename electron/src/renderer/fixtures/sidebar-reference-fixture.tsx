import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { themeKind, type ResolvedTheme } from "../../shared/appearance";
import { Sidebar } from "../components/Sidebar";
import type { SidebarGroup, SidebarTerminalRow } from "../sidebar-view";
import { assert, frame, noop } from "./fixture-support";

/** Reference-style navigation with real workspace rows, states and keyboard handlers. */
export async function sidebarReferenceFixture(root: Root, theme: ResolvedTheme, chrome: string, width: number) {
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  let selected = "interface";
  let working = true;
  let settingsActive = false;
  let settingsDisabled = false;
  let height = 660;
  let settingsOpened = 0;
  let alertOpened = 0;
  let needsSetup = false;
  const terminal = (id: string, project: string, title: string,
      activity: SidebarTerminalRow["activity"] = "idle"): SidebarTerminalRow => ({
    id, root: `/fixture/sidebar-reference/${project}`, cwd: `/fixture/sidebar-reference/${project}`,
    title, state: "running", activity, selected: false, lastActivity: "2026-09-09T00:00:00Z",
  });
  const render = (): void => {
    const terminals = [
      terminal("interface", "Interface", "Interface", working ? "working" : "idle"),
      terminal("review", "Interface", "Review the terminal layout and keyboard navigation"),
      terminal("scanner", "Market scanner", "Investigate the next market opportunity"),
      terminal("website", "Portfolio", "Refine the case study"),
    ];
    const groups: SidebarGroup[] = ["Interface", "Market scanner", "Portfolio"].map(projectName => ({
      projectName, projectPath: `/fixture/sidebar-reference/${projectName}`,
      rows: [], counts: { total: 0, waiting: 0, working: 0 },
      terminals: terminals.filter(item => item.root.endsWith(`/${projectName}`)),
    }));
    flushSync(() => root.render(<div style={{ width, height, display: "flex", padding: 8, boxSizing: "border-box" }}>
      <Sidebar groups={groups} terminalProjects={Object.fromEntries(terminals.map(item => [item.id, item.root]))}
        codeWorkspaces terminalsActive sectionLabel="Workspaces" newLabel="New workspace"
        rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
        onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop}
        onQuery={noop} query=""
        settingsActive={settingsActive} settingsDisabled={settingsDisabled}
        onSettings={() => { settingsOpened++; settingsActive = true; render(); }}
        footerAlert={needsSetup ? { label: "2 providers need setup", shortLabel: "2 need setup",
          description: "Codex: sign in", accessibleName: "2 providers need setup: open Models & providers" } : undefined}
        onFooterAlert={() => { alertOpened++; }}
        terminalWorkspaces={{
          records: terminals.map(item => ({ id: item.id, name: item.title, cwd: item.cwd,
            firstTerminalTitle: item.title, openTerminalCount: item.id === "interface" ? 3 : 1,
            terminalIds: [item.id] })),
          activeId: selected, onSelect: id => { selected = id; render(); },
          onCreate: noop, onRename: noop, onDelete: noop, canCreate: true, busy: false,
        }} />
    </div>));
  };
  const settle = async (): Promise<void> => {
    await frame(); await frame();
    for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
    await frame();
  };
  render(); await settle();
  const row = (id: string) => document.querySelector<HTMLButtonElement>(`[data-terminal-workspace-id="${id}"]`)!;
  const selectedRow = row("interface");
  const header = document.querySelector<HTMLElement>(".group-header.is-current")!;
  const ticker = selectedRow.querySelector<HTMLElement>(".workspace-tree-ticker")!;
  assert(header?.textContent?.includes("Interface"), "CLI selection did not highlight its owning project");
  assert(ticker?.children.length === 6, "Working workspace lost its six-dot ticker");
  const tickerStyle = getComputedStyle(ticker);
  assert(tickerStyle.gridTemplateColumns === "3px 3px" && tickerStyle.gridTemplateRows === "3px 3px 3px",
    "Workspace ticker must have two columns and three rows");
  const tickerBounds = ticker.getBoundingClientRect();
  assert(tickerBounds.width === 8 && tickerBounds.height === 13, "Workspace ticker lost its compact 2x3 geometry");
  assert(document.querySelectorAll(".workspace-tree-ticker").length === 1, "Idle workspace gained a ticker");
  assert(getComputedStyle(header).backgroundColor !== getComputedStyle(selectedRow).backgroundColor,
    "Selected workspace should stand out from its parent");
  const headerCount = header.querySelector(".group-count")!.getBoundingClientRect();
  // A workspace row's pips stand where its count did (owner, 2026-10-01).
  const rowCount = selectedRow.querySelector(".workspace-tree-pips, .workspace-tree-count")!.getBoundingClientRect();
  assert(Math.abs(headerCount.right - rowCount.right) < 1, "Project count and workspace pips are misaligned");
  // Roomier rows are 32px (owner, 2026-10-02): above WCAG 2.5.8's 24px floor.
  assert(selectedRow.getBoundingClientRect().height >= 24, "Workspace row is too cramped");
  for (const item of document.querySelectorAll<HTMLElement>(".workspace-tree-row, .group-header")) {
    assert(item.scrollWidth <= item.clientWidth, "Sidebar row overflows");
  }
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const dots = [...ticker.children];
  if (reduced) assert(dots.every(dot => getComputedStyle(dot).animationName === "none"
    && getComputedStyle(dot).opacity === "1"), "Reduced motion did not stop the ticker");
  else {
    const before = getComputedStyle(dots[0]).opacity;
    const title = selectedRow.querySelector(".workspace-tree-title")!.getBoundingClientRect();
    await new Promise(resolve => setTimeout(resolve, 180));
    assert(getComputedStyle(dots[0]).opacity !== before, "Working ticker did not animate");
    assert(selectedRow.querySelector(".workspace-tree-title")!.getBoundingClientRect().x === title.x,
      "Ticker animation moved the workspace title");
  }
  selectedRow.focus();
  selectedRow.dispatchEvent(new KeyboardEvent("keydown", { key: "ContextMenu", bubbles: true }));
  await settle();
  const menuDeadline = performance.now() + 1500;
  while (!document.querySelector('.sidebar-aux-menu')?.contains(document.activeElement)
      && performance.now() < menuDeadline) {
    await new Promise(resolve => setTimeout(resolve, 20));
  }
  const menu = document.querySelector<HTMLElement>('.sidebar-aux-menu');
  assert(menu && menu.contains(document.activeElement), `Keyboard workspace menu did not receive focus: ${JSON.stringify({
    menu: menu?.outerHTML, focus: document.activeElement?.outerHTML.slice(0, 300),
    visibility: menu && getComputedStyle(menu).visibility,
  })}`);
  menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await settle();
  assert(document.activeElement === row("interface"), "Closing the keyboard menu lost workspace focus");
  row("scanner").click(); await settle();
  assert(document.querySelector(".group-header.is-current")?.textContent?.includes("Market scanner"),
    "Workspace selection did not update the project highlight");
  working = false; render(); await settle();
  assert(!document.querySelector(".workspace-tree-ticker"), "Settled workspace kept ticking");
  const settings = (): HTMLButtonElement => document.querySelector<HTMLButtonElement>('.sidebar-settings-button')!;
  const footerBounds = (): DOMRect => document.querySelector('.sidebar-footer')!.getBoundingClientRect();
  const footer = footerBounds();
  const aside = document.querySelector('.sidebar')!.getBoundingClientRect();
  const list = document.querySelector('.session-list')!.getBoundingClientRect();
  assert(Math.abs(footer.bottom - aside.bottom) <= 1 && footer.top >= list.bottom,
    "Settings footer is not below the workspace list at the bottom of the sidebar");
  assert(settings().getBoundingClientRect().height >= 32 && settings().scrollWidth <= settings().clientWidth,
    "Settings row is cramped or clipped");
  settings().click(); await settle();
  assert(settingsOpened === 1 && settings().getAttribute('aria-current') === 'page'
    && document.activeElement === settings(), "Settings did not open from its stable sidebar focus target");
  settingsActive = false; settingsDisabled = true; render(); await settle();
  settings().click();
  assert(settingsOpened === 1, "Disabled Settings opened during a launch");
  settingsDisabled = false; height = 260; render(); await settle();
  const shortFooter = footerBounds();
  const scroller = document.querySelector<HTMLElement>('.session-list')!;
  assert(scroller.scrollHeight > scroller.clientHeight, "Short sidebar did not exercise scrolling");
  scroller.scrollTop = scroller.scrollHeight; await settle();
  assert(footerBounds().top === shortFooter.top
    && shortFooter.bottom <= document.querySelector('.sidebar')!.getBoundingClientRect().bottom + 1,
    "Settings moved or clipped when workspaces scrolled");
  // Providers that need setup share the Settings row (owner, 2026-11-15):
  // the amber dot and words sit right of a whole "Settings", in one line, and
  // open Models & providers rather than plain Settings.
  height = 660; needsSetup = true; render(); await settle();
  const alert = document.querySelector<HTMLButtonElement>(".sidebar-footer-alert")!;
  const dot = document.querySelector(".sidebar-footer-alert-dot")!.getBoundingClientRect();
  const label = document.querySelector<HTMLElement>(".sidebar-settings-label")!;
  const words = Array.from(alert.querySelectorAll<HTMLElement>(".sidebar-footer-alert-label"))
    .find(node => node.getClientRects().length > 0);
  const settingsBox = settings().getBoundingClientRect();
  const alertBox = alert.getBoundingClientRect();
  assert(alert.parentElement === settings().parentElement
    && Math.abs((alertBox.top + alertBox.bottom) / 2 - (settingsBox.top + settingsBox.bottom) / 2) <= 1
    && alertBox.left >= label.getBoundingClientRect().right + 4
    && alertBox.right <= footerBounds().right && dot.width === 6 && dot.height === 6
    && label.scrollWidth <= label.clientWidth && words !== undefined && words.scrollWidth <= words.clientWidth,
  `Setup notice is not one tidy line beside Settings: ${JSON.stringify({
    settings: settingsBox, alert: alertBox, dot, words: words?.textContent,
    clipped: words && words.scrollWidth > words.clientWidth })}`);
  assert(getComputedStyle(alert).color !== getComputedStyle(label).color, "Setup notice is not in its own (amber) colour");
  const opened = settingsOpened;
  alert.click(); await settle();
  assert(alertOpened === 1 && settingsOpened === opened, "The setup notice did not open Models & providers");
  needsSetup = false; settingsActive = false;
  height = 660; selected = "interface"; working = true; render(); await settle();
  document.querySelector('.session-list')!.scrollTop = 0;
  return { theme, chrome, width, reducedMotion: reduced, ticker: "working only", keyboard: true, selection: true,
    setupNotice: words?.textContent };
}
