import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ThreadCommand, ThreadControlResponse, ThreadsView, ThreadView } from "../shared/threads";
import type { ThreadDiffLoad } from "./components/ThreadDiffPanel";
import { ipcMessage } from "./ipc-message";
import { threadDiffCommand, type ThreadDiffChoice } from "./thread-diff";
import { retainThreadIndex } from "./thread-pane";
import { LatestRead } from "./latest-read";

/** Session key → thread, each unchanged thread keeping its object across pushes. */
export function useThreadIndex(view: ThreadsView | null): ReadonlyMap<string, ThreadView> {
  const previous = useRef<ReadonlyMap<string, ThreadView>>(new Map());
  return useMemo(() => {
    previous.current = retainThreadIndex(previous.current, view);
    return previous.current;
  }, [view]);
}

/**
 * What ⌘D and a pane's Diff toggle do for `key`: close the panel showing it,
 * show the panel that has it but is hidden behind a subagent detail (never
 * close what the owner cannot see), or open it.
 */
export function threadDiffToggleAction(openKey: string | null, key: string,
    hiddenByDetail: boolean): "close" | "show" | "open" {
  if (openKey !== key) return "open";
  return hiddenByDetail ? "show" : "close";
}

/** The Diff toggle in `key`'s pane head, when that pane is drawn. */
export function threadDiffToggleOf(key: string): HTMLButtonElement | null {
  const tile = Array.from(document.querySelectorAll<HTMLElement>(".workspace-agent[data-agent-key]"))
    .find(node => node.dataset.agentKey === key);
  return tile?.querySelector<HTMLButtonElement>("[data-thread-diff-toggle]") ?? null;
}

/** Focus `key`'s Diff toggle once the panel that held focus has gone. */
export function focusThreadDiffToggle(key: string): void {
  requestAnimationFrame(() => { threadDiffToggleOf(key)?.focus(); });
}

export interface ThreadDiffTarget {
  key: string;
  choice: ThreadDiffChoice;
  /** The panel takes focus when it opens (a keyboard activation). */
  focus: boolean;
}

const LOADING: ThreadDiffLoad = { status: "loading" };

/**
 * The thread diff panel's state: which thread and scope it shows, and Rust's
 * answer for them. One read at a time; rapid selections replace the pending
 * read, and a superseded answer is never painted. `signal` is what changes in
 * the open thread that should re-read its diff (a turn ending, a new turn).
 */
export function useThreadDiff(request: (command: ThreadCommand) => Promise<ThreadControlResponse>,
    signalFor: (key: string) => string) {
  const [target, setTarget] = useState<ThreadDiffTarget | null>(null);
  const [load, setLoad] = useState<ThreadDiffLoad>(LOADING);
  const [reads] = useState(() => new LatestRead<ThreadControlResponse>());
  const requestRef = useRef(request);
  requestRef.current = request;

  const read = useCallback((key: string, choice: ThreadDiffChoice) => {
    setLoad(LOADING);
    reads.request(() => requestRef.current(threadDiffCommand(key, choice)), answer => {
      setLoad(answer.diff === undefined
        ? { status: "error", message: "No changes were returned for this scope." }
        : { status: "ready", diff: answer.diff });
    }, (err: unknown) => {
      setLoad({ status: "error", message: ipcMessage(err) });
    });
  }, [reads]);

  const open = useCallback((key: string, choice: ThreadDiffChoice, focus = false) => {
    setTarget({ key, choice, focus });
  }, []);
  const close = useCallback(() => {
    reads.clear();
    setTarget(null);
  }, [reads]);
  const choose = useCallback((choice: ThreadDiffChoice) => {
    setTarget(current => current === null ? null : { ...current, choice, focus: false });
  }, []);
  const refresh = useCallback(() => {
    setTarget(current => current === null ? null : { ...current });
  }, []);

  const key = target?.key ?? null;
  const signal = key === null ? "" : signalFor(key);
  useEffect(() => {
    if (target === null) return;
    read(target.key, target.choice);
    // The target object changes on open, scope and refresh; the signal on a
    // turn ending in the open thread.
    return () => { reads.clear(); };
  }, [read, reads, target, signal]);

  return { target, load, open, close, choose, refresh };
}
