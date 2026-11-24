import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { startVisiblePolling } from './visible-polling';

class Visibility extends EventTarget {
  hidden = false;
  change(hidden: boolean): void {
    this.hidden = hidden;
    this.dispatchEvent(new Event('visibilitychange'));
  }
}
const flush = async (): Promise<void> => { await vi.advanceTimersByTimeAsync(0); };
function pending() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

describe('visible read polling', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('waits for a slow initial read instead of accumulating polls', async () => {
    const visibility = new Visibility();
    const first = pending();
    const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(undefined);
    const stop = startVisiblePolling(read, 2000, visibility);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(read).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
    first.resolve(); await flush();
    await vi.advanceTimersByTimeAsync(1999);
    expect(read).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(read).toHaveBeenCalledTimes(2);
    stop();
  });

  it('suspends hidden reads and refreshes immediately on return', async () => {
    const visibility = new Visibility(); visibility.hidden = true;
    const read = vi.fn().mockResolvedValue(undefined);
    const stop = startVisiblePolling(read, 2000, visibility);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(read).not.toHaveBeenCalled();
    visibility.change(false); await flush();
    expect(read).toHaveBeenCalledTimes(1);
    visibility.change(true);
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(60_000);
    visibility.change(false); await flush();
    expect(read).toHaveBeenCalledTimes(2);
    stop();
  });

  it('coalesces visibility changes during a read into one fresh read', async () => {
    const visibility = new Visibility();
    const first = pending();
    const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(undefined);
    const stop = startVisiblePolling(read, 2000, visibility);
    await flush();
    for (let i = 0; i < 5; i++) { visibility.change(true); visibility.change(false); }
    await flush(); expect(read).toHaveBeenCalledTimes(1);
    first.resolve(); await flush(); expect(read).toHaveBeenCalledTimes(2);
    stop();
    await vi.advanceTimersByTimeAsync(60_000);
    visibility.change(true); visibility.change(false); await flush();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('does not reschedule a disposed in-flight read and survives failures', async () => {
    const visibility = new Visibility();
    const last = pending();
    const read = vi.fn().mockRejectedValueOnce(new Error('offline')).mockReturnValue(last.promise);
    const stop = startVisiblePolling(read, 2000, visibility);
    await vi.advanceTimersByTimeAsync(2000);
    expect(read).toHaveBeenCalledTimes(2);
    stop(); last.resolve(); await flush();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('never starts a read disposed before its first microtask', async () => {
    const read = vi.fn().mockResolvedValue(undefined);
    const stop = startVisiblePolling(read, 2000, new Visibility());
    stop(); await flush();
    expect(read).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
});

it('coalesces an explicit refresh and uses the latest delay after a pending read', async () => {
  vi.useFakeTimers();
  try {
    const first = pending(); const visibility = new Visibility();
    const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(undefined);
    let delay = 2000;
    const stop = startVisiblePolling(read, () => delay, visibility);
    await flush(); stop.refresh(); stop.refresh();
    expect(read).toHaveBeenCalledOnce();
    delay = 15_000;
    first.resolve(); await flush(); expect(read).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(14_999); expect(read).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1); expect(read).toHaveBeenCalledTimes(3);
    stop(); stop.refresh(); await flush(); expect(read).toHaveBeenCalledTimes(3);
  } finally { vi.useRealTimers(); }
});
