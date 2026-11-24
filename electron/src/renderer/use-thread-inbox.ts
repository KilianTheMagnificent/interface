import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SessionKey, SessionSummary } from "../shared/engine";
import { searchesMessages, type JournalHit } from "../shared/thread-journal";
import type { ThreadCommand, ThreadControlResponse, ThreadsView } from "../shared/threads";
import {
  buildThreadInbox, parseThreadScope, threadRunEndVisit,
  type ThreadInboxModel, type ThreadScope,
} from "./thread-inbox";

/** The inbox's project scope: a renderer preference, like the sidebar's folds. */
export const THREAD_SCOPE_KEY = "interface.threads.scope";

/** Everything the Threads sidebar needs from App, as one memoized value. */
export interface ThreadInboxController {
  readonly model: ThreadInboxModel;
  readonly setScope: (scope: ThreadScope) => void;
  /**
   * Sends one user action's commands in order. The first refusal is reported
   * in the sidebar and the rest are not sent; resolves whether all were taken.
   * Nothing is retried: Rust's answer is the state.
   */
  readonly send: (commands: readonly ThreadCommand[]) => Promise<boolean>;
  /** Undo for Archive: the session's own unarchive. */
  readonly unarchive: (key: SessionKey) => void;
  /** The thread chords may act: the Threads surface shows and nothing covers it. */
  readonly shortcuts: boolean;
}

export interface ThreadInboxOptions {
  readonly threads: {
    readonly view: ThreadsView | null;
    readonly request: (command: ThreadCommand) => Promise<ThreadControlResponse>;
  };
  /** Threads-surface sessions after the existing filters, before the search. */
  readonly sessions: readonly SessionSummary[];
  readonly query: string;
  readonly selectedKey: SessionKey | null;
  readonly needsYou: ReadonlySet<SessionKey>;
  /** The thread in the focused pane of the Threads surface, or null. */
  readonly watchingKey: SessionKey | null;
  readonly shortcuts: boolean;
  readonly onError: (error: unknown) => void;
  readonly onUnarchive: (key: SessionKey) => void;
  /** Protocol 56: the thread journal's message search (App's `threadJournal`). */
  readonly searchMessages?: (query: string) => Promise<readonly JournalHit[]>;
}

/** How long typing rests before the journal is searched. */
export const MESSAGE_SEARCH_DELAY_MS = 200;

/**
 * One user action's commands, in order. The first refusal is reported and
 * ends the run: the rest are not sent, and nothing is retried. Resolves
 * whether every command was taken.
 */
export async function sendThreadCommands(
  request: (command: ThreadCommand) => Promise<unknown>,
  commands: readonly ThreadCommand[],
  onError: (error: unknown) => void,
): Promise<boolean> {
  for (const command of commands) {
    try {
      await request(command);
    } catch (error: unknown) {
      onError(error);
      return false;
    }
  }
  return true;
}

function readScope(): ThreadScope {
  try {
    return parseThreadScope(localStorage.getItem(THREAD_SCOPE_KEY));
  } catch {
    return parseThreadScope(null);
  }
}

/**
 * The Threads inbox's App half: the scope preference, the joined model, the
 * command sender, and the one automatic visit — a thread whose run ends while
 * it is the focused pane of a focused window has been seen as it finished.
 */
export function useThreadInbox({
  threads, sessions, query, selectedKey, needsYou, watchingKey, shortcuts, onError, onUnarchive,
  searchMessages,
}: ThreadInboxOptions): ThreadInboxController {
  const { view, request } = threads;
  const [scope, setScopeState] = useState<ThreadScope>(readScope);
  const setScope = useCallback((next: ThreadScope) => {
    setScopeState(next);
    try {
      localStorage.setItem(THREAD_SCOPE_KEY, next);
    } catch {
      // A storage-less context keeps the scope for this run only.
    }
  }, []);

  // The journal's matches for the query they answered; a stale answer (the
  // query moved on) is never drawn, and a failed search draws none.
  const [found, setFound] = useState<{ query: string; hits: readonly JournalHit[] } | null>(null);
  useEffect(() => {
    if (searchMessages === undefined || !searchesMessages(query)) return;
    let current = true;
    const timer = window.setTimeout(() => {
      searchMessages(query.trim()).then(
        hits => { if (current) setFound({ query, hits }); },
        () => { if (current) setFound({ query, hits: [] }); },
      );
    }, MESSAGE_SEARCH_DELAY_MS);
    return () => { current = false; window.clearTimeout(timer); };
  }, [query, searchMessages]);
  const messageHits = found !== null && found.query === query ? found.hits : undefined;

  const model = useMemo(() => buildThreadInbox({
    sessions, view, scope, query, selectedKey, needsYou, messageHits,
  }), [messageHits, needsYou, query, scope, selectedKey, sessions, view]);

  const send = useCallback((commands: readonly ThreadCommand[]) =>
    sendThreadCommands(request, commands, onError), [onError, request]);

  // Run ends this window has seen, so a later view can tell which are new.
  const runEnds = useRef<Map<SessionKey, number | null> | null>(null);
  useEffect(() => {
    if (view === null) return;
    const { seen, visit } = threadRunEndVisit(runEnds.current, view, watchingKey, document.hasFocus());
    runEnds.current = seen;
    // Automatic, so a refusal is not the owner's to read; the dot stays.
    if (visit !== null) void request({ kind: "visit", key: visit }).catch(() => undefined);
  }, [request, view, watchingKey]);

  return useMemo(() => ({ model, setScope, send, unarchive: onUnarchive, shortcuts }),
    [model, onUnarchive, send, setScope, shortcuts]);
}
