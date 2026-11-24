import { providerLabel } from "./provider-labels";
import type { EngineUsage } from "../shared/engine";

/**
 * Plan usage views: windows, resets, freshness and the toolbar strip.
 */

/* ---- plan usage -----------------------------------------------------------
   One view of both subscriptions. The single hard rule, from the contract: an
   engine that could not report renders as "not reported" and NEVER as 0% — a
   bar reading zero when the truth is unknown is worse than no bar, because it
   is a number the user will act on. */

export interface UsageWindowView {
  label: string;
  /** Raw utilization, 0-100, or null when the engine did not say. */
  percent: number | null;
  /** What to print: "76%", or "not reported". Never "0%" for an unknown. */
  percentText: string;
  /**
   * The engine's own reset instant, verbatim, or null when it gave none.
   *
   * DELIBERATELY NOT a formatted countdown. This used to be a `resetText`
   * baked by `formatReset` at READ time, and a countdown is the one string that
   * cannot be baked: the panel keeps its snapshot for as long as the app is
   * open, so a read at 09:00 was still printing "resets in 1h 10m" at 14:00 —
   * counting down to an instant four hours in the past, for a window that had
   * already turned over. The caller formats this against the clock it is
   * drawing at (`formatReset(w.resetsAt, now)`), so the countdown can only ever
   * be as old as the render.
   */
  resetsAt: string | null;
}

export interface UsageEngineView {
  engine: string;
  available: boolean;
  /** The engine's own word for the plan ("max", "pro"), or "". */
  plan: string;
  /** Why it is unavailable, or a caveat worth showing. "" when there is none. */
  note: string;
  /** "$1.23", or "" when this engine reports no dollar figure (Codex never does). */
  costText: string;
  windows: UsageWindowView[];
  /** Protocol 60: the added account this row is for; absent on the Default account's row. */
  account?: string;
  /** That account's label, beside `account`. */
  accountLabel?: string;
}

/**
 * The key a usage row's headroom is filed under: the engine id for the Default
 * account's row (so a provider with one account reads exactly as before), and
 * the engine and account for an added account's row. Engine ids never hold
 * "@" and account ids are lowercase letters, digits and hyphens, so the two
 * cannot meet.
 */
export function usageAccountKey(engine: string, account?: string): string {
  return account === undefined || account === "" || account === "default" ? engine : `${engine}@${account}`;
}

/** "Claude" for the Default account's row, "Claude (Work)" for an added account's. */
export function usageRowLabel(view: Pick<UsageEngineView, "engine" | "accountLabel">,
    label?: string): string {
  const provider = providerLabel(view.engine, label);
  return view.accountLabel === undefined || view.accountLabel === ""
    ? provider : `${provider} (${view.accountLabel})`;
}

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

/**
 * "resets in 2h 5m". Relative, not a wall clock: the payloads carry UTC
 * instants (Claude's in microseconds with a +00:00 offset), and a formatted
 * local time would be one more thing to get wrong across a timezone.
 * Unparseable or absent -> "", which renders as nothing at all.
 */
export function formatReset(iso: string | null, now: number): string {
  if (iso === null || iso === "") return "";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  const ms = at - now;
  if (ms <= MINUTE_MS) return "resets now";
  if (ms < HOUR_MS) return `resets in ${Math.floor(ms / MINUTE_MS)}m`;
  if (ms < DAY_MS) {
    const h = Math.floor(ms / HOUR_MS);
    const m = Math.floor((ms % HOUR_MS) / MINUTE_MS);
    return m === 0 ? `resets in ${h}h` : `resets in ${h}h ${m}m`;
  }
  const d = Math.floor(ms / DAY_MS);
  const h = Math.floor((ms % DAY_MS) / HOUR_MS);
  return h === 0 ? `resets in ${d}d` : `resets in ${d}d ${h}h`;
}

/** How wide to paint the bar. Clamped; the TEXT still states the real number. */
export function barWidth(percent: number | null): number {
  if (percent === null || Number.isNaN(percent)) return 0;
  return Math.max(0, Math.min(100, percent));
}

/**
 * Takes no clock ON PURPOSE. Everything here is a fact the engine reported;
 * the two things that depend on when you are looking — the countdown and the
 * age of the read — are formatted at render time by `formatReset` and
 * `usageFreshness`. A view-model that bakes the clock in produces a snapshot
 * that keeps asserting itself hours later, which is the defect this signature
 * makes unrepresentable.
 */
export function buildUsageView(usages: readonly EngineUsage[]): UsageEngineView[] {
  return usages.map(u => ({
    engine: u.engine,
    available: u.available,
    plan: u.plan ?? "",
    note: u.note ?? "",
    costText: u.costUsd === undefined ? "" : `$${u.costUsd.toFixed(2)}`,
    // An unavailable engine has nothing to draw. The contract says windows is
    // empty then; this does not trust that, because an empty bar row is the
    // exact "0% when the truth is unknown" the panel must never show.
    windows: u.available
      ? u.windows.map(w => ({
        label: w.label,
        percent: w.utilization,
        percentText: w.utilization === null ? "not reported" : `${Math.round(w.utilization)}%`,
        resetsAt: w.resetsAt,
      }))
      : [],
    ...(u.account === undefined ? {} : { account: u.account }),
    ...(u.account === undefined || u.accountLabel === undefined ? {} : { accountLabel: u.accountLabel }),
  }));
}

/**
 * After this long, a usage snapshot stops being reported as a current number.
 *
 * Five minutes. The shortest vendor window is five hours, so the figure is not
 * a lie yet — but the always-on strip is what people glance at while they work,
 * and an hour between reads left a morning number standing through a whole
 * afternoon of turns. Stale no longer blanks the chips (they keep the last
 * figure as "~33%"), so a five-minute cutoff no longer empties the strip the
 * way the first five-minute attempt did.
 *
 * It is still not a 1 Hz poll: a read spawns the real vendor CLI (1.2 s warm,
 * 4.8 s cold, measured). The triggers are Refresh, opening the panel on a
 * stale snapshot, App.tsx's catch-up when the strip crosses this line, and a
 * coalesced re-read when a turn ends — at most one clocked spawn per five
 * minutes, plus one after work that actually spent quota.
 */
export const USAGE_STALE_MS = 5 * MINUTE_MS;

/** When the panel's numbers were read, and whether they may still be asserted. */
export interface UsageFreshness {
  /** "read just now", "read 12m ago", "read 5h ago". "" before any read. */
  text: string;
  /** True when the snapshot is too old to state as a current number. */
  stale: boolean;
}

/**
 * How old the snapshot is, in the words the panel prints.
 *
 * `readAt === null` is "never read", which is stale by definition — there is
 * nothing to assert. A `readAt` in the future (a clock that moved under us)
 * clamps to zero rather than printing a negative age.
 */
export function usageFreshness(readAt: number | null, now: number): UsageFreshness {
  if (readAt === null) return { text: "", stale: true };
  const age = Math.max(0, now - readAt);
  const stale = age >= USAGE_STALE_MS;
  if (age < MINUTE_MS) return { text: "read just now", stale };
  if (age < HOUR_MS) return { text: `read ${Math.floor(age / MINUTE_MS)}m ago`, stale };
  if (age < DAY_MS) return { text: `read ${Math.floor(age / HOUR_MS)}h ago`, stale };
  return { text: `read ${Math.floor(age / DAY_MS)}d ago`, stale };
}

/**
 * The one number for the status bar: the fullest window across every engine
 * that reported one. "" when nothing is known — the item then reads "Usage" and
 * still opens the panel, rather than showing a zero nobody measured.
 */
export function usageEnginePercent(view: UsageEngineView | undefined): number | null {
  if (view === undefined || !view.available) return null;
  let worst: number | null = null;
  for (const w of view.windows) {
    if (w.percent === null) continue;
    if (worst === null || w.percent > worst) worst = w.percent;
  }
  return worst;
}

export type UsageStripTone = "ok" | "high" | "full" | "unknown";

export function usageStripTone(percent: number | null): UsageStripTone {
  if (percent === null) return "unknown";
  if (percent >= 100) return "full";
  if (percent >= 80) return "high";
  return "ok";
}

export interface UsageEngineRef {
  id: string;
  label?: string;
}

export interface UsageStripItem {
  engine: string;
  label: string;
  percentText: string;
  tone: UsageStripTone;
}

/**
 * Compact chips for the always-visible usage menu. One row per advertised
 * provider, even before the first read, so the three engines stay on screen.
 *
 * A stale snapshot keeps its LAST-KNOWN figure and prefixes it with "~" rather
 * than blanking. The strip is the only always-on budget signal in the app, and
 * dropping the number turned it into three provider words — the audit measured
 * no percentages at all in five of eight captured frames. "~87%" on an hours-
 * long window is honest where a silent blank was merely safe, and the exact
 * age is one hover away in the trigger's title, which already spells out "read
 * 12m ago" and offers the re-read.
 *
 * `percent === null` still yields "" and always will: that is the provider
 * declining to report, and inventing a tilde for a number nobody measured is
 * the one thing worse than showing nothing.
 */
export function usageStripItems(
    views: readonly UsageEngineView[],
    readAt: number | null,
    now: number,
    engines: readonly UsageEngineRef[] = []): UsageStripItem[] {
  // One chip per provider: its Default account's row. An added account's row
  // is in the panel under its own label.
  const byEngine = new Map(views.filter(view => view.account === undefined)
    .map(view => [view.engine, view]));
  const refs: UsageEngineRef[] = engines.length > 0
    ? [...engines]
    : [...byEngine.keys()].map(id => ({ id }));
  for (const view of views) {
    if (!refs.some(ref => ref.id === view.engine)) refs.push({ id: view.engine });
  }
  const stale = usageFreshness(readAt, now).stale;
  return refs.map(ref => {
    const percent = usageEnginePercent(byEngine.get(ref.id));
    return {
      engine: ref.id,
      label: providerLabel(ref.id, ref.label),
      percentText: percent === null
        ? ""
        : `${stale ? "~" : ""}${Math.round(percent)}%`,
      tone: usageStripTone(percent),
    };
  });
}

export function usageStripSummary(items: readonly UsageStripItem[]): string {
  return items.map(item => item.percentText === ""
    ? item.label : `${item.label} ${item.percentText}`).join(" · ");
}

/** The window-rail usage readout: the fullest window anywhere. */
export interface UsageRailReading {
  readonly engine: string;
  /** "Claude", from the engine's own label. */
  readonly provider: string;
  /** The window, shortened for the rail: "7d", "5h", or its own word. */
  readonly window: string;
  /** "62%", or "~62%" once the snapshot is stale. */
  readonly text: string;
  /** 0-100 for the little bar. */
  readonly percent: number;
  readonly tone: UsageStripTone;
}

function railWindowWord(label: string): string {
  if (/week|7[\s-]*d(?:ay)?/iu.test(label)) return "7d";
  const hours = /(\d+)\s*-?\s*h(?:ou)?r/iu.exec(label);
  if (hours !== null) return `${hours[1]}h`;
  return label.trim().toLocaleLowerCase();
}

/**
 * The one number the window rail shows (owner, 2026-10-01): the fullest
 * window across every engine that reported a figure, from the snapshot the
 * app already holds. Null when nothing was measured — the rail then shows no
 * readout rather than a zero, and Usage stays in the ··· menu.
 */
export function usageRailReading(views: readonly UsageEngineView[],
    readAt: number | null, now: number): UsageRailReading | null {
  let best: { view: UsageEngineView; window: UsageWindowView; percent: number } | null = null;
  for (const view of views) {
    if (!view.available) continue;
    for (const window of view.windows) {
      if (window.percent === null) continue;
      if (best === null || window.percent > best.percent) best = { view, window, percent: window.percent };
    }
  }
  if (best === null) return null;
  const stale = usageFreshness(readAt, now).stale;
  return {
    engine: best.view.engine,
    provider: usageRowLabel(best.view),
    window: railWindowWord(best.window.label),
    text: `${stale ? "~" : ""}${Math.round(best.percent).toString()}%`,
    percent: Math.max(0, Math.min(100, best.percent)),
    tone: usageStripTone(best.percent),
  };
}

/**
 * A provider's remaining plan headroom, for the places a model or provider
 * is chosen (review E, owner 2026-10-14: the composer's model chip and the
 * launcher's provider tiles). Quiet by design: "38% left" from the fullest
 * window the provider reported, "~38% left" once the read is stale, and the
 * window, its use and its reset in `title`. Null when nothing was measured —
 * never a "100% left" for a number nobody read.
 */
export interface UsageHeadroom {
  readonly engine: string;
  /** "38% left", "~38% left" when stale, or "At limit". */
  readonly text: string;
  /** "Claude plan: 62% of the 5-hour window used, resets in 2h". */
  readonly title: string;
  readonly tone: UsageStripTone;
}

export function usageHeadroom(view: UsageEngineView | undefined, readAt: number | null,
    now: number): UsageHeadroom | null {
  if (view === undefined || !view.available) return null;
  let fullest: UsageWindowView | null = null;
  for (const window of view.windows) {
    if (window.percent === null) continue;
    if (fullest === null || window.percent > (fullest.percent ?? 0)) fullest = window;
  }
  if (fullest === null || fullest.percent === null) return null;
  const used = Math.max(0, Math.min(100, Math.round(fullest.percent)));
  const stale = usageFreshness(readAt, now).stale;
  const left = 100 - used;
  const text = left <= 0 ? "At limit" : `${stale ? "~" : ""}${left}% left`;
  const reset = formatReset(fullest.resetsAt, now);
  const freshness = usageFreshness(readAt, now).text;
  const title = `${usageRowLabel(view)} plan: ${used}% of the ${fullest.label.trim()} window used`
    + (reset === "" ? "" : `, ${reset}`) + (freshness === "" ? "" : ` (${freshness})`);
  return { engine: view.engine, text, title, tone: usageStripTone(fullest.percent) };
}

/**
 * Every provider's headroom, by engine id for its Default account's row and by
 * `usageAccountKey` for an added account's; rows with no figure are absent.
 */
export function usageHeadrooms(views: readonly UsageEngineView[], readAt: number | null,
    now: number): Readonly<Record<string, UsageHeadroom>> {
  const out: Record<string, UsageHeadroom> = {};
  for (const view of views) {
    const headroom = usageHeadroom(view, readAt, now);
    if (headroom !== null) out[usageAccountKey(view.engine, view.account)] = headroom;
  }
  return out;
}

/**
 * A thread's own headroom: its account's row (protocol 60). A thread on the
 * Default account (`account` absent) reads the row without an account.
 */
export function threadUsageHeadroom(headrooms: Readonly<Record<string, UsageHeadroom>>,
    engine: string, account: string | undefined): UsageHeadroom | undefined {
  return headrooms[usageAccountKey(engine, account)];
}
