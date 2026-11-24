import { startVisiblePolling } from './visible-polling';

type Visibility = Pick<Document, 'hidden' | 'addEventListener' | 'removeEventListener'>;
type Focus = Pick<Window, 'addEventListener' | 'removeEventListener'>;

/** The shared draft is checked this often, and read this often at first. */
export const SHARED_DRAFT_POLL_MS = 2_000;
/** Reads back off to this while the Mac's revision stays the same. */
export const SHARED_DRAFT_IDLE_POLL_MS = 10_000;
/** Unchanged reads at the quick pace before backing off. */
export const SHARED_DRAFT_QUICK_READS = 3;

export type SharedDraftPoller = (() => void) & {
  /** A local edit was saved: read at the next tick, then quickly for a while. */
  saved: () => void;
};

/**
 * The phone's shared-draft reads. The draft changes only when someone edits it
 * on the Mac, so a screen left open reads it every 2 s for the first few reads
 * and then doubles the wait up to 10 s while the revision stays the same. A
 * changed revision, a local save, window focus and the page showing again all
 * go back to the quick pace; showing again also reads at once.
 *
 * `read(quiet)` answers with the revision it read, or undefined when it read
 * nothing (a quiet read skipped for a local edit, or a failure). The first
 * read is the loud one. Ticks between reads cost a timer, not a request.
 */
export function startSharedDraftPolling(read: (quiet: boolean) => Promise<number | undefined>,
    options: { visibility?: Visibility; focus?: Focus; now?: () => number } = {}): SharedDraftPoller {
  const visibility = options.visibility ?? document;
  const focus = options.focus ?? (typeof window === 'undefined' ? undefined : window);
  const now = options.now ?? Date.now;
  let initial = true;
  let readAt = Number.NEGATIVE_INFINITY;
  let revision: number | undefined;
  let quick = SHARED_DRAFT_QUICK_READS;
  let wait = SHARED_DRAFT_POLL_MS;
  const quicken = (): void => {
    quick = SHARED_DRAFT_QUICK_READS;
    wait = SHARED_DRAFT_POLL_MS;
    readAt = Number.NEGATIVE_INFINITY;
  };
  const answered = (next: number | undefined): void => {
    if (next === undefined) return;
    if (next !== revision) {
      revision = next;
      quick = SHARED_DRAFT_QUICK_READS;
      wait = SHARED_DRAFT_POLL_MS;
      return;
    }
    if (quick > 0) quick -= 1;
    if (quick === 0) wait = Math.min(wait * 2, SHARED_DRAFT_IDLE_POLL_MS);
  };
  // Before startVisiblePolling's own listener runs the read it starts on return.
  const shown = (): void => { if (!visibility.hidden) quicken(); };
  visibility.addEventListener('visibilitychange', shown);
  focus?.addEventListener('focus', quicken);
  const polling = startVisiblePolling(async () => {
    if (!initial && now() - readAt < wait) return;
    const quiet = !initial;
    initial = false;
    readAt = now();
    answered(await read(quiet));
  }, SHARED_DRAFT_POLL_MS, visibility);
  return Object.assign(() => {
    polling();
    visibility.removeEventListener('visibilitychange', shown);
    focus?.removeEventListener('focus', quicken);
  }, { saved: quicken });
}
