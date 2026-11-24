import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  SHARED_DRAFT_IDLE_POLL_MS, SHARED_DRAFT_POLL_MS, SHARED_DRAFT_QUICK_READS, startSharedDraftPolling,
} from './shared-draft-polling';

class Visibility extends EventTarget {
  hidden = false;
  change(hidden: boolean): void {
    this.hidden = hidden;
    this.dispatchEvent(new Event('visibilitychange'));
  }
}
const flush = async (): Promise<void> => { await vi.advanceTimersByTimeAsync(0); };

/** A Mac whose draft revision the test moves; records when each read lands. */
function start() {
  const visibility = new Visibility();
  const focus = new EventTarget();
  const mac = { revision: 1 };
  const reads: { at: number; quiet: boolean }[] = [];
  const poller = startSharedDraftPolling(async quiet => {
    reads.push({ at: Date.now(), quiet });
    return mac.revision;
  }, { visibility, focus: focus as unknown as Window });
  const gaps = (): number[] => reads.slice(1).map((read, index) => read.at - reads[index].at);
  return { visibility, focus, mac, reads, poller, gaps };
}

describe('phone shared-draft polling', () => {
  beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(0); });
  afterEach(() => vi.useRealTimers());

  it('reads every two seconds at first, then doubles the wait to ten while nothing changes', async () => {
    const { reads, poller, gaps } = start();
    await flush();
    expect(reads).toEqual([{ at: 0, quiet: false }]);
    await vi.advanceTimersByTimeAsync(60_000);
    const quick = Array<number>(SHARED_DRAFT_QUICK_READS).fill(SHARED_DRAFT_POLL_MS);
    expect(gaps().slice(0, 6)).toEqual([...quick, 4_000, 8_000, SHARED_DRAFT_IDLE_POLL_MS]);
    expect(Math.max(...gaps())).toBe(SHARED_DRAFT_IDLE_POLL_MS);
    expect(reads.slice(1).every(read => read.quiet)).toBe(true);
    // Ten reads in the first minute where a fixed two seconds made thirty-one.
    expect(reads).toHaveLength(10);
    poller();
    const count = reads.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(reads).toHaveLength(count);
  });

  it('goes back to two seconds when the Mac changes the draft', async () => {
    const { mac, reads, poller, gaps } = start();
    await vi.advanceTimersByTimeAsync(40_000);
    expect(gaps().at(-1)).toBe(SHARED_DRAFT_IDLE_POLL_MS);
    mac.revision = 2;
    const before = reads.length;
    // The next idle read sees the change; the ones after it are quick again.
    await vi.advanceTimersByTimeAsync(SHARED_DRAFT_IDLE_POLL_MS
      + SHARED_DRAFT_QUICK_READS * SHARED_DRAFT_POLL_MS);
    expect(reads.length).toBeGreaterThan(before + SHARED_DRAFT_QUICK_READS);
    expect(gaps().slice(before - 1)).toEqual([SHARED_DRAFT_IDLE_POLL_MS,
      ...Array<number>(reads.length - before - 1).fill(SHARED_DRAFT_POLL_MS)]);
    poller();
  });

  it('reads quickly again after a local save or focus, and at once when shown again', async () => {
    const { visibility, focus, reads, poller } = start();
    await vi.advanceTimersByTimeAsync(40_000);
    let before = reads.length;
    poller.saved();
    await vi.advanceTimersByTimeAsync(SHARED_DRAFT_POLL_MS);
    expect(reads).toHaveLength(before + 1);
    await vi.advanceTimersByTimeAsync(SHARED_DRAFT_POLL_MS);
    expect(reads).toHaveLength(before + 2);

    await vi.advanceTimersByTimeAsync(40_000);
    before = reads.length;
    focus.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(SHARED_DRAFT_POLL_MS);
    expect(reads).toHaveLength(before + 1);

    await vi.advanceTimersByTimeAsync(40_000);
    visibility.change(true);
    before = reads.length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(reads).toHaveLength(before);
    visibility.change(false); await flush();
    expect(reads).toHaveLength(before + 1);
    await vi.advanceTimersByTimeAsync(SHARED_DRAFT_POLL_MS);
    expect(reads).toHaveLength(before + 2);
    poller();
  });

  it('does not count a read that read nothing', async () => {
    const visibility = new Visibility();
    let reads = 0;
    const poller = startSharedDraftPolling(async () => { reads += 1; return undefined; },
      { visibility, focus: new EventTarget() as unknown as Window });
    await vi.advanceTimersByTimeAsync(20 * SHARED_DRAFT_POLL_MS);
    // Every tick still tries: a skipped or failed read never backs the pace off.
    expect(reads).toBe(21);
    poller();
  });
});
