import { useCallback, useEffect, useMemo, useState } from "react";
import type { SessionKey, SessionKind, SessionSummary } from "../shared/engine";
import type { ThreadsView, ThreadView } from "../shared/threads";
import type { AwayProvider } from "./away-view";
import type { EngineOption } from "./composer-model";
import { threadProjectRoot } from "./thread-inbox";

/** What Away's panel reads from App. */
export interface AwayControllerOptions {
  surface: SessionKind;
  surfaceSession: SessionSummary | null;
  surfaceSessionKey: SessionKey | null;
  threads: { readonly view: ThreadsView | null };
  threadIndex: ReadonlyMap<string, ThreadView>;
  engines: readonly EngineOption[];
  sessionByKey: ReadonlyMap<SessionKey, SessionSummary>;
  showSurface: (kind: SessionKind) => void;
  selectSession: (key: SessionKey) => void;
}

/**
 * Project Away as App drives its panel: whether it is open, the palette's
 * open request, the project it starts in, the provider check, and which
 * loop thread to open once the session list has it. Moved out of App as it
 * was. The session lists Away reports (and the hidden keys the sidebar leaves
 * out) stay in App beside the other hidden sessions, as do the quit guard's
 * count and an Away thread's cards, which drive the thread diff panel.
 */
export function useAwayController({
  surface, surfaceSession, surfaceSessionKey, threads, threadIndex, engines,
  sessionByKey, showSurface, selectSession,
}: AwayControllerOptions) {
  const [awayOpen, setAwayOpen] = useState(false);
  // Bumped by the palette's "Away loop…": the panel opens without starting a loop.
  const [awayOpenRequest, setAwayOpenRequest] = useState(0);
  // The project Away starts in from Threads or Agent: the focused session's.
  // A thread's own record wins (a worktree thread runs inside the worktree);
  // a "No project" thread, or a task outside any repository (an agent's home
  // folder), has none, and Away then asks for a folder.
  const awayFocusedProject = surfaceSession === null || surfaceSessionKey === null ? null
    : surface === "chat" ? threadProjectRoot(surfaceSession, threadIndex.get(surfaceSessionKey))
    : surfaceSession.projectRoot || null;
  const awayProviderReady = useCallback((provider: AwayProvider): boolean =>
    engines.some(engine => engine.id === provider && engine.unavailable === undefined), [engines]);
  // A loop's thread opens on Threads once the session list has it (a loop's
  // thread is filed a moment before its registry row arrives).
  const [awayThreadToOpen, setAwayThreadToOpen] = useState<SessionKey | null>(null);
  const openAwayThread = useCallback((key: string) => { setAwayThreadToOpen(key); }, []);
  // Each loop's thread by run, as Rust's Threads view names it.
  const awayThreadKeys = useMemo(() => new Map((threads.view?.threads ?? [])
    .flatMap(thread => thread.away === undefined ? [] : [[thread.away.runId, thread.key] as const])),
  [threads.view]);
  useEffect(() => {
    if (awayThreadToOpen === null || !sessionByKey.has(awayThreadToOpen)) return;
    setAwayThreadToOpen(null);
    showSurface("chat");
    selectSession(awayThreadToOpen);
  }, [awayThreadToOpen, sessionByKey, selectSession, showSurface]);

  return {
    awayOpen, setAwayOpen, setAwayOpenRequest, awayOpenRequest,
    awayFocusedProject, awayProviderReady, openAwayThread, awayThreadKeys,
  };
}
