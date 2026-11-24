// The workspace's error notices (owner, 2026-11-15): small cards in the
// bottom-right corner of the main area instead of a full-width bar across its
// top. Each says what happened in plain words, offers Try again where a retry
// exists and Dismiss always; several stack newest first, three at most with
// "N more". The decisions live here; `components/CornerNotices.tsx` draws them.

/** One thing a provider/session refresh could not do. `line` is the exact text
 *  the refresh error has always carried ("codex: Interface could not refresh
 *  this provider's session list."), which Settings › Models & providers still
 *  quotes; the notice says it in plain words instead. */
export type RefreshFailure =
  | {
    kind: "provider";
    engine: string;
    /** The host still holds an older listing (`status !== "fresh"`). */
    stale: boolean;
    line: string;
  }
  | { kind: "listing" | "health" | "capability"; line: string };

/** The refresh error as one line, the shape Settings and the first-listing
 *  latch read: null when nothing failed. */
export function refreshErrorText(failures: readonly RefreshFailure[]): string | null {
  return failures.length === 0 ? null : failures.map(failure => failure.line).join(" ");
}

/** One refresh failure in plain words: the provider's display name and a
 *  sentence ("Codex could not refresh its sessions."). */
export function refreshFailureSentence(
  failure: RefreshFailure,
  providerName: (engine: string) => string,
  product: string,
): string {
  switch (failure.kind) {
    case "provider":
      return failure.stale
        ? `${providerName(failure.engine)} could not refresh its sessions.`
        : `${providerName(failure.engine)} could not list all of its sessions.`;
    case "listing": return `${product} could not refresh the session list.`;
    case "health": return `${product} could not check the providers.`;
    case "capability": return `${product} could not read what each provider supports.`;
  }
}

const failureIds = new WeakMap<RefreshFailure, number>();
let nextFailureId = 0;
/** A notice id for one failure that stays put while its neighbours are
 *  dismissed, and is new for the same words from a later refresh. */
export function refreshFailureId(failure: RefreshFailure): string {
  let id = failureIds.get(failure);
  if (id === undefined) { id = nextFailureId++; failureIds.set(failure, id); }
  return `refresh:${id}`;
}

/** The raw text behind a refresh failure, kept as the notice's tooltip: the
 *  part after the "<label>: " prefix. */
export function refreshFailureDetail(failure: RefreshFailure): string {
  const colon = failure.line.indexOf(": ");
  return colon < 0 ? failure.line : failure.line.slice(colon + 2);
}

/** How many notices show before the rest fold into "N more". */
export const CORNER_NOTICE_LIMIT = 3;

/**
 * Newest first. `seen` remembers when each notice id first appeared (an
 * increasing number); ids that left are forgotten, so one that comes back is
 * new again. Ids that arrive together keep the order they were given in.
 * Returns the ids in display order and the memory for the next call.
 */
export function orderNewestFirst(
  ids: readonly string[],
  seen: ReadonlyMap<string, number>,
  next: number,
): { order: string[]; seen: Map<string, number>; next: number } {
  const memory = new Map<string, number>();
  let arrived = false;
  for (const id of ids) {
    const at = seen.get(id);
    if (at !== undefined) memory.set(id, at);
    else { memory.set(id, next); arrived = true; }
  }
  const position = new Map(ids.map((id, index) => [id, index]));
  const order = [...new Set(ids)].sort((a, b) =>
    (memory.get(b)! - memory.get(a)!) || (position.get(a)! - position.get(b)!));
  return { order, seen: memory, next: arrived ? next + 1 : next };
}

/** The notices drawn and those folded into "N more" (still mounted, so their
 *  own countdown keeps running). Expanded shows every one. */
export function foldCornerNotices<T>(
  newestFirst: readonly T[],
  expanded: boolean,
): { shown: T[]; folded: T[] } {
  if (expanded || newestFirst.length <= CORNER_NOTICE_LIMIT) return { shown: [...newestFirst], folded: [] };
  return {
    shown: newestFirst.slice(0, CORNER_NOTICE_LIMIT),
    folded: newestFirst.slice(CORNER_NOTICE_LIMIT),
  };
}

export interface CornerRect { top: number; bottom: number; left: number; right: number }

/** The gap kept from the main area's edges and from anything the stack rises
 *  above. */
export const CORNER_NOTICE_INSET = 20;

/**
 * How far above the main area's bottom edge the stack sits so it never covers
 * a composer, its send button or another bottom-right overlay: the tallest of
 * those whose columns meet the stack's, plus the gap. `stackWidth` is the
 * stack's own width; it hugs the right edge.
 */
export function cornerLift(
  area: CornerRect,
  stackWidth: number,
  obstacles: readonly CornerRect[],
  inset = CORNER_NOTICE_INSET,
): number {
  const right = area.right - inset;
  const left = right - stackWidth;
  let lift = inset;
  for (const box of obstacles) {
    if (box.right <= left || box.left >= right) continue;
    if (box.bottom <= area.top || box.top >= area.bottom) continue;
    lift = Math.max(lift, area.bottom - box.top + inset / 2);
  }
  // Never pushed off the top of the area.
  return Math.min(lift, Math.max(inset, area.bottom - area.top - inset));
}
