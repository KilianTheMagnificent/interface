import { expect, it } from 'vitest';
import fixture from '../fixtures/remote-projects-v3.json';
import { OTHER_PROJECT, buildSidebarView } from '../renderer/viewmodels';
import type { SessionSummary } from '../shared/engine';
import type { TerminalSummary } from '../shared/ipc';

// The phone's roster v3 files terminals by `terminal_projects` in
// crates/interface-remote/src/workspaces.rs. Both sides read this fixture, so
// the iPhone list and the Mac sidebar cannot disagree about a session's project.
it('files each terminal under the same project as the Rust phone roster', () => {
  const sessions = fixture.sessions as unknown as SessionSummary[];
  const terminals = fixture.terminals as unknown as TerminalSummary[];
  const view = buildSidebarView(sessions, null, new Set(), false, false, '', {
    terminals, projectSessions: sessions, selectedTerminalId: null,
  });
  const projects = Object.fromEntries(Object.entries(view.terminalProjects)
    .map(([id, project]) => [id, project === OTHER_PROJECT ? null : project]));
  expect(projects).toEqual(fixture.expected);
});
