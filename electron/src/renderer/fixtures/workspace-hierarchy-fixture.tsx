import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import { Sidebar } from '../components/Sidebar';
import { SurfaceSwitch } from '../components/SurfaceSwitch';
import { TerminalSplitWorkspace } from '../components/TerminalWorkspace';
import { TerminalView, type TerminalViewHandle } from '../components/TerminalView';
import type { SidebarGroup, SidebarTerminalRow } from '../viewmodels';
import { holdTerminalsWhileSliding } from '../terminal-pane-motion';
import { sendTerminalViewCommand } from '../terminal-view-attachment';
import { EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, splitTerminalPane,
  attachTerminalsBalanced, closeTerminalTab, focusTerminalPane, focusTerminalTab, toggleTerminalPaneMaximized } from '../terminal-workspace';
import { assert, frame, noop, until } from './fixture-support';

/** Complete shell with populated emulators, real menus, and native input. */
export async function workspaceHierarchyFixture(root: Root, theme: string, chrome: string, count = 3, sidebarSlideOnly = false) {
  flushSync(() => root.render(null));
  document.documentElement.dataset.theme = theme;
  document.documentElement.dataset.chrome = chrome;
  const handles = new Map<string, TerminalViewHandle>();
  const inputs: string[] = [];
  let sidebar = true;
  let sidebarRestoring = false;
  let sidebarOverlay = false;
  const names = ['Session recovery', 'Keyboard navigation', 'Snapshot repair', 'Provider settings', 'Terminal resizing', 'Workspace naming', 'Theme consistency', 'Release review'].slice(0, count);
  let pinned = ['hierarchy-0'];
  let pinnedProjects: string[] = [];
  const terminals = names.map((title, index) => ({ id: `hierarchy-${index}`, title,
    cwd: '/fixture/Interface', status: 'running' as const, activity: 'idle' as const,
    fastMode: { version: 1 as const, enabled: false, model: 'gpt-6-astra', ready: index !== 2 } }));
  let state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, 'pane-0', terminals[0].id);
  state = splitTerminalPane(state, { paneId: 'pane-0', newPaneId: 'pane-1', splitId: 'split-1',
    terminalId: terminals[1].id, orientation: 'horizontal', ratio: innerWidth > 1100 ? 1 / 3 : 0.5 });
  state = splitTerminalPane(state, { paneId: 'pane-1', newPaneId: 'pane-2', splitId: 'split-2',
    terminalId: terminals[2].id, orientation: innerWidth > 1100 ? 'horizontal' : 'vertical', ratio: 0.5 });
  if (count !== 3) state = attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE, terminals.map(t => t.id), i => `pane-${i}`, i => `balanced-${i}`);
  state = focusTerminalPane(state, 'pane-0');
  const rows: SidebarTerminalRow[] = terminals.map(terminal => ({ ...terminal, root: terminal.cwd,
    state: 'running', selected: false, lastActivity: '2026-09-09T00:00:00Z' }));
  let groups: SidebarGroup[] = [{ projectName: 'Interface', projectPath: '/fixture/Interface',
    rows: [], counts: { total: 0, working: 0, waiting: 0 }, terminals: rows }];
  const render = (): void => flushSync(() => root.render(<div className={`app-grid${sidebar ? sidebarOverlay ? ' sidebar-overlay' : '' : ' no-sidebar'}`}>
    <header className="workspace-toolbar">
      <div className="workspace-toolbar-leading"><span className="workspace-toolbar-traffic" />
        <button className="workspace-icon-btn workspace-sidebar-toggle" aria-label="Toggle sidebar"
          onClick={() => { sidebar = !sidebar; render(); holdTerminalsWhileSliding(document.querySelector('.app-grid')!); }}><span className={sidebar ? "codicon codicon-layout-sidebar-left" : "codicon codicon-layout-sidebar-left-off"} /></button>
        <span className="workspace-brand"><span className="workspace-brand-name">Interface</span></span>
      </div>
      <SurfaceSwitch surface="code" onChange={noop} />
      <div className="workspace-toolbar-actions">{['file', 'mic', 'coffee', 'ellipsis'].map(icon =>
        <button key={icon} className="workspace-icon-btn" aria-label={icon}><span className={`codicon codicon-${icon}`} /></button>)}</div>
    </header>
    <Sidebar focusOnMobileOpen={sidebarOverlay} groups={sidebarRestoring ? groups.map(group => ({ ...group, terminals: [] })) : groups}
      terminalProjects={Object.fromEntries(rows.map(row => [row.id, row.root]))}
      pinnedProjects={pinnedProjects}
      onPinProject={(path, pin) => { pinnedProjects = pin ? [...pinnedProjects, path] : pinnedProjects.filter(value => value !== path); render(); }}
      pinnedWorkspaces={pinned} onPinWorkspace={(id, pin) => { pinned = pin ? [...pinned, id] : pinned.filter(v => v !== id); render(); }}
      codeWorkspaces terminalsActive sectionLabel="Workspaces" newLabel="New workspace"
      rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
      onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop}
      onQuery={noop} query="" onSettings={noop}
      terminalWorkspaces={{ records: rows.map(row => ({ id: row.id, name: row.title, cwd: row.cwd,
        firstTerminalTitle: row.title, openTerminalCount: 3, terminalIds: [row.id] })),
        activeId: rows[0].id, onSelect: noop, onCreate: noop, onRename: noop, onDelete: noop,
        canCreate: true, busy: false }} />
    <main className="workspace-shell"><div className="workspace-content"><div className="workspace-surface">
      <div className="terminal-center"><TerminalSplitWorkspace state={state} terminals={terminals}
        onFocusPane={id => { state = focusTerminalPane(state, id); render(); }}
        onSelectTab={noop} onCloseTab={noop} onMoveTerminal={noop} onRequestSplit={noop}
        onToggleFastMode={noop} onSplitRatioChange={noop}
        onToggleMaximize={id => { state = toggleTerminalPaneMaximized(state, id); render(); }}
        renderTerminal={({ terminalId, active }) =>
          <TerminalView terminalId={terminalId} active={active} onData={data => inputs.push(data)}
            onReady={handle => {
              if (handle) { handles.set(terminalId, handle); handle.completeSetup(); }
              else handles.delete(terminalId);
            }} />
        } />
      </div></div></div></main>
    <footer className="statusbar"><span>Layout preview · fixture transcripts</span></footer>
  </div>));
  render(); await document.fonts.ready; await frame(); await frame();
  const workspaceNames = () => [...document.querySelectorAll('.workspace-tree-title')].map(node => node.textContent).join('|');
  const savedNames = workspaceNames();
  sidebarRestoring = true; render(); await frame();
  assert(workspaceNames() === savedNames, 'Closed or restoring panes lost their saved workspace names');
  sidebarRestoring = false;
  terminals[0].fastMode.enabled = true; render(); await frame();
  assert(document.querySelector('.terminal-tab-title')?.textContent?.trim() === names[0], 'Fast metadata replaced the pane task name');
  assert(workspaceNames() === savedNames, 'Fast metadata changed the workspace name');
  terminals[0].fastMode.enabled = false; render(); await frame();
  for (const [index, terminal] of terminals.entries()) {
    const handle = handles.get(terminal.id)!;
    assert(handle, 'Terminal did not mount');
    const rows = handle.size()!.rows;
    await new Promise<void>(resolve => handle.write([
      `› ${names[index]}`, '', '  Fixture transcript', '',
      '• Reviewing the current behavior and', '  the relevant implementation.', '',
      '  Files', '  src/session.rs', '  src/workspace.rs', '',
      '• The workspace keeps the same terminal', '  while its pane is resized.', '',
      '  Next action', '  Check keyboard focus and selection.',
      `\x1b[${Math.max(20, rows - 2)};1H› Ask Codex to do anything`,
      '  gpt-6-astra xhigh · /fixture/Interface',
    ].join('\r\n'), resolve));
  }
  await frame(); await frame();
  const shell = document.querySelector<HTMLElement>('.workspace-shell')!;
  const workspace = document.querySelector<HTMLElement>('.terminal-workspace')!;
  assert(getComputedStyle(shell).borderTopWidth === '0px', 'Terminal workspace retained its outer border');
  assert(Math.abs(workspace.getBoundingClientRect().top - document.querySelector('.terminal-pane')!.getBoundingClientRect().top) < 1,
    'Terminal panes retain an unnecessary perimeter inset');
  if (innerWidth >= 1440) {
    const sidebarWidth = document.querySelector('.sidebar')!.getBoundingClientRect().width;
    assert(Math.abs(sidebarWidth - (chrome === 'cli' ? 256 : 264)) < 1, 'Sidebar did not preserve the roomier Interface width');
  }
  for (const head of document.querySelectorAll<HTMLElement>('.terminal-pane-head')) {
    const title = head.querySelector<HTMLElement>('.terminal-tab-title')!.getBoundingClientRect();
    const actions = head.querySelector<HTMLElement>('.terminal-pane-actions')!.getBoundingClientRect();
    assert(title.width >= 50 && title.right <= actions.left, `Pane name squeezed at ${theme}/${chrome}/${innerWidth}: title=${title.width}, pane=${head.clientWidth}, actions=${actions.width}`);
    assert(head.scrollWidth <= head.clientWidth, 'Pane header overflows');
    // The head is the tabs and the actions (owner, 2026-11-15): no folder,
    // no state in words, no Find and no Files.
    assert(head.querySelector('.terminal-pane-meta, .terminal-pane-folder, .terminal-pane-status, '
      + '[aria-label="Find in terminal"], [aria-label="Files in this folder"]') === null,
      'The folder, state words, Find or Files returned to the pane head');
    assert(head.querySelector('.terminal-pane-expand-label') === null, 'Expand label returned to the header');
    for (const button of head.querySelectorAll<HTMLElement>('button')) {
      if (button.getClientRects().length === 0) continue;
      const bounds = button.getBoundingClientRect();
      assert(bounds.top >= head.getBoundingClientRect().top && bounds.bottom <= head.getBoundingClientRect().bottom,
        'Pane action is clipped');
    }
  }
  // Nothing floats over the text: Find opens a row the screen gives up.
  assert(!document.querySelector('.terminal-view-toolbar'), 'A tool strip floats over terminal text');
  const findPane = document.querySelector<HTMLElement>('.terminal-pane')!;
  sendTerminalViewCommand(findPane, 'find');
  await frame(); await frame();
  const findRow = findPane.querySelector<HTMLElement>('.terminal-search');
  assert(findRow !== null, 'Find did not open its row');
  assert(findRow.getBoundingClientRect().bottom <= findPane.querySelector('.terminal-xterm')!.getBoundingClientRect().top + 0.5,
    'The find row covers terminal text');
  findPane.querySelector<HTMLButtonElement>('[aria-label="Close terminal search"]')!.click();
  await frame(); await frame();
  assert(findPane.querySelector('.terminal-search') === null, 'The find row did not close');
  const disabled = document.querySelector<HTMLElement>('.terminal-fast-mode:disabled')!;
  const tone = document.createElement('span');
  tone.style.color = 'var(--fg-faint)'; disabled.parentElement!.append(tone);
  assert(getComputedStyle(disabled).color === getComputedStyle(tone).color,
    'Disabled pane controls lost their distinct tone');
  tone.remove();
  const readingSize = getComputedStyle(workspace).getPropertyValue('--terminal-reading-size').trim();
  assert(readingSize === '11.5', 'Terminal readability size did not apply');
  if (count === 8) {
    const boxes = [...document.querySelectorAll('.terminal-pane')].map(p => p.getBoundingClientRect());
    const columns = new Set(boxes.map(box => Math.round(box.left))).size;
    assert(columns === (innerWidth > 1100 ? 4 : innerWidth > 640 ? 2 : 1), 'Eight-pane grid has incorrect columns');
  }
  // Mobile uses the real sidebar overlay while its controls are exercised.
  sidebarOverlay = innerWidth <= 840; render(); await frame(); await frame();
  // Pins move the same workspace between sections through the real context menu.
  const workspaceRow = () => document.querySelector<HTMLElement>('[data-terminal-workspace-id="hierarchy-0"]')!;
  const pinAction = async (label: string) => {
    workspaceRow().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 140, clientY: 200 }));
    await frame(); await frame();
    const action = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find(button => button.textContent?.trim() === label);
    assert(action, `Missing ${label} action`); action.click(); await frame(); await frame();
  };
  await pinAction('Unpin workspace');
  assert(!workspaceRow().closest('.workspace-pinned-section'), 'Unpinned workspace remained in Pinned');
  await pinAction('Pin workspace');
  assert(workspaceRow().closest('.workspace-pinned-section'), 'Pinned workspace did not move into Pinned');
  assert(document.querySelectorAll('[data-terminal-workspace-id="hierarchy-0"]').length === 1, 'Pinning duplicated a workspace');
  // Project pins preserve expandable groups and independent workspace pins.
  const projectPath = '/fixture/Interface';
  const projectGroup = (path: string) => {
    const group = [...document.querySelectorAll<HTMLElement>('[data-sidebar-project]')]
      .find(node => node.dataset.sidebarProject === path);
    assert(group, `Project group is missing: ${path}`);
    return group;
  };
  const projectAction = async (path: string, label: string) => {
    projectGroup(path).querySelector<HTMLButtonElement>('.project-menu-button')!.click();
    await frame(); await frame();
    const action = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
      .find(button => button.textContent?.trim() === label);
    assert(action, `Missing ${label}`); action.click(); await frame(); await frame();
    assert(document.activeElement === projectGroup(path).querySelector('.group-header'), `${label} lost project focus at ${theme}/${chrome}/${innerWidth}`);
  };
  groups = [...groups, { ...groups[0], projectPath: '/other/Interface', terminals: [] }];
  render(); await frame();
  await projectAction(projectPath, 'Pin project');
  assert(projectGroup(projectPath).closest('.workspace-pinned-section'), 'Pinned project did not move above Folders');
  assert(!projectGroup('/other/Interface').closest('.workspace-pinned-section'), 'Pinning used the display name instead of the project path');
  assert(projectGroup(projectPath).querySelector('[data-terminal-workspace-id="hierarchy-1"]'), 'Pinned project lost child workspaces');
  assert(document.querySelectorAll('[data-terminal-workspace-id="hierarchy-0"]').length === 1, 'Project pin duplicated an independently pinned workspace');
  await pinAction('Unpin workspace');
  assert(workspaceRow().closest('[data-sidebar-project]') === projectGroup(projectPath), 'Unpinned workspace did not return inside the pinned project');
  await pinAction('Pin workspace');
  projectGroup(projectPath).querySelector<HTMLButtonElement>('.group-header')!.click(); await frame();
  // Its rows may still be folding shut (inert) in the motion pass.
  assert(!projectGroup(projectPath).querySelector('.session-items:not(.is-leaving)'), 'Pinned project did not collapse');
  await projectAction(projectPath, 'Unpin project');
  assert(!projectGroup(projectPath).closest('.workspace-pinned-section'), 'Unpinned project remained in Pinned');
  assert(projectGroup(projectPath).querySelector('.group-header')!.getAttribute('aria-expanded') === 'false', 'Moving project forgot its collapse state');
  assert(workspaceRow().closest('.workspace-pinned-section'), 'Unpinning project removed the independent workspace pin');
  projectGroup(projectPath).querySelector<HTMLButtonElement>('.group-header')!.click(); await frame();
  await projectAction('/other/Interface', 'Pin project');
  groups = groups.slice(0, -1); render(); await frame();
  assert(projectGroup('/other/Interface').closest('.workspace-pinned-section'), 'Empty pinned project disappeared');
  projectGroup('/other/Interface').querySelector<HTMLButtonElement>('.project-menu-button')!.click();
  await frame(); await frame();
  [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')]
    .find(button => button.textContent?.trim() === 'Unpin project')!.click();
  await frame(); await frame();
  assert(![...document.querySelectorAll<HTMLElement>('[data-sidebar-project]')]
    .some(node => node.dataset.sidebarProject === '/other/Interface'), 'Empty unpinned project was retained');
  assert(document.activeElement === document.querySelector('.sidebar button:not(:disabled)'), 'Removing an empty pin lost keyboard focus');
  sidebarOverlay = false; render(); await frame();
  if (workspace.scrollHeight > workspace.clientHeight) {
    workspace.scrollTop = workspace.scrollHeight;
    await frame();
    const lastPane = [...workspace.querySelectorAll('.terminal-pane')].at(-1)!;
    assert(lastPane.getBoundingClientRect().bottom <= workspace.getBoundingClientRect().bottom + 1,
      'The last pane is unreachable in a short workspace');
    workspace.scrollTop = 0;
  }
  const originals = [...document.querySelectorAll('.xterm')];
  const originalHandle = handles.get(terminals[0].id);
  document.documentElement.style.setProperty('--workspace-terminal-font-size', '24');
  document.documentElement.dispatchEvent(new CustomEvent('interface-theme-change'));
  await frame(); await frame();
  assert(handles.get(terminals[0].id) === originalHandle, 'Reading size disposed the terminal');
  assert(getComputedStyle(workspace).getPropertyValue('--terminal-reading-size').trim() === '24', 'Reading size did not update');
  document.documentElement.style.setProperty('--workspace-terminal-font-size', '11.5');
  document.documentElement.dispatchEvent(new CustomEvent('interface-theme-change'));
  await frame(); await frame();

  const toggle = document.querySelector<HTMLButtonElement>('.workspace-sidebar-toggle')!;
  // Just right of the traffic lights (owner, 2026-10-20), over the column.
  const besideTraffic = toggle.getBoundingClientRect().left
    - document.querySelector('.workspace-toolbar-traffic')!.getBoundingClientRect().right;
  assert(besideTraffic > 0 && besideTraffic <= 8, `Sidebar toggle is not beside the traffic lights (${besideTraffic}px)`);
  if (innerWidth > 840) {
    // Hiding slides the column away while the one toggle stays exactly put.
    const grid = document.querySelector<HTMLElement>('.app-grid')!;
    // A slide or fold runs on the animation clock, which a loaded machine
    // pushes well past its nominal duration: a fixed 400 ms wait raced it.
    // Wait for every finite animation to finish, then for what follows it (the
    // refit after a slide, the unmount after a fold) to commit.
    const settled = async (): Promise<void> => {
      await Promise.allSettled(document.getAnimations()
        .filter(animation => animation.effect?.getComputedTiming().endTime !== Infinity)
        .map(animation => animation.finished));
      await frame(); await frame();
    };
    const toggleAt = toggle.getBoundingClientRect();
    const openLeft = shell.getBoundingClientRect().left;
    const closedLeft = grid.getBoundingClientRect().left + parseFloat(getComputedStyle(grid).paddingLeft);
    const openCols = handles.get(terminals[0].id)!.size()!.columns;
    toggle.click(); await frame(); await frame();
    const midLeft = shell.getBoundingClientRect().left;
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) assert(midLeft === closedLeft, 'Reduced motion still slid the sidebar');
    else {
      assert(midLeft < openLeft && midLeft > closedLeft, `Sidebar did not slide (${openLeft} -> ${midLeft} -> ${closedLeft})`);
      assert(handles.get(terminals[0].id)!.size()!.columns === openCols, 'Terminal refit mid-slide');
    }
    await settled();
    await until(() => handles.get(terminals[0].id)!.size()!.columns > openCols,
      () => `Terminal did not refit after the slide (${handles.get(terminals[0].id)!.size()!.columns} <= ${openCols} columns)`);
    assert(Math.abs(shell.getBoundingClientRect().left - closedLeft) <= 1, 'Hidden sidebar left a gap');
    assert(getComputedStyle(document.querySelector('.sidebar')!).visibility === 'hidden', 'Hidden sidebar is still reachable');
    const hiddenAt = toggle.getBoundingClientRect();
    assert(hiddenAt.left === toggleAt.left && hiddenAt.top === toggleAt.top, 'Sidebar toggle moved when the sidebar hid');
    toggle.click(); await settled();
    assert(Math.abs(shell.getBoundingClientRect().left - openLeft) <= 1, 'Sidebar did not return to its column');
    // The motion pass stops here; later checks assume reduced motion.
    if (sidebarSlideOnly) {
      // A project's rows fold shut and open with the chevron, never blinking.
      const group = projectGroup(projectPath);
      const header = group.querySelector<HTMLButtonElement>('.group-header')!;
      const list = (): HTMLElement | null => group.querySelector('.session-items');
      const full = list()!.getBoundingClientRect().height;
      // The first frame the fold has moved. Two fixed frames raced a loaded
      // machine, where the animation can start a few frames late; waiting for
      // movement still proves the rows pass through a middle height.
      const firstMove = async (moved: (height: number) => boolean): Promise<number> => {
        let height = list()?.getBoundingClientRect().height ?? 0;
        for (let i = 0; i < 20 && list() !== null && !moved(height); i += 1) {
          await frame();
          height = list()?.getBoundingClientRect().height ?? 0;
        }
        return height;
      };
      header.click(); await frame();
      const shutting = list();
      assert(shutting !== null && shutting.classList.contains('is-leaving') && shutting.inert,
        'Collapsing did not fold the rows shut');
      const midShut = await firstMove(height => height < full);
      assert(midShut > 0 && midShut < full, `Rows did not fold shut gradually (${full} -> ${midShut})`);
      await settled();
      assert(list() === null, 'Folded rows stayed mounted');
      header.click(); await frame();
      const midOpen = await firstMove(height => height > 0);
      assert(list()!.classList.contains('is-entering') && midOpen > 0 && midOpen < full,
        `Rows did not fold open gradually (${midOpen} of ${full})`);
      await settled();
      assert(Math.abs(list()!.getBoundingClientRect().height - full) <= 1 && !list()!.classList.contains('is-entering'),
        'Rows did not open to their full height');
      return { sidebarSlide: midLeft, fold: { full, midShut, midOpen } };
    }
  } else { toggle.click(); await frame(); toggle.click(); await frame(); }
  assert(originals.every((node, i) => node === document.querySelectorAll('.xterm')[i]), 'Sidebar toggle remounted terminals');
  document.querySelector<HTMLButtonElement>('.terminal-pane-action[aria-label="Maximize terminal pane"]')!.click();
  await frame(); await frame();
  const expanded = document.querySelector<HTMLElement>('.terminal-pane.is-maximized')!;
  assert(expanded.getBoundingClientRect().width >= workspace.clientWidth - 2, 'Expanded pane did not fill the reading area');
  assert(expanded.querySelector('[aria-label="Restore terminal layout"]'), 'Expanded view has no accessible return action');
  // A second tab must not leave unused title space in a wide reading pane.
  terminals.push({ ...terminals[0], id: 'hierarchy-background', title: 'Earlier attempt' });
  state = openTerminalTab(state, 'pane-0', 'hierarchy-background');
  state = focusTerminalTab(state, terminals[0].id);
  render(); await frame(); await frame();
  const tabs = expanded.querySelector<HTMLElement>('.terminal-tabs')!;
  const lastTab = tabs.lastElementChild!.getBoundingClientRect();
  assert(Math.abs(lastTab.right - tabs.getBoundingClientRect().right) < 2,
    'Tabbed pane leaves spare header space unused');
  assert(tabs.querySelector('.terminal-tab-wrap.is-active')!.getBoundingClientRect().width > 220,
    'Active task title is still capped in a wide reading pane');
  state = closeTerminalTab(state, 'hierarchy-background');
  render(); await frame(); await frame();
  document.querySelector<HTMLButtonElement>('.terminal-pane-action[aria-label="Restore terminal layout"]')!.click();
  await frame(); await frame();
  assert(handles.size === count, 'Maximizing disposed a terminal');
  assert(originals.every((node, i) => node === document.querySelectorAll('.xterm')[i]), 'Reading view replaced an existing terminal');
  handles.get(terminals[0].id)!.paste('hierarchy-input');
  assert(inputs.some(input => input.includes('hierarchy-input')), 'Native terminal input was lost');
  (document.activeElement as HTMLElement)?.blur();
  const panes = handles.size;
  // Last: it closes a pane.
  if (count === 6 && innerWidth > 1100) await sixPaneGrid(workspace, () => {
    state = closeTerminalTab(state, terminals[3].id); render();
  });
  return { count, theme, chrome, width: innerWidth, panes, readingSize };
}

/**
 * The six-pane grid the packaged smoke used to pin by number (review F2,
 * 2026-10-15), checked here against the tokens that draw it: a look change
 * edits `--tile-gap`, `--tile-radius` or `--tile-head-h` and this follows it,
 * while a grid that stops being an equal, flat 3 by 2 — or that does not
 * reflow to a full-width 3 + 2 when a pane closes — fails the gate instead of
 * the promote.
 */
async function sixPaneGrid(workspace: HTMLElement, closeFourth: () => void): Promise<void> {
  const token = (name: string) => Number.parseFloat(getComputedStyle(workspace).getPropertyValue(name));
  const gap = token('--tile-gap');
  const radius = token('--tile-radius');
  const head = token('--tile-head-h');
  assert([gap, radius, head].every(Number.isFinite), 'Tile tokens are not set on the terminal workspace');
  assert(workspace.classList.contains('is-equal-grid'), 'Six launched panes are not an equal grid');
  const near = (left: number, right: number) => Math.abs(left - right) <= 1;
  const distinct = (values: number[]) => values.filter((value, index) =>
    values.findIndex(other => near(other, value)) === index).length;
  const spread = (values: number[]) => Math.max(...values) - Math.min(...values);
  const panes = [...document.querySelectorAll<HTMLElement>('.terminal-pane')];
  assert(panes.length === 6, `Expected six panes, found ${panes.length}`);
  const boxes = panes.map(pane => pane.getBoundingClientRect());
  assert(distinct(boxes.map(box => box.left)) === 3 && distinct(boxes.map(box => box.top)) === 2,
    'Six panes do not form a 3 by 2 grid');
  assert(spread(boxes.map(box => box.width)) <= 1 && spread(boxes.map(box => box.height)) <= 1,
    'Six panes are not the same size');
  const grid = getComputedStyle(workspace);
  assert(near(Number.parseFloat(grid.columnGap), gap) && near(Number.parseFloat(grid.rowGap), gap),
    `Grid gutters ${grid.columnGap}/${grid.rowGap} do not follow --tile-gap (${gap}px)`);
  const ordered = [...boxes].sort((left, right) => left.top - right.top || left.left - right.left);
  for (const [index, box] of ordered.entries()) {
    const next = ordered[index + 1];
    if (next !== undefined && near(next.top, box.top)) {
      assert(near(next.left - box.right, gap), 'Neighbouring panes are not one gutter apart');
    }
  }
  for (const pane of panes) {
    const style = getComputedStyle(pane);
    assert(near(Number.parseFloat(style.borderTopLeftRadius), radius),
      `Pane corner ${style.borderTopLeftRadius} does not follow --tile-radius (${radius}px)`);
    assert(style.boxShadow === 'none', `A pane casts a shadow: ${style.boxShadow}`);
    const height = pane.querySelector('.terminal-pane-head')!.getBoundingClientRect().height;
    assert(height >= head - 0.5 && height <= head + 1, `Pane head ${height}px does not follow --tile-head-h (${head}px)`);
  }
  assert(panes.filter(pane => pane.classList.contains('is-focused')).length === 1,
    'Six panes do not have exactly one focused pane');
  closeFourth();
  await frame(); await frame();
  const fiveRows = (): DOMRect[][] | null => {
    const remaining = [...document.querySelectorAll<HTMLElement>('.terminal-pane')]
      .map(pane => pane.getBoundingClientRect());
    if (remaining.length !== 5) return null;
    const grouped: DOMRect[][] = [];
    for (const box of remaining) {
      const row = grouped.find(candidate => near(candidate[0].top, box.top));
      if (row === undefined) grouped.push([box]); else row.push(box);
    }
    grouped.sort((left, right) => left[0].top - right[0].top);
    for (const row of grouped) row.sort((left, right) => left.left - right.left);
    return grouped.length === 2 && grouped[0].length === 3 && grouped[1].length === 2 ? grouped : null;
  };
  await until(() => fiveRows() !== null, 'Closing one of six panes did not reflow to a 3 + 2 grid');
  const [upper, lower] = fiveRows()!;
  assert(spread(upper.map(box => box.width)) <= 1 && spread(lower.map(box => box.width)) <= 1
    && near(upper[0].left, lower[0].left) && near(upper[2].right, lower[1].right)
    && lower[0].width > upper[0].width, 'Five panes are not a full-width 3 + 2 grid');
}
