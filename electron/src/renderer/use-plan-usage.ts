import { useCallback, useEffect, useMemo, useState } from "react";
import type { IfaceApi } from "../shared/ipc";
import type { UsageMenuState } from "./components/UsageMenu";
import { ipcMessage } from "./ipc-message";
import { PlanUsageReads } from "./plan-usage-reads";
export { PlanUsageReads, USAGE_TURN_FLOOR_MS, type PlanUsageReadsEnvironment } from "./plan-usage-reads";
import { buildUsageView, usageFreshness } from "./usage-view";

const USAGE_IDLE: UsageMenuState = { status: "idle", views: [], error: null, readAt: null };

/**
 * How often the clock behind the usage panel advances, once a snapshot exists.
 *
 * A minute, because a minute is the finest thing either text says ("read 12m
 * ago", "resets in 1h 10m"). It costs one render of an app that already
 * re-renders on every engine event. It is still not a poll: the vendor CLI runs
 * on Refresh, on opening a stale panel, on the ONE tick that carries a
 * snapshot past USAGE_STALE_MS — five minutes apart at most from the clock —
 * and after a turn that just spent quota, at most once a minute
 * (USAGE_TURN_FLOOR_MS). Without this clock the age, the countdown and that
 * crossing all freeze on screen whenever the app is quiet, which is exactly
 * when they are most wrong.
 */
const USAGE_TICK_MS = 60_000;

type Visibility = Pick<Document, "hidden" | "addEventListener" | "removeEventListener">;

/** Whether the minute clock runs: a snapshot to keep honest, and a window to show it in. */
export function usageClockActive(hidden: boolean, readAt: number | null): boolean {
  return readAt !== null && !hidden;
}

/**
 * What the always-visible strip needs now: nothing, the first read after a
 * short pause, or a read at once (an opened menu with no snapshot, or the
 * stale catch-up). A hidden window reads nothing: its chips are not on
 * screen, and showing it again advances the clock, which asks once.
 */
export function usageCatchUp(state: { status: UsageMenuState["status"]; stale: boolean;
    open: boolean; hidden: boolean }): "none" | "soon" | "now" {
  if (state.status === "loading" || state.status === "error") return "none";
  if (state.status === "ready" && !state.stale) return "none";
  if (state.hidden) return "none";
  return state.status === "idle" && !state.open ? "soon" : "now";
}

/**
 * Plan usage. A probe costs a real CLI spawn (1.2s warm, 4.8s cold). The
 * first read fills the chips; later reads fire on Refresh, when a snapshot
 * crosses USAGE_STALE_MS, and after a turn has just spent quota (floored at
 * USAGE_TURN_FLOOR_MS). None of them fire while the window is hidden, which on
 * macOS is most of the day for an app closed to the Dock.
 */
export function usePlanUsage(api: Pick<IfaceApi, "readUsage">, visibility: Visibility = document) {
  const [usage, setUsage] = useState<UsageMenuState>(USAGE_IDLE);
  const [usageOpen, setUsageOpen] = useState(false);
  const [hidden, setHidden] = useState(() => visibility.hidden);

  // The clock the panel and badge are drawn against. A successful read resets
  // it immediately; the minute timer below only keeps an existing snapshot
  // honest while it remains on screen.
  const [now, setNow] = useState(() => Date.now());

  const reads = useMemo(() => new PlanUsageReads({
    hidden: () => visibility.hidden,
    read: () => api.readUsage(),
    loading: () => { setUsage(current => ({ ...current, status: "loading", error: null })); },
    // `buildUsageView` takes no clock any more — the countdown and the age are
    // formatted against the clock of whatever render draws them, which is what
    // stops a snapshot asserting itself hours later.
    landed: (rows, readAt) => {
      setNow(readAt);
      setUsage({ status: "ready", views: buildUsageView(rows), error: null, readAt });
    },
    // readUsage is specified never to reject — an engine that cannot answer
    // returns available:false with a note — but the IPC call itself still
    // can, and a usage panel is not worth a white screen.
    //
    // The failed read leaves NO snapshot behind: readAt goes back to null,
    // which is what makes the badge stop printing the number the previous
    // read produced. A refusal must not leave an older figure standing.
    failed: err => { setUsage({ status: "error", views: [], error: ipcMessage(err), readAt: null }); },
  }), [api, visibility]);
  useEffect(() => {
    reads.activate();
    return () => { reads.dispose(); };
  }, [reads]);
  // Back on screen: the clock catches up (one stale read if it crossed
  // USAGE_STALE_MS while hidden) and a turn that ended meanwhile is read.
  useEffect(() => {
    const changed = (): void => {
      setHidden(visibility.hidden);
      if (visibility.hidden) return;
      setNow(Date.now());
      reads.shown();
    };
    visibility.addEventListener("visibilitychange", changed);
    return () => { visibility.removeEventListener("visibilitychange", changed); };
  }, [reads, visibility]);
  const refreshUsage = useCallback(() => { reads.refresh(); }, [reads]);

  // Whether the always-visible strip is currently drawing a stale snapshot,
  // measured against `now` — the same clock the chips themselves are drawn
  // against, which is what lets the effect below fire on the tick that crosses
  // USAGE_STALE_MS. The old guard called Date.now() inside the effect, so no
  // dependency could ever change at that moment and the crossing was invisible.
  const usageStale = usageFreshness(usage.readAt, now).stale;

  // An idle snapshot fills the chips after a short pause so the first
  // session/engine probes are not competing with three usage CLIs. Opening
  // the menu, or Refresh, still reads immediately.
  //
  // The catch-up read (M1): once a snapshot goes stale, ONE read — whether or
  // not the menu is open, because the strip is on screen either way and a
  // meter nobody refreshes is the defect. It cannot become a poll: the read
  // resets readAt, `usageStale` goes false, and the effect will not re-enter
  // until USAGE_STALE_MS of wall clock has passed; a read that fails lands in
  // "error", which returns on the first line and never retries by itself.
  useEffect(() => {
    const due = usageCatchUp({ status: usage.status, stale: usageStale, open: usageOpen, hidden });
    if (due === "none") return;
    if (due === "soon") {
      const id = window.setTimeout(() => { refreshUsage(); }, 1500);
      return () => { window.clearTimeout(id); };
    }
    refreshUsage();
    return undefined;
  }, [usageOpen, usage.status, usageStale, hidden, refreshUsage]);

  // The clock the panel, the badge and the staleness line above are drawn
  // against. It advances only once a snapshot exists — before the first read
  // there is no age and no countdown to keep honest, and an app-wide timer
  // running for nothing is a cost with no reader. See USAGE_TICK_MS.
  useEffect(() => {
    if (!usageClockActive(hidden, usage.readAt)) return;
    const id = setInterval(() => { setNow(Date.now()); }, USAGE_TICK_MS);
    return () => { clearInterval(id); };
  }, [hidden, usage.readAt]);

  // Quota moves when a turn ends, not on the clock. PlanUsageReads coalesces a
  // burst of tiles into one read and floors these reads at a minute apart.
  const noteUsageTurnDone = useCallback(() => { reads.turnDone(); }, [reads]);

  return { usage, usageOpen, setUsageOpen, now, usageStale, refreshUsage, noteUsageTurnDone };
}
