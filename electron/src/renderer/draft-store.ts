/**
 * Composer drafts, one per session key, outside React state.
 *
 * A conversation's composer reads its draft through `subscribe`/`get` (React's
 * useSyncExternalStore), so a keystroke re-renders that composer and nothing
 * else: not the App shell, not the tile, not the transcript beside it. The
 * store outlives every pane, which is what lets a half-written prompt survive
 * a pane switch, a workspace switch or a remount, and every pane showing the
 * same session reads and writes the same entry.
 *
 * `onWrite` hears every `set`, including one that writes the value already
 * held — the owner publishes each write exactly as it did when drafts were App
 * state. `delete` forgets a closed session's draft without reporting a write.
 */
export interface DraftStore {
  get(key: string): string;
  set(key: string, text: string): void;
  delete(key: string): void;
  subscribe(key: string, listener: () => void): () => void;
}

export function createDraftStore(options: {
  initial?: Readonly<Record<string, string>>;
  onWrite?: (key: string, text: string) => void;
} = {}): DraftStore {
  const drafts = new Map<string, string>();
  for (const [key, text] of Object.entries(options.initial ?? {})) {
    if (text !== "") drafts.set(key, text);
  }
  const listeners = new Map<string, Set<() => void>>();
  const notify = (key: string): void => {
    const waiting = listeners.get(key);
    if (waiting === undefined) return;
    for (const listener of [...waiting]) listener();
  };
  const store: DraftStore = {
    get: key => drafts.get(key) ?? "",
    set: (key, text) => {
      const changed = store.get(key) !== text;
      if (text === "") drafts.delete(key);
      else drafts.set(key, text);
      if (changed) notify(key);
      options.onWrite?.(key, text);
    },
    delete: key => {
      if (!drafts.delete(key)) return;
      notify(key);
    },
    subscribe: (key, listener) => {
      let waiting = listeners.get(key);
      if (waiting === undefined) {
        waiting = new Set();
        listeners.set(key, waiting);
      }
      waiting.add(listener);
      return () => {
        const current = listeners.get(key);
        if (current === undefined) return;
        current.delete(listener);
        if (current.size === 0) listeners.delete(key);
      };
    },
  };
  return store;
}

/**
 * The revision for one session's next advisory collaboration draft state.
 *
 * The host keeps each source session's last accepted revision for its whole
 * lifetime and refuses any update that does not exceed it. The renderer's
 * counter lives for one document and is forgotten when the session leaves the
 * listing, so a reload, an archive-then-restore or a reused key would restart
 * it below the host's mark and every write would be refused as stale until it
 * caught up. Seeding from the wall clock keeps a fresh counter above any value
 * an earlier document produced, and `previous + 1` keeps it strictly
 * increasing within one document however fast the writes come.
 */
export function nextDraftRevision(previous: number | undefined, now: number): number {
  return Math.max((previous ?? 0) + 1, Math.floor(now));
}

/**
 * The engines whose sessions can be a collaboration source: the host's
 * `CollaborationEngineId::ALL` (crates/interface-collaboration/src/state/mod.rs,
 * held equal by draft-store.test.ts). The host refuses a draft state for any
 * other engine's session, so publishing one is a round trip that can only fail.
 */
export const COLLABORATION_SOURCE_ENGINES: readonly string[] = ["claude", "codex", "acp:grok"];

/** Whether `key` (`<engine>#<session id>`) names a possible collaboration source. */
export function isCollaborationSourceKey(key: string): boolean {
  const split = key.indexOf("#");
  return split > 0 && COLLABORATION_SOURCE_ENGINES.includes(key.slice(0, split));
}

/**
 * Whether a composer write has to reach the host's collaboration draft state.
 * The host keeps only whether the draft is empty (collaboration waits for an
 * empty composer), so a non-empty write after a non-empty publish tells it
 * nothing. An empty write always goes: after a send or a failed collaboration
 * the host holds "unknown", and only an empty publish answers it. The publish
 * after every send (the outbound lane's `publishDraft`) does not come through
 * here; it always goes.
 */
export function draftWriteNeedsPublish(lastPublishedEmpty: boolean | undefined,
    text: string): boolean {
  return text.length === 0 || lastPublishedEmpty !== false;
}
