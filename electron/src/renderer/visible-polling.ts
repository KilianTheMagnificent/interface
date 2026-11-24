export type VisiblePolling = (() => void) & { refresh: () => void };

type Visibility = Pick<Document, 'hidden' | 'addEventListener' | 'removeEventListener'>;

/** Read-only refreshes: one in flight, no hidden timer, one refresh on return.
 * Callers own error presentation and must ignore results after their cleanup.
 * Mutations and their delivery/retry policy never belong in this scheduler. */
export function startVisiblePolling(read: () => Promise<void>, delayMs: number | (() => number), visibility: Visibility = document): VisiblePolling {
  let stopped = false;
  let running = false;
  let refreshWanted = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const clear = (): void => { clearTimeout(timer); timer = undefined; };
  const settled = (): void => {
    running = false;
    if (stopped || visibility.hidden) return;
    if (refreshWanted) run();
    else timer = setTimeout(run, typeof delayMs === 'function' ? delayMs() : delayMs);
  };
  const run = (): void => {
    clear();
    if (stopped || visibility.hidden || running) return;
    running = true; refreshWanted = false;
    // Handle either outcome so a failed read does not kill future refreshes.
    void Promise.resolve().then(() => {
      if (!stopped && !visibility.hidden) return read();
      return undefined;
    }).then(settled, settled);
  };
  const changed = (): void => {
    clear();
    refreshWanted = !visibility.hidden;
    if (refreshWanted) run();
  };
  visibility.addEventListener('visibilitychange', changed);
  run();
  return Object.assign(() => {
    stopped = true; clear();
    visibility.removeEventListener('visibilitychange', changed);
  }, { refresh: (): void => {
    if (stopped) return;
    refreshWanted = true;
    run();
  } });
}
