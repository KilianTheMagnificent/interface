import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import { useState } from 'react';
import { Sidebar } from '../components/Sidebar';
import type { SidebarGroup, SidebarTerminalRow } from '../viewmodels';
import { useSidebarPeek } from '../sidebar-peek';
import { noop } from './fixture-support';

/** The hidden sidebar's hover reveal with App's wiring around the real hook,
 * rail toggle and Sidebar; the lane moves the native pointer and reads
 * `sidebarPeekState`. */
export function sidebarPeekFixture(root: Root): void {
  flushSync(() => root.render(null));
  document.documentElement.dataset.theme = 'dark';
  document.documentElement.dataset.chrome = 'refined';
  const rows: SidebarTerminalRow[] = ['Session recovery', 'Keyboard navigation'].map((title, index) => ({
    id: `peek-${index}`, title, cwd: '/fixture/Interface', status: 'running' as const, activity: 'idle' as const,
    root: '/fixture/Interface', state: 'running', selected: false, lastActivity: '2026-10-20T00:00:00Z' }));
  const groups: SidebarGroup[] = [{ projectName: 'Interface', projectPath: '/fixture/Interface',
    rows: [], counts: { total: 0, working: 0, waiting: 0 }, terminals: rows }];
  function Shell() {
    const [sidebarOpen, setSidebarOpen] = useState(false);
    const peek = useSidebarPeek(!sidebarOpen);
    return <div className={(sidebarOpen ? 'app-grid sidebar-overlay' : 'app-grid no-sidebar')
      + (peek.shown ? ' sidebar-peek' : '')}>
      <header className="workspace-toolbar">
        <div className="workspace-toolbar-leading"><span className="workspace-toolbar-traffic" />
          <button type="button" className="workspace-icon-btn workspace-sidebar-toggle" data-sidebar-peek-keep
            aria-label="Toggle sidebar" onClick={() => { setSidebarOpen(open => !open); }} {...peek.trigger}>
            <span className="codicon codicon-layout-sidebar-left" /></button>
          <span className="workspace-brand"><span className="workspace-brand-name">Interface</span></span>
        </div>
      </header>
      <Sidebar groups={groups} terminalProjects={Object.fromEntries(rows.map(row => [row.id, row.root]))}
        codeWorkspaces terminalsActive sectionLabel="Workspaces" newLabel="New workspace"
        rateLimits={{}} hidden={{ external: 0, trivial: 0 }} hideable={0}
        onSelect={noop} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop}
        onQuery={noop} query="" onSettings={noop}
        terminalWorkspaces={{ records: rows.map(row => ({ id: row.id, name: row.title, cwd: row.cwd,
          firstTerminalTitle: row.title, openTerminalCount: 1, terminalIds: [row.id] })),
          activeId: rows[0].id, onSelect: noop, onCreate: noop, onRename: noop, onDelete: noop,
          canCreate: true, busy: false }} />
      <main className="workspace-shell"><div className="workspace-content">
        <div className="sidebar-peek-work" style={{ height: '100%', background: 'var(--workbench-panel)' }} />
      </div></main>
      <footer className="statusbar"><span>Hover reveal</span></footer>
    </div>;
  }
  flushSync(() => root.render(<Shell />));
}

/** What the lane asserts on, all in CSS pixels. */
export function sidebarPeekState(): unknown {
  const grid = document.querySelector<HTMLElement>('.app-grid')!;
  const sidebar = document.querySelector<HTMLElement>('.sidebar')!;
  const shell = document.querySelector<HTMLElement>('.workspace-shell')!;
  const box = (selector: string) => {
    const { left, right, top, bottom } = document.querySelector(selector)!.getBoundingClientRect();
    return { left, right, top, bottom };
  };
  const rect = sidebar.getBoundingClientRect();
  const probe = document.elementFromPoint(rect.left + 40, rect.top + rect.height / 2);
  return {
    peek: grid.classList.contains('sidebar-peek'),
    docked: !grid.classList.contains('no-sidebar'),
    visibility: getComputedStyle(sidebar).visibility,
    sidebar: box('.sidebar'),
    shellLeft: shell.getBoundingClientRect().left,
    inset: parseFloat(getComputedStyle(grid).paddingLeft),
    paneLeft: parseFloat(getComputedStyle(grid).getPropertyValue('--pane-left')),
    toggle: box('.workspace-sidebar-toggle'),
    traffic: box('.workspace-toolbar-traffic'),
    onTop: probe !== null && probe.closest('.sidebar') !== null,
  };
}

/** The sidebar's left edge over the next `count` frames, from the next one. */
export async function sidebarPeekTrack(count: number): Promise<number[]> {
  const sidebar = document.querySelector<HTMLElement>('.sidebar')!;
  const lefts: number[] = [];
  for (let index = 0; index < count; index++) {
    await new Promise(resolve => requestAnimationFrame(resolve));
    lefts.push(sidebar.getBoundingClientRect().left);
  }
  return lefts;
}
