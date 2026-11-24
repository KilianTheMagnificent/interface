import type { RemoteWorkspaceSnapshotV2 } from '../shared/remote';
import { startVisiblePolling } from '../renderer/visible-polling';

type Visibility = Pick<Document, 'hidden' | 'addEventListener' | 'removeEventListener'>;

/** The workspace list is read this often while the page shows. */
export const ROSTER_POLL_MS = 2_000;
/** An open iPhone session shows one terminal. What it uses of the list (the
 * terminal's size, generation and launch) changes along with its output or
 * its stream, so while neither happens it is re-read only this often. */
export const QUIET_SESSION_ROSTER_MS = 10_000;

export interface RosterPolling {
  request: (path: string) => Promise<Response>;
  /** True while a native session view is showing its terminal. */
  quiet: () => boolean;
  workspaces: (workspaces: RemoteWorkspaceSnapshotV2['workspaces']) => void;
  failed: (error: unknown) => void;
  now?: () => number;
}
export type RosterPoller = (() => void) & {
  /** The session's output moved: read at the next ordinary tick. */
  stale: () => void;
  /** The session's stream dropped: read now (or as soon as the page shows). */
  refresh: () => void;
};

/** One roster read at a time, none while hidden, one on return. */
export function startRosterPolling(options: RosterPolling, visibility: Visibility = document): RosterPoller {
  const now = options.now ?? Date.now;
  let stopped = false;
  let answered = false;
  let stale = true;
  let readAt = 0;
  const polling = startVisiblePolling(async () => {
    const at = now();
    if (options.quiet() && answered && !stale && at - readAt < QUIET_SESSION_ROSTER_MS) return;
    stale = false; readAt = at;
    try {
      const snapshot = await (await options.request('/api/v2/workspaces')).json() as RemoteWorkspaceSnapshotV2;
      if (snapshot.version !== 2) throw new Error('Update Interface on your Mac to use this companion.');
      if (stopped) return;
      answered = true;
      options.workspaces(snapshot.workspaces);
    } catch (error) {
      if (stopped) return;
      answered = false;
      options.failed(error);
    }
  }, ROSTER_POLL_MS, visibility);
  // Coming back always reads, whatever the session did while away.
  const shown = (): void => { if (!visibility.hidden) stale = true; };
  visibility.addEventListener('visibilitychange', shown);
  return Object.assign(() => {
    stopped = true; polling();
    visibility.removeEventListener('visibilitychange', shown);
  }, {
    stale: (): void => { stale = true; },
    refresh: (): void => { stale = true; polling.refresh(); },
  });
}
