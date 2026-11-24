import { useCallback, useEffect, useMemo, useState } from "react";
import type { IfaceApi } from "../shared/ipc";
import type { ThreadCommand, ThreadControlResponse, ThreadsView } from "../shared/threads";
import { equalValue } from "./stable-values";
import { LatestRead } from "./latest-read";

export type ThreadsApi = Pick<IfaceApi, "threadControl" | "onThreads">;

/**
 * Keeps `next` unless `current` is already as new: Rust bumps the revision on
 * every committed change, so an older answer or push that lands late never
 * replaces a newer view, and an equal one keeps its identity and renders nothing.
 * A newer view whose hidden sessions did not change keeps the held list, so
 * Chat's filter over it is not rebuilt on every push.
 */
export function newerThreadsView(current: ThreadsView | null, next: ThreadsView): ThreadsView | null {
  if (current === null) return next;
  if (next.revision < current.revision) return current;
  if (equalValue({ ...current, now: 0 }, { ...next, now: 0 })) return current;
  return current.hiddenKeys !== undefined && equalValue(current.hiddenKeys, next.hiddenKeys)
    ? { ...next, hiddenKeys: current.hiddenKeys } : next;
}

/**
 * The Rust-owned thread state (`iface:threadControl` and the `iface:threads`
 * push). Every answer and push goes through `newerThreadsView`; `request`
 * resolves with the full answer so callers can read its optional fields
 * (prepared workspace, diff, new key) and rejects with the host's message.
 * The renderer renders this view and sends commands; it never derives a
 * section, unread state, order or permission itself.
 */
export function useThreads(api: ThreadsApi) {
  const [view, setView] = useState<ThreadsView | null>(null);
  const accept = useCallback((next: ThreadsView) => {
    setView(current => newerThreadsView(current, next));
  }, []);

  const request = useCallback(async (command: ThreadCommand): Promise<ThreadControlResponse> => {
    const answer = await api.threadControl(command);
    accept(answer.view);
    return answer;
  }, [accept, api]);

  useEffect(() => {
    const reads = new LatestRead<ThreadControlResponse>();
    const unsubscribe = api.onThreads(accept);
    const refresh = (): void => {
      reads.request(() => api.threadControl({ kind: "read" }), answer => { accept(answer.view); },
        () => undefined);
    };
    refresh();
    window.addEventListener("focus", refresh);
    return () => {
      reads.clear();
      unsubscribe();
      window.removeEventListener("focus", refresh);
    };
  }, [accept, api]);

  return useMemo(() => ({ view, request }), [view, request]);
}
