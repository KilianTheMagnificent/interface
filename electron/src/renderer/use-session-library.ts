import { useCallback, useRef, useState, type RefObject } from "react";
import { sessionKey, type SessionKey } from "../shared/engine";
import { parseKey, type IfaceApi } from "../shared/ipc";
import type {
  ArchivedSessionView, ManagedWorktreeView, SessionLibraryTab, SessionSearchHitView,
} from "./components/SessionLibrary";
import { ipcMessage } from "./ipc-message";

type LibraryApi = Pick<IfaceApi,
  | "listArchivedSessions" | "listManagedWorktrees" | "searchSessions" | "archive"
  | "openManagedWorktree" | "openTerminal" | "cleanupManagedWorktree">;

/**
 * The Session Library dialog: transcript search, archived sessions and managed
 * worktrees, plus the actions its rows offer. Every read and mutation goes to
 * Rust; this keeps the dialog's own state and the request fences that drop a
 * stale answer.
 */
export function useSessionLibrary({
  api, refreshAll, selectSession, closeQuickOpen, dismissComposer, launchingRef, pendingTileFocus,
}: {
  api: LibraryApi;
  refreshAll: () => Promise<void>;
  selectSession: (key: SessionKey) => void;
  closeQuickOpen: () => void;
  dismissComposer: (focusBack: boolean) => void;
  launchingRef: RefObject<boolean>;
  pendingTileFocus: RefObject<SessionKey | null>;
}) {
  const [sessionLibraryOpen, setSessionLibraryOpen] = useState(false);
  const [sessionLibraryTab, setSessionLibraryTab] = useState<SessionLibraryTab>("search");
  const [sessionSearchQuery, setSessionSearchQuery] = useState("");
  const [sessionSearchHits, setSessionSearchHits] = useState<SessionSearchHitView[]>([]);
  const [sessionSearchLoading, setSessionSearchLoading] = useState(false);
  const [sessionSearchError, setSessionSearchError] = useState<string | null>(null);
  const [sessionSearchIncomplete, setSessionSearchIncomplete] = useState<string[]>([]);
  const [archivedSessions, setArchivedSessions] = useState<ArchivedSessionView[]>([]);
  const [archivedSessionsLoading, setArchivedSessionsLoading] = useState(false);
  const [managedWorktrees, setManagedWorktrees] = useState<ManagedWorktreeView[]>([]);
  const [managedWorktreesLoading, setManagedWorktreesLoading] = useState(false);
  const [sessionLibraryNotice, setSessionLibraryNotice] = useState<{
    message: string; error?: boolean;
  } | null>(null);
  const sessionLibraryRefreshRequest = useRef(0);
  const sessionSearchRequest = useRef(0);

  const refreshSessionLibrary = useCallback(async (): Promise<void> => {
    const request = sessionLibraryRefreshRequest.current + 1;
    sessionLibraryRefreshRequest.current = request;
    setArchivedSessionsLoading(true);
    setManagedWorktreesLoading(true);
    setSessionLibraryNotice(null);
    const [archives, worktrees] = await Promise.allSettled([
      api.listArchivedSessions(),
      api.listManagedWorktrees(),
    ]);
    if (sessionLibraryRefreshRequest.current !== request) return;
    const failures: string[] = [];
    if (archives.status === "fulfilled") {
      setArchivedSessions(archives.value.map(summary => ({
        key: sessionKey(summary.engine, summary.sessionId),
        engine: summary.engine,
        title: summary.title,
        cwd: summary.cwd,
        lastActivity: summary.lastActivity,
      })));
    } else {
      failures.push(`Archived sessions: ${ipcMessage(archives.reason)}`);
    }
    if (worktrees.status === "fulfilled") {
      // `dirtySource` records what the source checkout looked like at launch;
      // it is not the current worktree status. Leave `dirty` unknown here and
      // let cleanup perform its authoritative, immediate Git recheck.
      setManagedWorktrees(worktrees.value.map(item => ({
        key: item.key,
        path: item.path,
        sourceCwd: item.sourceCwd,
        branch: item.branch,
        state: item.state,
      })));
    } else {
      failures.push(`Worktrees: ${ipcMessage(worktrees.reason)}`);
    }
    setArchivedSessionsLoading(false);
    setManagedWorktreesLoading(false);
    if (failures.length > 0) {
      setSessionLibraryNotice({ message: failures.join(" "), error: true });
    }
  }, [api]);

  const runSessionSearch = useCallback((raw: string): void => {
    const query = raw.trim();
    if (query === "") return;
    const request = sessionSearchRequest.current + 1;
    sessionSearchRequest.current = request;
    setSessionSearchLoading(true);
    setSessionSearchError(null);
    setSessionSearchIncomplete([]);
    api.searchSessions(query).then(response => {
      if (sessionSearchRequest.current !== request) return;
      setSessionSearchHits(response.hits.map(hit => ({
        key: hit.key,
        engine: parseKey(hit.key).engine,
        title: hit.title,
        cwd: hit.cwd,
        excerpts: hit.matches.map(match => match.preview),
      })));
      const incomplete = response.providers
        .filter(provider => !provider.complete)
        .map(provider => `${provider.engine}: ${provider.error ?? "stored history is incomplete"}`);
      if (response.truncated) incomplete.push("A bounded search limit was reached.");
      setSessionSearchIncomplete(incomplete);
    }, (err: unknown) => {
      if (sessionSearchRequest.current !== request) return;
      setSessionSearchHits([]);
      setSessionSearchError(ipcMessage(err));
    }).finally(() => {
      if (sessionSearchRequest.current === request) setSessionSearchLoading(false);
    });
  }, [api]);

  // The Library's own Search button. Takes no argument on purpose: it is wired
  // straight to a click handler, which would otherwise hand it a MouseEvent.
  const searchSessionLibrary = useCallback((): void => {
    runSessionSearch(sessionSearchQuery);
  }, [runSessionSearch, sessionSearchQuery]);

  const openSessionLibrary = useCallback((tab: SessionLibraryTab = "search"): void => {
    if (launchingRef.current) return;
    dismissComposer(false);
    closeQuickOpen();
    setSessionLibraryTab(tab);
    setSessionLibraryOpen(true);
    void refreshSessionLibrary();
  }, [closeQuickOpen, dismissComposer, launchingRef, refreshSessionLibrary]);

  /**
   * The sidebar's filter found nothing, so its query goes on to the search that
   * reads further: stored transcripts, host-side. Prefilled AND run, because
   * the user already typed it once.
   */
  const searchTranscriptsFor = useCallback((query: string): void => {
    setSessionSearchQuery(query);
    openSessionLibrary("search");
    runSessionSearch(query);
  }, [openSessionLibrary, runSessionSearch]);

  const restoreSessionFromLibrary = useCallback((key: SessionKey): void => {
    setSessionLibraryNotice(null);
    void api.archive(key, false)
      .then(async () => {
        await Promise.all([refreshAll(), refreshSessionLibrary()]);
        setSessionLibraryNotice({ message: "Session restored to Projects." });
      })
      .catch((err: unknown) => {
        setSessionLibraryNotice({ message: ipcMessage(err), error: true });
      });
  }, [api, refreshAll, refreshSessionLibrary]);

  const openSessionFromLibrary = useCallback((key: SessionKey): void => {
    setSessionLibraryNotice(null);
    void api.archive(key, false)
      .then(() => refreshAll())
      .then(() => {
        pendingTileFocus.current = key;
        setSessionLibraryOpen(false);
        selectSession(key);
      })
      .catch((err: unknown) => {
        setSessionLibraryNotice({ message: ipcMessage(err), error: true });
      });
  }, [api, pendingTileFocus, refreshAll, selectSession]);

  const openWorktreeFromLibrary = useCallback((key: SessionKey): void => {
    setSessionLibraryNotice(null);
    void api.openManagedWorktree(key)
      .then(async () => {
        await api.archive(key, false);
        await refreshAll();
        setSessionLibraryOpen(false);
        selectSession(key);
      })
      .catch((err: unknown) => {
        setSessionLibraryNotice({ message: ipcMessage(err), error: true });
      });
  }, [api, refreshAll, selectSession]);

  const openTerminalForWorktree = useCallback((key: SessionKey): void => {
    setSessionLibraryNotice(null);
    void api.openManagedWorktree(key)
      .then(worktree => api.openTerminal(worktree.path))
      .then(result => {
        setSessionLibraryNotice({ message: `Opened ${result.root} in ${result.app}.` });
      }, (err: unknown) => {
        setSessionLibraryNotice({ message: ipcMessage(err), error: true });
      });
  }, [api]);

  const cleanupWorktreeFromLibrary = useCallback((key: SessionKey): void => {
    setSessionLibraryNotice(null);
    void api.cleanupManagedWorktree(key).then(async result => {
      if (result.removed) {
        await Promise.all([refreshAll(), refreshSessionLibrary()]);
        setSessionLibraryNotice({ message: "Removed the clean managed worktree." });
        return;
      }
      const changes = result.changes === undefined
        ? ""
        : ` (${result.changes} uncommitted change${result.changes === 1 ? "" : "s"})`;
      await refreshSessionLibrary();
      setSessionLibraryNotice({
        message: `${result.detail ?? "The managed worktree was kept."}${changes}`,
        error: result.kept === "failed",
      });
    }, (err: unknown) => {
      setSessionLibraryNotice({ message: ipcMessage(err), error: true });
    });
  }, [api, refreshAll, refreshSessionLibrary]);

  // Closing drops any answer still in flight: the fences advance so a late
  // response is ignored, and the loading flags go quiet with the dialog.
  const closeSessionLibrary = useCallback((): void => {
    sessionLibraryRefreshRequest.current += 1;
    sessionSearchRequest.current += 1;
    setArchivedSessionsLoading(false);
    setManagedWorktreesLoading(false);
    setSessionSearchLoading(false);
    setSessionLibraryOpen(false);
  }, []);

  return {
    sessionLibraryOpen, sessionLibraryTab, setSessionLibraryTab,
    sessionSearchQuery, setSessionSearchQuery, sessionSearchHits, sessionSearchLoading,
    sessionSearchError, sessionSearchIncomplete, archivedSessions, archivedSessionsLoading,
    managedWorktrees, managedWorktreesLoading, sessionLibraryNotice, setSessionLibraryNotice,
    refreshSessionLibrary, searchSessionLibrary, openSessionLibrary, searchTranscriptsFor,
    restoreSessionFromLibrary, openSessionFromLibrary, openWorktreeFromLibrary,
    openTerminalForWorktree, cleanupWorktreeFromLibrary, closeSessionLibrary,
  };
}
