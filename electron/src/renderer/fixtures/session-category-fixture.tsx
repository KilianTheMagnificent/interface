import { flushSync } from 'react-dom';
import type { Root } from 'react-dom/client';
import type { SessionKind, SessionSummary } from '../../shared/engine';
import type { TerminalSummary } from '../../shared/ipc';
import { Sidebar } from '../components/Sidebar';
import { buildSidebarView, sessionSurfaceOf } from '../viewmodels';
import { reconcileWorkspaceState } from '../workspace';
import { assert, frame, noop } from './fixture-support';

/** Exercise the same surface projection with the real sidebar and row clicks. */
export async function sessionCategoryFixture(root: Root) {
  const sessions: SessionSummary[] = [
    { engine: 'codex', sessionId: 'article', title: 'Refine quarterly economics article',
      cwd: '/fixture/conversation', status: 'idle', cloud: false, origin: 'app',
      firstSeen: '2026-09-09T00:00:00.000Z', lastActivity: '2026-09-09T00:00:00.000Z', archived: false },
  ];
  sessions.push({ ...sessions[0], sessionId: 'code', title: 'Codex work conversation', kind: 'code' },
    { ...sessions[0], sessionId: 'store', title: 'Imported Codex work conversation', kind: 'code', origin: 'store' },
    { ...sessions[0], sessionId: 'voice', title: 'Spoken conversation', kind: 'voice' },
    { ...sessions[0], sessionId: 'agent', title: 'Agent task', kind: 'agent' });
  const terminal: TerminalSummary = { id: '00000000-0000-4000-8000-000000000001',
    generation: 1, title: 'Project terminal', root: '/fixture/project', cwd: '/fixture/project',
    state: 'running', size: { columns: 80, rows: 24 },
    createdAt: '2026-09-09T00:00:00.000Z', updatedAt: '2026-09-09T00:00:00.000Z' };
  let selected: string | null = null;
  let opened: SessionKind | null = null;
  let selectedWorkspace: string | null = null;
  const terminalWorkspaces = {
    records: [{ id: 'cli', name: 'CLI workspace', terminalIds: [terminal.id],
      openTerminalCount: 1, cwd: terminal.cwd }],
    activeId: 'cli', busy: false, canCreate: true,
    onSelect: (id: string) => { selectedWorkspace = id; },
    onCreate: noop, onRename: noop, onDelete: noop,
  };
  const render = async (surface: SessionKind, query = ''): Promise<void> => {
    const view = buildSidebarView(sessions.filter(session => sessionSurfaceOf(session) === surface),
      selected, new Set(), false, false, query, {
        terminals: surface === 'code' ? [terminal] : [], projectSessions: sessions, selectedTerminalId: null,
      });
    flushSync(() => root.render(<div style={{ height: 600, width: 320 }}>
      <Sidebar groups={view.groups} terminalProjects={view.terminalProjects}
        terminalWorkspaces={surface === 'code' ? terminalWorkspaces : undefined}
        terminalsActive={surface === 'code'}
        sectionLabel={surface === 'chat' ? 'Threads' : surface === 'code' ? 'Workspaces' : 'Tasks'}
        codeWorkspaces={surface === 'code'} query={query} searching={view.searching}
        rateLimits={{}} hidden={view.hidden} hideable={view.hideable}
        onSelect={key => {
          selected = key;
          opened = sessionSurfaceOf(sessions.find(session => `${session.engine}#${session.sessionId}` === key)!);
        }} onNew={noop} onNewInProject={noop} onArchive={noop} onToggleFilters={noop} />
    </div>));
    await frame(); await frame();
  };
  await render('code');
  assert(!document.body.textContent?.includes('Refine quarterly'), 'Imported chat leaked into Code');
  assert(!document.querySelector('.session-row'), 'A conversation row leaked into Code');
  assert(!document.body.textContent?.includes('Codex work conversation'), 'Code-capable conversation leaked into Code');
  assert(document.body.textContent?.includes('project'), 'Code lost its terminal project');
  const cli = document.querySelector<HTMLButtonElement>('[data-terminal-workspace-id="cli"]');
  assert(cli, 'Code lost its CLI workspace');
  cli!.click();
  assert(selectedWorkspace === 'cli' && selected === null, 'CLI workspace click opened a conversation');
  await render('code', 'quarterly');
  assert(!document.body.textContent?.includes('Refine quarterly'), 'Search bypassed the Code category');
  await render('code', 'Codex work');
  assert(!document.body.textContent?.includes('Codex work conversation'), 'Search leaked code-capable chats into Code');
  await render('chat');
  assert(document.body.textContent?.includes('Spoken conversation'), 'Voice was not categorized as Chat');
  assert(document.body.textContent?.includes('Imported Codex work conversation'), 'Imported work is missing from Chat');
  assert(!document.querySelector('[data-terminal-workspace-id]'), 'CLI workspace leaked into Chat');
  const work = [...document.querySelectorAll<HTMLButtonElement>('button')]
    .find(button => button.textContent?.trim() === 'Codex work conversation');
  assert(work, 'Code-capable conversation is missing from Chat');
  work!.click();
  assert(selected === 'codex#code' && opened === 'chat', 'Work conversation click opened the wrong identity or surface');
  const article = [...document.querySelectorAll<HTMLButtonElement>('button')]
    .find(button => button.textContent?.includes('Refine quarterly'));
  assert(article, 'Imported conversation is missing from Chat');
  article!.click();
  assert(selected === 'codex#article' && opened === 'chat', 'Chat click opened the wrong identity or surface');
  await render('agent');
  assert(document.body.textContent?.includes('Agent task') && !document.body.textContent?.includes('Refine quarterly'),
    'Agent categories crossed');
  const restored = { openKeys: ['codex#code'], focusedKey: 'codex#code', maximizedKey: null };
  const known = (surface: SessionKind) => sessions.filter(session => sessionSurfaceOf(session) === surface)
    .map(session => `${session.engine}#${session.sessionId}`);
  assert(reconcileWorkspaceState(restored, known('code')).openKeys.length === 0,
    'Restored Code pane retained a conversation');
  assert(reconcileWorkspaceState(restored, known('chat')).focusedKey === 'codex#code',
    'Restored Chat pane lost its work conversation');
  return { uncategorized: 'chat', explicitCode: 'chat', voice: 'chat', agent: 'agent',
    cliWorkspace: 'code', searchAndNavigation: true, restoredPanes: true };
}
