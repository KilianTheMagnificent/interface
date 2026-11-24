import { expect, it } from 'vitest';
import { HISTORY_IDLE_MS, nextHistoryRead } from './history-poll';

it('re-reads while output moves, once after it stops, then only on the idle refresh', () => {
  let state = { cursor: 10, trailing: false, at: 0 };
  // Output advanced since the last read.
  const moving = nextHistoryRead(state, 42, 2_500);
  expect(moving).toEqual({ cursor: 42, trailing: true, at: 2_500 });
  state = moving!;
  expect(nextHistoryRead(state, 50, 5_000)).toEqual({ cursor: 50, trailing: true, at: 5_000 });
  state = nextHistoryRead(state, 50, 5_000)!;
  // Output stopped: one trailing read for the transcript that lags the bytes.
  const trailing = nextHistoryRead(state, 50, 7_500);
  expect(trailing).toEqual({ cursor: 50, trailing: false, at: 7_500 });
  state = trailing!;
  // Then nothing until the idle refresh.
  expect(nextHistoryRead(state, 50, 10_000)).toBeNull();
  expect(nextHistoryRead(state, 50, 7_500 + HISTORY_IDLE_MS - 1)).toBeNull();
  expect(nextHistoryRead(state, 50, 7_500 + HISTORY_IDLE_MS)).toEqual(
    { cursor: 50, trailing: false, at: 7_500 + HISTORY_IDLE_MS });
});
