import { describe, expect, it } from 'vitest';
import type { AwayRequest, AwaySnapshot, AwayStatus } from '../shared/away';
import fixture from '../fixtures/project-away-v22.json';
import compact from '../fixtures/project-away-status-v28.json';
import v45 from '../fixtures/project-away-v45.json';
import v47 from '../fixtures/project-away-v47.json';
import { awayDefaultReviewer, awayReviewer, awayWithReviewer, withAwayReviewer } from './away-view';
import { EMPTY_AWAY_VIEW, awayButtonAction, awayFormOptions, awayNeedsSnapshot, awayProject, awayRefreshDelay, awayReceiptObserved, awaySessionKeys, mergeAwayView } from './away-view';
import { awayHiddenSessionKeys, awayLoopRows, awayModelFromValue, awayModelRows, awayModelValue, awayStartedThread } from './away-view';
import type { EngineChoices } from './composer-model';
const full = (): AwaySnapshot => structuredClone(fixture.snapshot) as AwaySnapshot;
const status = (): AwayStatus => structuredClone(compact.response) as AwayStatus;

describe('Away display snapshots', () => {
  it('preserves identities on unchanged polls but accepts same-revision uncertainty', () => {
    const snapshot = full(); const first = mergeAwayView(EMPTY_AWAY_VIEW, snapshot);
    expect(mergeAwayView(first, structuredClone(snapshot))).toBe(first);
    const uncertain = mergeAwayView(first, { ...snapshot, storageUncertain: true });
    expect(uncertain.snapshot?.storageUncertain).toBe(true);
    expect(uncertain.status?.storageUncertain).toBe(true);
    expect(mergeAwayView(uncertain, snapshot)).toBe(uncertain);
  });
  it('keeps reports cached during compact reads and refuses older responses', () => {
    const first = mergeAwayView(EMPTY_AWAY_VIEW, full());
    const summary = { ...status(), revision: first.status!.revision + 2 };
    const next = mergeAwayView(first, summary);
    expect(next.snapshot).toBe(first.snapshot);
    expect(mergeAwayView(next, structuredClone(summary))).toBe(next);
    expect(mergeAwayView(next, first.snapshot!)).toBe(next);
    const detail = { ...full(), revision: summary.revision };
    expect(mergeAwayView(next, detail).snapshot).toBe(detail);
  });
  it('reconciles completed starts and cancellations using the small shared wire fixture', () => {
    const request: AwayRequest = { version: 2, command: { kind: 'status' } };
    expect(request).toEqual(compact.request);
    expect(awayReceiptObserved(status(), 'completed-run')).toBe(true);
    expect(awayReceiptObserved(status(), 'cancelled-start')).toBe(true);
    expect(awayReceiptObserved(status(), 'absent')).toBe(false);
  });
});

it('slows only confirmed idle reads and keeps initial recovery and receipts responsive', () => {
  const idle = { ...status(), activeRuns: [] };
  expect(awayRefreshDelay(null, false, false)).toBe(2000);
  expect(awayRefreshDelay(idle, false, false)).toBe(15_000);
  expect(awayRefreshDelay(idle, true, false)).toBe(2000);
  expect(awayRefreshDelay(idle, false, true)).toBe(2000);
  expect(awayRefreshDelay(status(), false, false)).toBe(2000);
});

it('reads the snapshot again only after a compact read moved past it', () => {
  expect(awayNeedsSnapshot(EMPTY_AWAY_VIEW, false)).toBe(true);
  const cached = mergeAwayView(EMPTY_AWAY_VIEW, full());
  expect(awayNeedsSnapshot(cached, false)).toBe(false);
  expect(awayNeedsSnapshot(cached, true)).toBe(true);
  const moved = mergeAwayView(cached, { ...status(), revision: cached.status!.revision + 1 });
  expect(awayNeedsSnapshot(moved, false)).toBe(true);
  expect(awayNeedsSnapshot(mergeAwayView(moved, { ...full(), revision: moved.status!.revision }), false)).toBe(false);
});

it('lists the sessions each Away turn created and skips turns still launching', () => {
  const snapshot = full();
  const run: AwaySnapshot['runs'][number] = { id: 'run', cwd: '/project', options: snapshot.defaults,
    startedAtMs: 0, deadlineAtMs: 1, state: 'running', detail: '', turns: [
    { iteration: 1, role: 'work' as const, sessionKey: 'claude#work-1', state: 'completed', report: '' },
    { iteration: 1, role: 'review' as const, sessionKey: 'codex#review-1', state: 'completed', report: '' },
    { iteration: 2, role: 'work' as const, sessionKey: null, state: 'started', report: '' },
  ] };
  expect(awaySessionKeys({ ...snapshot, runs: [run] })).toEqual(['claude#work-1', 'codex#review-1']);
  expect(awaySessionKeys(null)).toEqual([]);
});

/* Review A1 (2026-10-14): with a thread focused, the coffee button started a
   loop in the Code tab's terminal folder (home), and while loops ran the
   "View Away loops" button still started one. */
describe('the Away button', () => {
  const snapshot = (presets: string[] = []): AwaySnapshot => ({
    ...full(), runs: [], presets: presets.map(cwd => ({ cwd, options: full().defaults })),
  });
  const running = (runs: { id: string; cwd: string }[]): AwayStatus => ({
    version: 2, revision: 1, storageUncertain: false, activeRuns: runs, runIds: runs.map(run => run.id), cancelledStarts: [],
  });

  it('opens the running loops instead of starting another', () => {
    const status = running([{ id: 'r1', cwd: '/a/web' }, { id: 'r2', cwd: '/a/api' }]);
    expect(awayButtonAction(snapshot(['/a/api']), status, '/a/api')).toEqual({ kind: 'view', cwd: '/a/api', runId: 'r2' });
    expect(awayButtonAction(snapshot(['/a/docs']), status, '/a/docs')).toEqual({ kind: 'view', cwd: '/a/web', runId: 'r1' });
  });

  it('asks before the first start in a project, then starts at once', () => {
    expect(awayButtonAction(snapshot(), running([]), '/a/web')).toEqual({ kind: 'confirm', cwd: '/a/web' });
    expect(awayButtonAction(snapshot(['/a/web']), running([]), '/a/web')).toEqual({ kind: 'start', cwd: '/a/web' });
    expect(awayButtonAction(snapshot(['/a/web']), running([]), '')).toEqual({ kind: 'confirm', cwd: '' });
  });

  it('works on the project in front: the focused thread on Threads, the terminal on Code', () => {
    let asked = 0;
    const terminal = (): string => { asked++; return '/Users/me'; };
    expect(awayProject('chat', '/a/web', terminal)).toBe('/a/web');
    expect(awayProject('chat', null, terminal)).toBe('');
    expect(awayProject('agent', '/a/api', terminal)).toBe('/a/api');
    expect(asked).toBe(0);
    expect(awayProject('code', '/a/web', terminal)).toBe('/Users/me');
  });

  it('offers a provider that can run when the project has no saved settings', () => {
    const defaults = { ...full().defaults, provider: 'codex' as const, model: 'gpt-6' };
    const base = { ...snapshot(), defaults };
    expect(awayFormOptions(base, '/a/web', provider => provider === 'claude'))
      .toEqual({ ...defaults, provider: 'claude', model: null });
    // Both can run: Codex works and Claude, the other vendor, reviews (protocol 47).
    expect(awayFormOptions(base, '/a/web', () => true)).toEqual({ ...defaults, reviewer: 'claude' });
    const saved = { ...defaults, objective: 'mine' };
    expect(awayFormOptions({ ...base, presets: [{ cwd: '/a/web', options: saved }] }, '/a/web', () => false)).toBe(saved);
  });
});

/* Review B3 (2026-10-14): each loop runs as a thread of its own worktree. */
describe('Away loops as threads (protocol 45)', () => {
  const threaded = (): AwaySnapshot => structuredClone(v45.snapshot) as AwaySnapshot;
  it('parses a run that names its thread and the thread turn of each work round', () => {
    const snapshot = threaded();
    const run = snapshot.runs[0]!;
    expect(run.thread?.key).toBe('codex#0199f2a4-7c1e-7d30-9a5b-1c2d3e4f5a6b');
    expect(run.turns.map(turn => turn.threadTurn)).toEqual([0, undefined]);
    expect(awayStartedThread(snapshot, run.id)).toBe(run.thread?.key);
    expect(awayStartedThread(snapshot, 'other')).toBeNull();
    expect(awayStartedThread(full(), 'x')).toBeNull();
  });
  it('lists a loop thread, and keeps its reviews and every older loop session out', () => {
    const snapshot = threaded();
    expect(awayHiddenSessionKeys(snapshot)).toEqual(['codex#0199f2a4-review']);
    expect(awaySessionKeys(snapshot)).toContain('codex#0199f2a4-7c1e-7d30-9a5b-1c2d3e4f5a6b');
    const older = full();
    older.runs = [{ ...snapshot.runs[0]!, thread: undefined }];
    expect(awayHiddenSessionKeys(older)).toEqual(awaySessionKeys(older));
  });
  it('lists running loops first, each opening its thread, and finished ones after', () => {
    const snapshot = threaded();
    const done = { ...snapshot.runs[0]!, id: 'done', state: 'completed' as const, detail: 'No justified work remains.',
      startedAtMs: 1, thread: undefined };
    snapshot.runs = [snapshot.runs[0]!, done];
    const rows = awayLoopRows(snapshot);
    expect(rows.map(row => [row.id, row.active])).toEqual([['0199f2a4-away-run', true], ['done', false]]);
    expect(rows[0]).toMatchObject({ project: 'atlas', objective: 'Fix the flaky login test and keep the suite green.',
      round: 'Round 1 of 6', threadKey: 'codex#0199f2a4-7c1e-7d30-9a5b-1c2d3e4f5a6b' });
    // A loop from before has no thread: its newest session opens instead.
    expect(rows[1]).toMatchObject({ threadKey: null, sessionKey: 'codex#0199f2a4-review' });
    expect(awayLoopRows(snapshot, 1)).toHaveLength(1);
  });
  it("offers the shared catalog's Codex and Claude models, each provider led by its default", () => {
    const engines = [{ id: 'grok' }, { id: 'claude' }, { id: 'codex', unavailable: 'not signed in' }];
    const choicesFor = (engine: string): EngineChoices => ({ models: engine === 'claude' ? ['opus'] : ['gpt-5.6'],
      efforts: [], permissionModes: [] });
    const rows = awayModelRows(engines, choicesFor, { ...full().defaults, provider: 'claude', model: null });
    const groups = [...new Set(rows.map(row => row.group))];
    expect(groups).toEqual(['Claude Code', 'Codex · Unavailable']);
    expect(rows[0]).toMatchObject({ label: 'Default model', group: 'Claude Code' });
    expect(rows.some(row => row.label === 'opus')).toBe(true);
    expect(rows.some(row => row.group?.startsWith('Grok'))).toBe(false);
    const value = awayModelValue({ provider: 'claude', model: null });
    expect(value).toBe(rows[0]!.value);
    expect(awayModelFromValue(value)).toEqual({ provider: 'claude', model: null });
    const opus = rows.find(row => row.label === 'opus')!.value;
    expect(awayModelFromValue(opus)).toEqual({ provider: 'claude', model: 'opus' });
    expect(awayModelValue({ provider: 'claude', model: 'opus' })).toBe(opus);
    expect(awayModelFromValue(rows.find(row => row.custom === true)!.value)).toBeNull();
  });
});

/* Owner, 2026-10-14: "Other vendor by default" (protocol 47). */
describe("an Away loop's reviewer", () => {
  it('defaults to the other vendor when it can run, else the worker reviews', () => {
    expect(awayDefaultReviewer('codex', () => true)).toBe('claude');
    expect(awayDefaultReviewer('claude', () => true)).toBe('codex');
    expect(awayDefaultReviewer('codex', provider => provider === 'codex')).toBeUndefined();
    const defaults = full().defaults;
    expect(withAwayReviewer(defaults, () => true).reviewer).toBe('claude');
    expect('reviewer' in withAwayReviewer({ ...defaults, reviewer: 'claude' }, provider => provider === 'codex'))
      .toBe(false);
    // Only Claude can run: Claude works, and reviews.
    const form = awayFormOptions(full(), '/a/new', provider => provider === 'claude')!;
    expect([form.provider, awayReviewer(form)]).toEqual(['claude', 'claude']);
  });
  it('keeps a saved project as it was: no reviewer means the worker reviews', () => {
    const saved = { ...full().defaults, objective: 'mine' };
    const snapshot = { ...full(), presets: [{ cwd: '/a/web', options: saved }] };
    expect(awayFormOptions(snapshot, '/a/web', () => true)).toBe(saved);
    expect(awayReviewer(saved)).toBe('codex');
  });
  it('stores the worker as no reviewer and the other vendor by name', () => {
    const defaults = full().defaults;
    expect(awayWithReviewer(defaults, 'claude').reviewer).toBe('claude');
    expect('reviewer' in awayWithReviewer({ ...defaults, reviewer: 'claude' }, 'codex')).toBe(false);
  });
  it('parses the shared v47 commands, presets and runs that name a reviewer', () => {
    const snapshot = structuredClone(v47.snapshot) as AwaySnapshot;
    expect(snapshot.defaults.reviewer).toBeUndefined();
    expect(snapshot.presets[0]?.options.reviewer).toBe('claude');
    expect(snapshot.runs[0]?.options.reviewer).toBe('claude');
    for (const request of v47.commands as AwayRequest[]) {
      expect(request.command.kind === 'start' || request.command.kind === 'save').toBe(true);
    }
    expect(v47.threadAway.reviewer).toBe('claude');
  });
});
