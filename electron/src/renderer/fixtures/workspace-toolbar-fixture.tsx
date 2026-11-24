import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { useRef, useState } from "react";
import { isResolvedTheme, themeKind } from "../../shared/appearance";
import { APP_SELECT_DISMISS_EVENT } from "../components/AppSelect";
import { SurfaceSwitch } from "../components/SurfaceSwitch";
import { WorkspaceMoreMenu } from "../components/WorkspaceMoreMenu";
import { UsageMenu } from "../components/UsageMenu";
import { UpdateButton } from "../components/UpdateButton";
import type { AttentionRow } from "../attention-tray";
import { AttentionTray } from "../components/AttentionTray";
import { assert, frame, noop } from "./fixture-support";

export async function workspaceToolbarFixture(root: Root, theme: string, chrome: string): Promise<unknown> {
  if (!isResolvedTheme(theme)) throw new Error(`Unknown theme ${theme}`);
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  document.documentElement.style.colorScheme = themeKind(theme);
  const chosen: string[] = [];
  const layoutChosen: string[] = [];
  let setCount = (_count: number): void => {};
  const rows: AttentionRow[] = Array.from({ length: 120 }, (_, index) => ({
    // Live needs: the count on the ··· button counts exactly these (owner, 2026-09-12).
    id: `notice-${index}`, kind: "needs-input", live: true, at: 0,
    title: `Finished task ${index + 1}`, where: "Workspace",
    target: { kind: "session", key: `codex#notice-${index}` },
  }));
  function Fixture() {
    const [surface, setSurface] = useState<"agent" | "code" | "chat">("code");
    const [usage, setUsage] = useState(false);
    const [attention, setAttention] = useState(false);
    const [overview, setOverview] = useState(false);
    const [count, updateCount] = useState(11);
    setCount = updateCount;
    const more = useRef<HTMLButtonElement>(null);
    const shown = rows.slice(0, count);
    // The compact bar (owner, 2026-09-26): Output preview, the update pill,
    // Voice, Away and ···. Attention, Usage and the layout controls live in ···.
    return <div style={{ minHeight: "100vh", background: "var(--workbench-canvas)" }}>
      <header className="workspace-toolbar" aria-label="Workspace controls">
        <div className="workspace-toolbar-leading">
          <span className="workspace-toolbar-traffic" aria-hidden="true" />
          <button className="workspace-icon-btn workspace-sidebar-toggle" aria-label="Hide sidebar"><span className="codicon codicon-layout-sidebar-left" /></button>
          <span className="workspace-brand"><span className="workspace-brand-name">Interface<span className="product-dev-badge">Dev</span></span></span>
        </div>
        <SurfaceSwitch surface={surface} onChange={setSurface} />
        <div className="workspace-toolbar-actions">
          <span className="workspace-toolbar-terminal"><button type="button" className="terminal-center-action"
            aria-label="Output preview" aria-pressed={false}><span className="codicon codicon-open-preview" /></button></span>
          <UpdateButton state={{ phase: "ready", progress: null }} label="Restart to update"
            title="Restart to update — Interface 0.3.148" onClick={() => {}} />
          <button className="workspace-icon-btn" aria-label="Voice assistant"><span className="codicon codicon-mic" /></button>
          <AttentionTray anchorRef={more} rows={shown} unread={count} open={attention}
            onOpenChange={setAttention} onActivate={noop} onClearAll={noop} now={0} />
          <span className="workspace-more-anchor">
            <WorkspaceMoreMenu buttonRef={more} inert={false}
              attention={{ rows: shown, unread: count, onOpen: () => { setAttention(true); } }}
              onUsage={() => { layoutChosen.push("usage"); setUsage(true); }}
              layout={{
                cwd: "/Users/me/Desktop/interface", view: "split", equalGrid: false, disabled: false,
                canTidy: true, onFlip: () => layoutChosen.push("flip"), onTidy: () => layoutChosen.push("tidy"),
                overview: { open: overview, onToggle: () => { setOverview(open => !open); } },
              }}
              onOpenTasks={() => layoutChosen.push("tasks")}
              onOpenSavedSessions={() => layoutChosen.push("sessions")}
              onCommands={() => chosen.push("commands")} />
            <UsageMenu anchorRef={more} state={{ status: "ready", views: [], error: null, readAt: 0 }} now={0}
              open={usage} onOpenChange={setUsage} engines={[]} onRefresh={noop} />
          </span>
          <button className="workspace-icon-btn" aria-label="Close settings"><span className="codicon codicon-close" /></button>
        </div>
      </header>
    </div>;
  }
  flushSync(() => root.render(<Fixture />));
  await document.fonts.ready;
  await frame(); await frame();
  // A real press: pointerdown first, so outside-press dismissal runs as it does for a person.
  const press = async (button: HTMLButtonElement): Promise<void> => {
    button.focus();
    button.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    flushSync(() => button.click()); await frame(); await frame();
  };
  const click = async (label: string): Promise<HTMLButtonElement> => {
    const button = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
    if (!button) throw new Error(`Missing ${label}`);
    await press(button);
    return button;
  };
  const checkGeometry = (): number => {
    const header = document.querySelector<HTMLElement>('.workspace-toolbar')!;
    const nav = document.querySelector<HTMLElement>('.surface-switch')!;
    const bounds = header.getBoundingClientRect();
    const switchBounds = nav.getBoundingClientRect();
    const delta = Math.abs(switchBounds.left + switchBounds.width / 2 - bounds.left - bounds.width / 2);
    assert(delta <= 1, `Header switch shifted ${delta}px`);
    const buttons = [...header.querySelectorAll<HTMLButtonElement>('button')].filter(button => button.getClientRects().length > 0);
    for (const button of buttons) {
      const b = button.getBoundingClientRect();
      assert(b.width >= 24 && b.height >= 24, `Small target: ${button.ariaLabel}`);
      assert(b.top >= bounds.top && b.bottom <= bounds.bottom + 1, `Clipped vertically: ${button.ariaLabel}`);
      assert(b.left >= bounds.left && b.right <= bounds.right + 1, `Clipped control: ${button.ariaLabel}`);
      assert(getComputedStyle(button).getPropertyValue('-webkit-app-region') !== 'drag', `Dragging blocks ${button.ariaLabel}`);
      for (const other of buttons) {
        if (button === other) continue;
        const o = other.getBoundingClientRect();
        assert(Math.min(b.right, o.right) - Math.max(b.left, o.left) <= 1
          || Math.min(b.bottom, o.bottom) - Math.max(b.top, o.top) <= 1,
          `${theme}/${chrome}/${innerWidth}: ${button.ariaLabel} overlaps ${other.ariaLabel}`);
      }
    }
    // The bell is gone from the bar; its count rides the ··· button.
    assert(!header.querySelector('.attention-tray-button'), 'The bell came back to the top bar');
    assert(!header.querySelector('.usage-menu-trigger'), 'The Usage button came back to the top bar');
    const badge = header.querySelector<HTMLElement>('.workspace-more-button .attention-tray-count');
    if (badge) {
      const count = badge.getBoundingClientRect();
      const dots = header.querySelector<HTMLElement>('.workspace-more-button .codicon-ellipsis')!.getBoundingClientRect();
      const button = badge.parentElement!.getBoundingClientRect();
      assert(count.left >= dots.right + 2, 'Attention count covers the ··· glyph');
      assert(count.right <= button.right && count.top >= button.top && count.bottom <= button.bottom,
        'Attention count leaves its button');
    }
    assert(switchBounds.height <= 30, 'Mode switch overwhelms the header');
    assert(header.scrollWidth <= header.clientWidth + 1, 'Toolbar overflow');
    return delta;
  };
  const centerDelta = checkGeometry();
  const waitForFocus = async (target: () => Element | null | undefined, message: string): Promise<void> => {
    const deadline = performance.now() + 2000;
    while (document.activeElement === null || document.activeElement !== target()) {
      if (performance.now() > deadline) {
        throw new Error(`${message}; focus is on ${document.activeElement?.outerHTML.slice(0, 160) ?? "nothing"}`);
      }
      await frame();
    }
  };
  const menu = (): HTMLElement | null => document.querySelector<HTMLElement>('[aria-label="Workspace menu"]');
  const row = (label: string): HTMLButtonElement => {
    const found = [...menu()?.querySelectorAll<HTMLButtonElement>('button') ?? []]
      .find(button => button.textContent?.trim() === label);
    if (!found) throw new Error(`Missing ··· row ${label}`);
    return found;
  };
  // ··· opens under itself, hanging leftward from its right edge, focus on the first row.
  const trigger = await click('More workspace options'); checkGeometry();
  await waitForFocus(() => menu()?.querySelector('[role^="menuitem"]'), 'Menu did not focus its first row');
  assert([...menu()!.querySelectorAll('button')].map(b => b.textContent?.trim()).join('/')
    === 'Attention11/Usage/Live panes/Tidy panes/Flip terminals/Tasks/Codex accounts…/Saved sessions…/Command palette⌘⇧P',
    `··· rows changed: ${[...menu()!.querySelectorAll('button')].map(b => b.textContent?.trim()).join('/')}`);
  const menuBox = menu()!.getBoundingClientRect();
  const triggerBox = trigger.getBoundingClientRect();
  assert(menuBox.top >= triggerBox.bottom && menuBox.left >= 0 && menuBox.right <= innerWidth,
    'Menu is not below ··· inside the window');
  assert(Math.abs(menuBox.right - triggerBox.right) <= 1 || menuBox.left <= 9, 'Menu does not hang from ···');
  document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
  assert(document.activeElement === row('Usage'), 'Arrow keys skip rows');
  flushSync(() => row('Codex accounts…').click()); await frame(); await frame();
  assert(menu() !== null && layoutChosen.length === 0, 'Codex accounts… must do nothing yet');
  flushSync(() => row('Tidy panes').click()); await frame(); await frame();
  assert(menu() === null && layoutChosen.join() === 'tidy' && document.activeElement === trigger,
    'A menu choice must close the menu onto ···');
  await click('More workspace options');
  flushSync(() => row('Live panes').click()); await frame(); await frame();
  await click('More workspace options');
  assert(row('Live panes').getAttribute('aria-checked') === 'true', 'Live panes did not toggle');
  // Usage opens its panel under ···; pressing ··· again dismisses it.
  flushSync(() => row('Usage').click()); await frame(); await frame();
  const usagePanel = document.querySelector<HTMLElement>('[aria-label="Provider usage details"]');
  assert(usagePanel, 'Usage did not open the usage panel');
  const usageBox = usagePanel!.getBoundingClientRect();
  assert(usageBox.top >= triggerBox.bottom && Math.abs(usageBox.right - triggerBox.right) <= 2,
    `Usage panel is not under ··· (${usageBox.right} vs ${triggerBox.right})`);
  await press(trigger);
  assert(!document.querySelector('[aria-label="Provider usage details"]'), 'Pressing ··· left Usage open');
  document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await frame(); await frame();
  // Attention opens the tray under ···; Escape hands focus back to ···.
  await click('More workspace options');
  flushSync(() => row('Attention11').click()); await frame(); await frame();
  const tray = (): HTMLElement | null => document.querySelector<HTMLElement>('[role="menu"][aria-label="Attention"]');
  assert(tray(), 'Attention unreachable');
  await waitForFocus(() => tray()?.querySelector('[role="menuitem"]'), 'Attention tray did not focus its first row');
  const trayBox = tray()!.getBoundingClientRect();
  assert(trayBox.top >= triggerBox.bottom && trayBox.right <= innerWidth, 'Attention tray is not under ···');
  document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await frame(); await frame();
  await waitForFocus(() => trigger, 'Escape did not hand focus back to ···');
  assert(!tray(), 'Escape did not close the attention tray');
  assert([...document.querySelectorAll('.surface-switch button')].map(b => b.textContent).join('/') === 'Agent/Code/Threads', 'Wrong workspace order');
  for (const label of ['Agent', 'Threads', 'Code']) {
    const button = await click(label);
    assert(button.ariaPressed === 'true', `${label} did not select`);
  }
  await click('More workspace options');
  flushSync(() => row('Command palette⌘⇧P').click()); await frame(); await frame();
  assert(chosen.join() === 'commands' && document.activeElement === trigger, 'Command handoff lost the stable opener');
  assert(!document.querySelector('button[aria-label="Settings"]'), 'Settings remained in the top bar');
  await click('More workspace options');
  document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  await frame(); await frame();
  assert(!menu() && document.activeElement === trigger, 'Escape failed to restore focus');
  await click('More workspace options');
  window.dispatchEvent(new Event(APP_SELECT_DISMISS_EVENT)); await frame(); await frame();
  assert(!menu(), 'Navigation left the overflow menu open');
  // A pending initial-focus retry must not steal a user's newer focus choice.
  trigger.focus(); flushSync(() => trigger.click());
  const movedFocus = document.querySelector<HTMLButtonElement>('[aria-label="Voice assistant"]')!;
  movedFocus.focus(); await frame(); await frame(); await frame();
  assert(document.activeElement === movedFocus, 'Menu initialization stole newer focus');
  window.dispatchEvent(new Event(APP_SELECT_DISMISS_EVENT)); await frame(); await frame();
  for (const count of [0, 1, 120]) {
    flushSync(() => setCount(count)); await frame(); checkGeometry();
    const badge = document.querySelector('.workspace-more-button .attention-tray-count');
    assert(count === 0 ? badge === null : badge?.textContent === (count > 99 ? '99+' : String(count)),
      'Attention count is not bounded for the toolbar');
    const described = document.getElementById(trigger.getAttribute('aria-describedby') ?? '')?.textContent;
    assert(described === (count === 0 ? 'Attention: nothing needs you' : `Attention: ${count} items, ${count} new`).replace('1 items', '1 item'),
      'Accessible count lost the full total');
    assert(trigger.getAttribute('aria-label') === 'More workspace options', '··· lost its stable name');
  }
  flushSync(() => setCount(11)); await frame();
  (document.activeElement as HTMLElement)?.blur();
  return { theme, chrome, width: innerWidth, centerDelta, choices: chosen };
}
