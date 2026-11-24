import { memo, useEffect, useMemo, useState } from 'react';
import type { TerminalSummary, HistoryPage } from '../shared/ipc';
import type { RemoteHistoryV2 } from '../shared/remote';
import { Conversation } from '../renderer/components/Conversation';
import type { HistoryView, SessionConfigView } from '../renderer/conversation-types';
import { createConversationProjection } from '../renderer/conversation-projection';
import { retainHistoryPage } from '../renderer/history-page-projection';
import { request, RemoteFailure } from './protocol';
import { HISTORY_TICK_MS, nextHistoryRead, type HistoryPollState } from './history-poll';

const emptyItems: never[] = [];
const noApprovals = {};
const noApprovalIds = new Set<string>();

/** Memoized: the terminal screen around it re-renders for every output frame
 * and roster poll, none of which changes this page. */
export const RemoteConversation = memo(function RemoteConversation({ terminal, visible, onTerminal, onUnpair, refined = false, onAvailable, outputCursor }: {
  terminal: TerminalSummary; visible: boolean; onTerminal: () => void; onUnpair: () => void;
  refined?: boolean; onAvailable?: (available: boolean) => void;
  /** The terminal's committed output cursor. Without it the page is re-read every tick. */
  outputCursor?: { readonly current: number };
}) {
  const [page, setPage] = useState<HistoryPage | null>(null);
  const [error, setError] = useState('');
  const [limit, setLimit] = useState(500);
  const [retry, setRetry] = useState(0);
  const target = `/api/v2/terminals/${terminal.id}/${terminal.generation}/history`;
  useEffect(() => {
    if (!visible) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    let reading = false;
    let polled: HistoryPollState | null = null;
    const abort = new AbortController();
    const read = async () => {
      try {
        const result: RemoteHistoryV2 = await (await request(`${target}?limit=${limit}`, { signal: AbortSignal.any([abort.signal, AbortSignal.timeout(10_000)]) })).json();
        if (stopped) return;
        if (result.version !== 2 || result.terminalId !== terminal.id || result.generation !== terminal.generation) {
          throw new Error('This conversation changed. Reopen the session.');
        }
        setPage(current => retainHistoryPage(current, result.history)); setError('');
      } catch (error) {
        if (stopped) return;
        if (error instanceof RemoteFailure && error.status === 401) onUnpair();
        setError(error instanceof Error ? error.message : 'Conversation updates are unavailable.');
      }
    };
    // Completion-scheduled: one read at a time. Opening, Retry, Load older and
    // a return to the foreground read at once; otherwise only when output
    // moved (see history-poll.ts).
    const tick = async (force: boolean) => {
      if (stopped || reading) return;
      clearTimeout(timer);
      if (!document.hidden) {
        const cursor = outputCursor?.current ?? 0;
        const next = force || polled === null || outputCursor === undefined
          ? { cursor, trailing: false, at: Date.now() }
          : nextHistoryRead(polled, cursor, Date.now());
        if (next !== null) {
          polled = next;
          reading = true;
          try { await read(); } finally { reading = false; }
        }
      }
      if (!stopped) timer = setTimeout(() => { void tick(false); }, HISTORY_TICK_MS);
    };
    const foreground = () => { if (!document.hidden) void tick(true); };
    document.addEventListener('visibilitychange', foreground);
    void tick(true);
    return () => {
      stopped = true; clearTimeout(timer); abort.abort();
      document.removeEventListener('visibilitychange', foreground);
    };
  }, [target, terminal.id, terminal.generation, visible, limit, retry, outputCursor]);
  const project = useMemo(() => createConversationProjection(), [target]);
  const items = useMemo(() => project(page?.events ?? []), [page, project]);
  useEffect(() => { onAvailable?.(page !== null && page.availability !== 'unavailable' && !error); }, [page, error, onAvailable]);
  const history: HistoryView = !page ? error ? { status: 'error', message: error } : { status: 'loading' }
    : page.availability === 'unavailable' ? { status: 'unavailable', message: refined
      ? 'This conversation is not available yet. Open the session on your Mac, then try again.'
      : page.reason ?? 'Use Terminal view for this session.' }
    : { status: 'ready', items, olderMessages: page.olderMessages, canLoadOlder: limit < 5000 };
  const engine = terminal.launch?.profileId === 'claude' ? 'claude' : 'codex';
  const config: SessionConfigView = { engine, engineLabel: engine === 'claude' ? 'Claude' : 'Codex', effort: null,
    models: [], effortLevels: [], canSetModel: false, canSetEffort: false, showEffort: false,
    pendingModel: false, pendingEffort: false, error: null };
  return <div className="mobile-conversation" hidden={!visible}>
    {page && error && <p className="remote-error" role="status">{error}</p>}
    {page?.availability === 'unavailable' && (refined
      ? <button className="phone-retry" onClick={() => setRetry(v => v + 1)}>Try again</button>
      : <button className="mobile-open-terminal" onClick={onTerminal}>Open Terminal view</button>)}
    <Conversation items={emptyItems} history={history} config={config} live={false} thinking={false}
      sessionKey={target} pathRoot={terminal.root} focused={visible} composer={<></>}
      onSend={onTerminal} onStop={onTerminal} onSetModel={onTerminal} onSetEffort={onTerminal}
      onApprove={onTerminal} onDeny={onTerminal} approvals={noApprovals} activeApprovalIds={noApprovalIds}
      onRetryHistory={() => setRetry(v => v + 1)} onLoadOlderHistory={() => setLimit(v => Math.min(5000, v + 500))} />
  </div>;
});
