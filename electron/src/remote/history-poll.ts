/** The phone's conversation page is re-read only when it can have changed:
 * the terminal's committed output cursor advanced since the last read, one
 * trailing read after output stops (the provider writes its transcript a
 * little after the bytes), and a slow idle refresh. The wire is unchanged. */
export const HISTORY_TICK_MS = 2_500;
export const HISTORY_IDLE_MS = 30_000;

export interface HistoryPollState {
  /** Output cursor when the page was last read. */
  readonly cursor: number;
  /** Output advanced before the last read, so one more read is owed. */
  readonly trailing: boolean;
  /** When the page was last read. */
  readonly at: number;
}

/** The state after a read taken now, or null when this tick needs none. */
export function nextHistoryRead(state: HistoryPollState, cursor: number, now: number): HistoryPollState | null {
  if (cursor !== state.cursor) return { cursor, trailing: true, at: now };
  if (state.trailing || now - state.at >= HISTORY_IDLE_MS) return { cursor, trailing: false, at: now };
  return null;
}
