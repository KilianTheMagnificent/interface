import { flushSync } from 'react-dom';
import { useMemo } from 'react';
import type { Root } from 'react-dom/client';
import { TaskInspector } from '../components/TaskInspector';
import { useSharedDraft } from '../use-shared-draft';
import type { HarnessView, SharedDraft } from '../../shared/harness';
import { assert, wait } from './fixture-support';
const target = { kind: 'session' as const, key: 'codex#fixture' };
export async function harnessFixture(root: Root, visual = false): Promise<unknown> {
  let captures = 0; let checks = 0; let closed = false;
  const view: HarnessView = { version: 1, state: { version: 1, revision: 0, attention: [], tasks: [], drafts: [], branches: [] }, catalog: [],
    result: { id: 'r', target, objective: 'Review change', root: '/project', baseline: 'abc', candidate: 'hash', changedPaths: ['app.rs'], checks: [], checkCandidate: null, findings: ['Concurrent edits are not attributed to the task.'], state: 'captured', at: 1 }, stale: true,
    checks: { root: '/project', packagePresent: true, checks: ['test'] } };
  flushSync(() => root.render(<TaskInspector view={view} busy={false} error={null} servers={[]}
    onClose={() => { closed = true; }} onCapture={() => { captures++; }} onRunChecks={() => { checks++; }}
    onInspect={() => {}} onContext={() => {}} onAttach={() => {}} onPreset={() => {}} onOpenSession={() => {}} onFindServers={() => {}} onOpenServer={() => {}} />));
  await wait(30);
  const dialog = document.querySelector<HTMLElement>('[role="dialog"]')!;
  const box = dialog.getBoundingClientRect();
  assert(box.left >= 0 && box.right <= innerWidth && box.top >= 0 && box.bottom <= innerHeight, 'Task dialog escaped the viewport');
  assert(Math.abs((box.left + box.right) / 2 - innerWidth / 2) < 2, 'Task dialog is not centered');
  assert(captures === 0 && checks === 0, 'Opening an inspector dispatched an effect');
  assert(document.body.textContent?.includes('evidence is stale'), 'Stale candidate warning missing');
  const run = [...document.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === 'Run test');
  assert(run?.disabled, 'Stale checks can be launched');
  document.querySelector('[role="dialog"]')!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
  assert(closed, 'Task dialog Escape did not close');
  if (visual) return { inspector: "visible" };
  let stored: SharedDraft = { target, revision: 1, text: 'saved on phone' };
  let attempts = 0;
  let reads = 0;
  let readBarrier: Promise<void> | undefined;
  function DraftFixture() {
    const port = useMemo(() => ({ read: async () => { reads++; await readBarrier; return structuredClone(stored); }, save: async (revision: number, text: string) => {
      attempts++; if (revision !== stored.revision) throw new Error('conflict');
      stored = { ...stored, revision: revision + 1, text }; return structuredClone(stored);
    } }), []);
    const { draft, setDraft, draftStatus, reloadDraft } = useSharedDraft(port);
    return <><textarea value={draft} onChange={event => setDraft(event.target.value)} /><button onClick={() => setDraft('desktop edit')}>Edit draft</button><button onClick={reloadDraft}>Reload draft</button><p>{draftStatus}</p></>;
  }
  flushSync(() => root.render(<DraftFixture />)); await wait(30);
  assert(document.querySelector('textarea')?.value === 'saved on phone', 'Confirmed draft did not restore');
  stored = { target, revision: 2, text: 'new phone edit' };
  document.querySelector('button')!.click(); await wait(500);
  assert(stored.text === 'new phone edit', 'A stale desktop edit overwrote the phone');
  assert(document.querySelector('textarea')?.value === 'desktop edit', 'Conflict erased the local draft');
  assert(attempts === 1, 'Draft conflict retried automatically');
  document.querySelectorAll('button')[1].click(); await wait(30);
  assert(document.querySelector('textarea')?.value === 'new phone edit', 'Explicit reload did not restore the confirmed draft');
  stored = { invalid: true } as unknown as SharedDraft;
  flushSync(() => root.render(<DraftFixture key="malformed" />)); await wait(30);
  assert(document.querySelector('textarea')?.value === '', 'Malformed draft replaced the local text');
  assert(document.body.textContent?.includes('unavailable'), 'Malformed draft was not reported');
  flushSync(() => root.render(null));
  stored = { target, revision: 3, text: 'slow saved draft' };
  let release!: () => void;
  readBarrier = new Promise<void>(resolve => { release = resolve; }); reads = 0;
  const hiddenDescriptor = Object.getOwnPropertyDescriptor(document, 'hidden');
  let hidden = false;
  Object.defineProperty(document, 'hidden', { configurable: true, get: () => hidden });
  const visibility = (value: boolean): void => { hidden = value; document.dispatchEvent(new Event('visibilitychange')); };
  try {
    flushSync(() => root.render(<DraftFixture key="slow" />)); await wait(2200);
    assert(reads === 1, 'Slow initial draft read overlapped');
    release(); readBarrier = undefined; await wait(80);
    assert(reads === 1, 'Slow read accumulated timer work');
    assert(document.querySelector('textarea')?.value === 'slow saved draft', 'Slow draft did not restore');
    visibility(true); await wait(2200);
    assert(reads === 1, 'Hidden draft kept polling');
    stored = { target, revision: 4, text: 'changed while hidden' };
    visibility(false); await wait(80);
    assert(Number(reads) === 2 && document.querySelector('textarea')?.value === stored.text, 'Returning to the window did not refresh the draft');
  } finally {
    flushSync(() => root.render(null)); release();
    if (hiddenDescriptor) Object.defineProperty(document, 'hidden', hiddenDescriptor);
    else Reflect.deleteProperty(document, 'hidden');
  }
  return { malformedDraft: 'refused', inspector: 'passed', draftConflict: 'passed', slowReadQueue: 'bounded', hiddenPolling: 'paused', providerEffects: 0 };
}
