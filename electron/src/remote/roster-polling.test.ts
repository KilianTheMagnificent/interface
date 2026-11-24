import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RemoteWorkspaceSnapshotV2 } from '../shared/remote';
import { QUIET_SESSION_ROSTER_MS, ROSTER_POLL_MS, startRosterPolling } from './roster-polling';

class Visibility extends EventTarget {
  hidden = false;
  change(hidden: boolean): void {
    this.hidden = hidden;
    this.dispatchEvent(new Event('visibilitychange'));
  }
}
const flush = async (): Promise<void> => { await vi.advanceTimersByTimeAsync(0); };
const snapshot: RemoteWorkspaceSnapshotV2 = { version: 2, workspaces: [] };
const answering = () => vi.fn(async (_path: string) => new Response(JSON.stringify(snapshot)));

function start(quiet: { value: boolean }, request = answering(), visibility = new Visibility()) {
  const workspaces = vi.fn();
  const failed = vi.fn();
  const stop = startRosterPolling({ request, quiet: () => quiet.value, workspaces, failed, now: Date.now }, visibility);
  return { request, workspaces, failed, stop, visibility };
}

describe('phone roster polling', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('reads the workspace list every two seconds outside a session', async () => {
    const { request, workspaces, stop } = start({ value: false });
    await flush();
    expect(request).toHaveBeenCalledExactlyOnceWith('/api/v2/workspaces');
    expect(workspaces).toHaveBeenCalledWith([]);
    await vi.advanceTimersByTimeAsync(3 * ROSTER_POLL_MS);
    expect(request).toHaveBeenCalledTimes(4);
    stop();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(request).toHaveBeenCalledTimes(4);
  });

  it('reads a quiet session rarely, at the next tick after output, and at once when its stream drops', async () => {
    const { request, failed, stop: poller } = start({ value: true });
    await flush();
    expect(request).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(QUIET_SESSION_ROSTER_MS - ROSTER_POLL_MS);
    expect(request).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(ROSTER_POLL_MS);
    expect(request).toHaveBeenCalledTimes(2);
    poller.stale();
    await vi.advanceTimersByTimeAsync(ROSTER_POLL_MS);
    expect(request).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(ROSTER_POLL_MS);
    expect(request).toHaveBeenCalledTimes(3);
    poller.refresh(); await flush();
    expect(request).toHaveBeenCalledTimes(4);
    expect(failed).not.toHaveBeenCalled();
    poller();
  });

  it('keeps retrying every two seconds while the Mac does not answer, even in a session', async () => {
    const request = vi.fn(async (_path: string): Promise<Response> => { throw new Error('The Mac is unavailable.'); });
    const { failed, stop } = start({ value: true }, request);
    await flush();
    await vi.advanceTimersByTimeAsync(2 * ROSTER_POLL_MS);
    expect(request).toHaveBeenCalledTimes(3);
    expect(failed).toHaveBeenCalledTimes(3);
    stop();
  });

  it('refuses an unknown snapshot version as a failure', async () => {
    const request = vi.fn(async (_path: string) => new Response(JSON.stringify({ version: 3, workspaces: [] })));
    const { workspaces, failed, stop } = start({ value: false }, request);
    await flush();
    expect(workspaces).not.toHaveBeenCalled();
    expect(failed.mock.calls[0][0]).toBeInstanceOf(Error);
    expect((failed.mock.calls[0][0] as Error).message).toBe('Update Interface on your Mac to use this companion.');
    stop();
  });

  it('reads nothing while hidden and reads at once on return, even in a quiet session', async () => {
    const visibility = new Visibility();
    const { request, stop } = start({ value: true }, answering(), visibility);
    await flush();
    visibility.change(true);
    stop.refresh();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(request).toHaveBeenCalledTimes(1);
    visibility.change(false); await flush();
    expect(request).toHaveBeenCalledTimes(2);
    visibility.change(true); visibility.change(false); await flush();
    expect(request).toHaveBeenCalledTimes(3);
    stop();
  });

  it('ignores an answer that lands after it stopped', async () => {
    let answer!: (response: Response) => void;
    const request = vi.fn((_path: string) => new Promise<Response>(resolve => { answer = resolve; }));
    const { workspaces, failed, stop } = start({ value: false }, request);
    await flush();
    stop();
    answer(new Response(JSON.stringify(snapshot))); await flush();
    expect(workspaces).not.toHaveBeenCalled();
    expect(failed).not.toHaveBeenCalled();
  });
});
