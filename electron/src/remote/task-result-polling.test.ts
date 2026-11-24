import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TaskEvidence } from '../shared/harness';
import { startTaskResultPolling } from './task-result-polling';

class Visibility extends EventTarget {
  hidden = false;
  change(hidden: boolean): void { this.hidden = hidden; this.dispatchEvent(new Event('visibilitychange')); }
}
function pending<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
const reply = (terminalId = 'pane', generation = 1, results: TaskEvidence[] = []): Response =>
  Response.json({ version: 1, terminalId, generation, results });

describe('saved remote results', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('keeps native reads single-flight past the interval and rejects a timed-out response', async () => {
    const visibility = new Visibility(); const held = pending<Response>();
    const request = vi.fn().mockReturnValueOnce(held.promise).mockResolvedValue(reply());
    const results = vi.fn(); const status = vi.fn();
    const stop = startTaskResultPolling({ terminalId: 'pane', generation: 1, request, results, status }, visibility);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(request).toHaveBeenCalledTimes(1);
    expect(status).toHaveBeenLastCalledWith('unavailable');
    expect(request.mock.calls[0]?.[1].signal.aborted).toBe(true);
    held.resolve(reply()); await vi.advanceTimersByTimeAsync(0);
    expect(results).not.toHaveBeenCalled();
    expect(status).toHaveBeenLastCalledWith('unavailable');
    await vi.advanceTimersByTimeAsync(10_000);
    expect(request).toHaveBeenCalledTimes(2);
    expect(results).toHaveBeenCalledOnce();
    stop(); expect(vi.getTimerCount()).toBe(0);
  });

  it('does no hidden reads and refreshes once on visibility return', async () => {
    const visibility = new Visibility(); visibility.hidden = true;
    const request = vi.fn().mockResolvedValue(reply());
    const results = vi.fn(); const status = vi.fn();
    const stop = startTaskResultPolling({ terminalId: 'pane', generation: 1, request, results, status }, visibility);
    await vi.advanceTimersByTimeAsync(60_000); expect(request).not.toHaveBeenCalled();
    visibility.change(false); await vi.advanceTimersByTimeAsync(0);
    expect(results).toHaveBeenCalledOnce();
    visibility.change(true); await vi.advanceTimersByTimeAsync(60_000);
    expect(request).toHaveBeenCalledOnce();
    stop();
  });

  it('ignores an old generation even when its native response arrives after replacement', async () => {
    const visibility = new Visibility(); const old = pending<Response>();
    const oldResults = vi.fn(); const oldStatus = vi.fn();
    const oldRequest = vi.fn().mockReturnValue(old.promise);
    const dispose = startTaskResultPolling({ terminalId: 'pane', generation: 1, request: oldRequest, results: oldResults, status: oldStatus }, visibility);
    await vi.advanceTimersByTimeAsync(0); dispose();
    expect(oldRequest.mock.calls[0]?.[1].signal.aborted).toBe(true);
    const results = vi.fn(); const status = vi.fn();
    const stop = startTaskResultPolling({ terminalId: 'pane', generation: 2, request: vi.fn().mockResolvedValue(reply('pane', 2)), results, status }, visibility);
    await vi.advanceTimersByTimeAsync(0);
    old.resolve(reply()); await vi.advanceTimersByTimeAsync(0);
    expect(results).toHaveBeenCalledOnce(); expect(oldResults).not.toHaveBeenCalled(); expect(oldStatus).not.toHaveBeenCalled();
    stop(); expect(vi.getTimerCount()).toBe(0);
  });

  it('preserves confirmed data on failures and rejects mismatched identities', async () => {
    const visibility = new Visibility(); const results = vi.fn(); const status = vi.fn();
    const request = vi.fn().mockResolvedValueOnce(reply()).mockRejectedValueOnce(new Error('offline')).mockResolvedValue(reply('other'));
    const stop = startTaskResultPolling({ terminalId: 'pane', generation: 1, request, results, status }, visibility);
    await vi.advanceTimersByTimeAsync(20_000);
    expect(results).toHaveBeenCalledOnce(); expect(status.mock.calls).toEqual([['ready'], ['unavailable'], ['unavailable']]);
    stop();
  });
});
