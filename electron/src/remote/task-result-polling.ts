import type { TaskEvidence } from '../shared/harness';
import { startVisiblePolling } from '../renderer/visible-polling';

type Visibility = Pick<Document, 'hidden' | 'addEventListener' | 'removeEventListener'>;
interface TaskResultPolling {
  terminalId: string;
  generation: number;
  request: (path: string, init?: RequestInit) => Promise<Response>;
  results: (results: TaskEvidence[]) => void;
  status: (status: 'ready' | 'unavailable') => void;
}

/** One read at a time, with identity and disposal fences shared by web/native.
 * Aborting suppresses a native response; it does not cancel the Swift request. */
export function startTaskResultPolling(options: TaskResultPolling, visibility: Visibility = document): () => void {
  let disposed = false;
  const lifetime = new AbortController();
  const stop = startVisiblePolling(async () => {
    const request = new AbortController();
    const abort = (): void => request.abort();
    lifetime.signal.addEventListener('abort', abort, { once: true });
    const deadline = setTimeout(() => {
      abort();
      if (!disposed) options.status('unavailable');
    }, 10_000);
    try {
      const signal = request.signal;
      const response = await options.request(`/api/v1/terminals/${options.terminalId}/${options.generation}/results`, { signal });
      const value = await response.json() as { version: number; terminalId: string; generation: number; results: TaskEvidence[] };
      if (disposed) return;
      signal.throwIfAborted();
      if (value.version !== 1 || value.terminalId !== options.terminalId || value.generation !== options.generation || !Array.isArray(value.results)) {
        throw new Error('Task identity changed.');
      }
      options.results(value.results);
      options.status('ready');
    } catch { if (!disposed) options.status('unavailable'); }
    finally { clearTimeout(deadline); lifetime.signal.removeEventListener('abort', abort); }
  }, 10_000, visibility);
  return () => { disposed = true; lifetime.abort(); stop(); };
}
