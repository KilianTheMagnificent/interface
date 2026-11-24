import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { sessionKey, type EngineEvent, type SessionKey, type SessionSummary } from "../shared/engine";
import { withoutKey } from "./helpers";
import {
  RECAP_AUTO_DISMISS_MS, recapLine, recapSnapshot,
  type RecapChecks, type RecapSnapshot,
} from "./recap";
import { pendingApproval } from "./session-activity";
import type { WorkspaceState } from "./workspace";

// Shared empty fallback, as App keeps its own.
const NO_EVENTS: EngineEvent[] = [];

/** What the recap reads from App: the focused pane and the logs, through refs. */
export interface ReturnRecapOptions {
  workspace: Pick<WorkspaceState, "focusedKey">;
  focusedKeyRef: RefObject<SessionKey | null>;
  eventsRef: RefObject<Readonly<Record<SessionKey, EngineEvent[]>>>;
  sessionsRef: RefObject<readonly SessionSummary[]>;
}

/**
 * The return recap's attention tracking, lines, timers and dismissal, moved
 * out of App as they were. App calls it where the block stood, so its two
 * effects keep their place among App's, and a deleted session's teardown
 * clears the stores it returns.
 */
export function useReturnRecap({
  workspace, focusedKeyRef, eventsRef, sessionsRef,
}: ReturnRecapOptions) {
  /* ---- the return recap (see ./recap) -------------------------------------
     A pane has the user's attention when it is the focused tile AND this window
     has the OS focus. Both halves matter: switching to another app leaves every
     pane, and switching tiles leaves exactly one. Attention arriving somewhere
     takes a snapshot of where it left; attention returning subtracts the two.

     All of it rides on refs. A window that re-rendered the whole shell on every
     focus and blur would pay for this line far more often than it is drawn —
     the only state here is the handful of lines currently on screen. */
  const [recaps, setRecaps] = useState<Record<SessionKey, string>>({});
  const recapAwayRef = useRef<Record<SessionKey, { at: number; snapshot: RecapSnapshot }>>({});
  const recapAttendedRef = useRef<SessionKey | null>(null);
  const recapTimersRef = useRef<Record<SessionKey, number>>({});
  // The last project check run that FINISHED, with the repository it ran over.
  // Checks are repository-scoped and sessions are not, so a run is attributed
  // to a pane only when it ran over that pane's own repository and finished
  // inside the absence being recapped. Anything else is someone else's result.
  const recapChecksRef = useRef<{ root: string; at: number; checks: RecapChecks } | null>(null);

  const dismissRecap = useCallback((key: SessionKey): void => {
    const timer = recapTimersRef.current[key];
    if (timer !== undefined) {
      window.clearTimeout(timer);
      recapTimersRef.current = withoutKey(recapTimersRef.current, key);
    }
    setRecaps(current => key in current ? withoutKey(current, key) : current);
  }, []);

  const syncRecapAttention = useCallback((): void => {
    const attended = document.hasFocus() ? focusedKeyRef.current : null;
    const previous = recapAttendedRef.current;
    if (attended === previous) return;
    recapAttendedRef.current = attended;
    const now = Date.now();
    if (previous !== null) {
      recapAwayRef.current[previous] = {
        at: now, snapshot: recapSnapshot(eventsRef.current[previous] ?? NO_EVENTS),
      };
    }
    if (attended === null) return;
    const absence = recapAwayRef.current[attended];
    // Consumed whether or not it yields a line: one absence is one recap, so
    // glancing away and back cannot redraw the briefing just dismissed.
    recapAwayRef.current = withoutKey(recapAwayRef.current, attended);
    const summary = sessionsRef.current.find(
      session => sessionKey(session.engine, session.sessionId) === attended);
    if (absence === undefined || summary === undefined) return;
    const log = eventsRef.current[attended] ?? NO_EVENTS;
    const run = recapChecksRef.current;
    const root = summary.projectRoot ?? summary.cwd;
    const line = recapLine({
      before: absence.snapshot,
      after: recapSnapshot(log),
      leftAt: absence.at,
      returnedAt: now,
      working: summary.status === "working",
      approvalWaiting: pendingApproval(log) !== undefined,
      checks: run !== null && run.root === root && run.at > absence.at ? run.checks : null,
    });
    if (line === null) return;
    setRecaps(current => ({ ...current, [attended]: line.text }));
    recapTimersRef.current[attended] = window.setTimeout(() => {
      recapTimersRef.current = withoutKey(recapTimersRef.current, attended);
      setRecaps(current => withoutKey(current, attended));
    }, RECAP_AUTO_DISMISS_MS);
  }, []);

  useEffect(() => {
    const sync = (): void => { syncRecapAttention(); };
    window.addEventListener("focus", sync);
    window.addEventListener("blur", sync);
    const timers = recapTimersRef;
    return () => {
      window.removeEventListener("focus", sync);
      window.removeEventListener("blur", sync);
      for (const timer of Object.values(timers.current)) window.clearTimeout(timer);
    };
  }, [syncRecapAttention]);

  useEffect(() => { syncRecapAttention(); }, [syncRecapAttention, workspace.focusedKey]);

  return { recaps, setRecaps, recapAwayRef, recapTimersRef, dismissRecap };
}
