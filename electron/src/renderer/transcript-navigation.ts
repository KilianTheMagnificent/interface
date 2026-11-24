import type { ConvItem } from "./conversation-reducer";
import type { TranscriptRow } from "./transcript-merge";

/**
 * Moving through a transcript: the truncation notice, the jump-to-latest
 * capsule's backlog, the prompt anchor and the paced streaming reveal.
 */

/**
 * What the pane says above a shortened transcript, or null for nothing to say.
 *
 * `olderMessages` is the engine's own count (HistoryPage.olderMessages, which
 * is `messages.length - keep`), and it is the whole point of this function: on
 * the 1353-message session "Older messages are not shown." tells the user that
 * something is missing while withholding the one fact that makes it actionable,
 * which is that ~850 messages are. A count of 0 means nothing was dropped, and
 * outranks `truncated` — it is the more specific claim.
 *
 * `truncated` is the older boolean, kept for a caller that knows only THAT the
 * window was bounded. It says less, so it is only consulted when there is no
 * count at all.
 */
export function truncationNotice(olderMessages?: number, truncated?: boolean): string | null {
  if (olderMessages !== undefined && Number.isFinite(olderMessages)) {
    if (olderMessages <= 0) return null;
    const n = Math.floor(olderMessages);
    return n === 1
      ? "1 older message is not shown."
      : `${String(n)} older messages are not shown.`;
  }
  return truncated === true ? "Older messages are not shown." : null;
}

/**
 * The scroll behaviour "Jump to latest" should use.
 *
 * theme.css's reduced-motion guard zeroes CSS animation and transition
 * durations, but no stylesheet can reach a JS `scrollTo` — so the one animation
 * in the app that is driven from script has to read the preference itself, or a
 * thousand-message transcript animates past the reader who asked it not to.
 * `match` is the caller's `matchMedia`, which keeps the decision (including the
 * query it is made from) pure and testable without a DOM.
 */
export function jumpScrollBehavior(
  match?: (query: string) => { matches: boolean },
): ScrollBehavior {
  if (match === undefined) return "smooth";
  try {
    return match("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
  } catch {
    // An environment that has matchMedia but refuses this query tells us
    // nothing about the preference; the default stands.
    return "smooth";
  }
}

/** What the jump-to-latest capsule is standing in front of. */
export type JumpLatestBacklog = {
  /** DRAWN lines carrying something that landed after the reader paused. */
  readonly count: number;
  /** An approval the bridge can still answer is somewhere in this transcript. */
  readonly approvalWaiting: boolean;
};

/** Above this the capsule stops counting and says so — see jumpLatestLabel. */
export const JUMP_LATEST_COUNT_CAP = 99;

const NO_BACKLOG: JumpLatestBacklog = { count: 0, approvalWaiting: false };
const NO_ANSWERABLE: ReadonlySet<string> = new Set<string>();

/**
 * PURE. What has arrived behind a reader who scrolled up.
 *
 * `anchor` is the key of the last row that existed at the moment auto-follow
 * was paused, so everything AFTER it is what the reader has not seen. A null
 * anchor means they are still at the latest and nothing is behind them.
 *
 * TWO VIEWS OF THE SAME TRANSCRIPT GO IN, AND THEY ARE NOT INTERCHANGEABLE.
 *
 * `drawn` is the display list — one entry per LINE the pane paints, holding
 * every transcript key that line now stands for (see displayRowKeys). The count
 * is taken over it because the count is a claim about the screen: at balanced
 * density twelve Bash calls behind the reader resolve to ONE folded line, and
 * "12 new" over one line is the capsule contradicting the transcript beside it.
 * A line counts when it holds anything positioned after the anchor — including
 * the anchor's OWN line, which is the case where the fold grew to swallow it.
 * Every step of the dial folds something, so only the rendered list is right at
 * all of them.
 *
 * `rows` is the transcript itself, and it is the approval scan's input for the
 * mirror-image reason: that reading must survive any fold, any grouping, and a
 * display list that has lost track of a key entirely.
 *
 * EVERY drawn line counts, not only prose. A message-only count reads "0 new"
 * through a long tool-heavy stretch — which is the exact moment a paused reader
 * most wants to know the pane below them is still filling up. The seam mark is
 * not a row anybody wrote, so it is skipped.
 *
 * THE APPROVAL SCAN IS DELIBERATELY NOT ANCHORED. A pending approval blocks the
 * agent until somebody answers it, so this reports one wherever it sits rather
 * than only below the anchor: an anchor row can legitimately vanish (an echo
 * the history read supplied, a session switch), and a lost anchor must never be
 * able to swallow the one signal whose absence leaves a run stalled with nobody
 * watching. Over-reporting costs one word on a capsule the reader is already
 * looking at; under-reporting costs the turn.
 *
 * `answerable` is the caller's live approval ids — empty for a detached or
 * replayed transcript, and already stripped of every id that has a verdict — so
 * a history full of old approvals raises nothing. `resolved` catches the same
 * thing from the item's own side.
 */
export function jumpLatestBacklog(
  rows: readonly TranscriptRow[],
  drawn: readonly (readonly string[])[],
  anchor: string | null,
  answerable: ReadonlySet<string> = NO_ANSWERABLE,
): JumpLatestBacklog {
  if (anchor === null) return NO_BACKLOG;
  // Item rows only, so the seam never earns a position and can never be the
  // thing a drawn line is counted for.
  const positions = new Map<string, number>();
  let anchorAt = -1;
  let approvalWaiting = false;
  for (const row of rows) {
    if (row.kind !== "item") continue;
    if (row.key === anchor) anchorAt = positions.size;
    positions.set(row.key, positions.size);
    if (row.item.kind === "approval" && row.item.resolved === undefined
      && answerable.has(row.item.id)) approvalWaiting = true;
  }
  if (anchorAt < 0) return { count: 0, approvalWaiting };
  let count = 0;
  for (const keys of drawn) {
    for (const key of keys) {
      const at = positions.get(key);
      if (at !== undefined && at > anchorAt) { count += 1; break; }
    }
  }
  return { count, approvalWaiting };
}

/**
 * PURE. The capsule's two lengths and its accessible name.
 *
 * `text` is what a normal pane draws and `short` is what a narrow one swaps in.
 * The label SHORTENS rather than wrapping, because a capsule that grows a
 * second line covers the transcript it is meant to sit beside — and the number
 * is the half worth keeping, so the word is what goes.
 *
 * The button carries `aria` as its accessible name, so neither visible form has
 * to read as a sentence on its own.
 */
export function jumpLatestLabel(backlog: JumpLatestBacklog): {
  readonly text: string; readonly short: string; readonly aria: string;
} {
  if (backlog.approvalWaiting) {
    return { text: "Approval waiting", short: "Approval",
      aria: "An approval is waiting below. Jump to latest." };
  }
  const total = Math.max(0, Math.floor(backlog.count));
  if (total === 0) return { text: "Latest", short: "Latest", aria: "Jump to latest" };
  const shown = total > JUMP_LATEST_COUNT_CAP
    ? `${String(JUMP_LATEST_COUNT_CAP)}+` : String(total);
  return { text: `${shown} new`, short: shown,
    aria: `${shown} new below. Jump to latest.` };
}

/* ==========================================================================
   WHICH QUESTION THE VISIBLE REGION IS THE ANSWER TO.

   Long turns push the prompt that started them off the top of the pane, and
   what is left on screen is an answer with its question missing. These three
   are the whole rule for the bar that names it again: what the prompts ARE,
   which one governs what the reader can see, and where that one sits in the
   run. All pure — the pane measures the DOM and hands the numbers in.
   ========================================================================== */

/** A user turn, in transcript order, as the anchor bar names it. */
export interface PromptAnchorRow {
  readonly key: string;
  /** The prompt on ONE line: the bar is one line and the tooltip is one string. */
  readonly text: string;
}

/**
 * PURE. Every user turn in the merged transcript, in order.
 *
 * Whitespace is collapsed here rather than in CSS, because the same string is
 * both the drawn line (where `white-space:nowrap` would flatten it) and the
 * `title` tooltip (where the browser would keep the raw newlines) — a pasted
 * three-paragraph prompt has to read the same way in both.
 */
export function transcriptPrompts(rows: readonly TranscriptRow[]): PromptAnchorRow[] {
  const prompts: PromptAnchorRow[] = [];
  for (const row of rows) {
    if (row.kind !== "item" || row.item.kind !== "user") continue;
    prompts.push({ key: row.key, text: promptLine(row.item) });
  }
  return prompts;
}

/** Per item, because the rows are rebuilt on every streamed frame and prompts are not. */
const PROMPT_LINES = new WeakMap<ConvItem, string>();
function promptLine(item: Extract<ConvItem, { kind: "user" }>): string {
  const known = PROMPT_LINES.get(item);
  if (known !== undefined) return known;
  const line = item.text.replace(/\s+/gu, " ").trim();
  PROMPT_LINES.set(item, line);
  return line;
}

/** Where one prompt sits, measured from the TOP OF THE SCROLLER'S VISIBLE REGION. */
export interface PromptAnchorSpot {
  readonly key: string;
  /** Negative once the prompt has scrolled off the top; past the height is below. */
  readonly top: number;
}

/**
 * PURE. Which prompt governs what is on screen, or null for no bar at all.
 *
 * VIEWPORT COORDINATES, NOT CONTENT COORDINATES. `top` is already relative to
 * the visible region, so this needs no scrollTop and no content height — the
 * caller reads one rect for the scroller and one per prompt, and the decision
 * is the same arithmetic in a test as it is on screen.
 *
 * The governing prompt is the LAST one at or above the top edge: everything
 * under it, down to the next prompt, is its answer. Two states draw nothing:
 *
 * - THE NEWEST PROMPT IS ALREADY ON SCREEN. The bar exists to name a prompt the
 *   reader cannot see; repeating one they are looking at spends a line of
 *   transcript saying nothing. Checked against the newest specifically, because
 *   that is the one whose region runs to the bottom of the transcript.
 * - THE VIEWPORT SITS ABOVE THE FIRST PROMPT — history notices, the seam, an
 *   empty session. Nothing governs there, and inventing the first prompt would
 *   be a claim about text the reader has not reached.
 */
export function promptAnchorKey(
  spots: readonly PromptAnchorSpot[],
  viewportHeight: number,
): string | null {
  const newest = spots[spots.length - 1];
  if (newest === undefined) return null;
  if (newest.top >= 0 && newest.top < viewportHeight) return null;
  let governing: string | null = null;
  for (const spot of spots) {
    if (spot.top <= 0) governing = spot.key;
  }
  return governing;
}

/** The bar's whole content: what it says and where both steppers go. */
export interface PromptAnchorView {
  readonly key: string;
  readonly text: string;
  /** 1-based, the way the bar prints it ("3/7"). */
  readonly ordinal: number;
  readonly total: number;
  /** null at the ends: the steppers stop rather than wrapping round. */
  readonly prevKey: string | null;
  readonly nextKey: string | null;
}

/**
 * PURE. What the bar draws for an anchored key, or null when it draws nothing.
 *
 * Split from promptAnchorKey because the two answer different questions at
 * different moments: the key comes from a scroll event and is held as state,
 * while the ordinal, the total and the steps are facts about the transcript
 * THIS render — a prompt arriving renumbers the total under a key that has not
 * moved. A key this transcript no longer holds (a session switch, an echo the
 * history read superseded) draws nothing rather than a stale line.
 */
export function promptAnchorView(
  prompts: readonly PromptAnchorRow[],
  key: string | null,
): PromptAnchorView | null {
  if (key === null) return null;
  const at = prompts.findIndex(prompt => prompt.key === key);
  const prompt = prompts[at];
  if (prompt === undefined) return null;
  return {
    key: prompt.key, text: prompt.text, ordinal: at + 1, total: prompts.length,
    prevKey: prompts[at - 1]?.key ?? null,
    nextKey: prompts[at + 1]?.key ?? null,
  };
}

/**
 * Whether the reader has asked for less motion.
 *
 * The same defensive shape as jumpScrollBehavior, and for the same reason: a
 * reveal paced from script is invisible to theme.css's reduced-motion guard, so
 * it has to read the preference itself. An environment that cannot answer is
 * not a reader who objected — the default is to animate.
 */
export function prefersReducedMotion(
  match?: (query: string) => { matches: boolean },
): boolean {
  if (match === undefined) return false;
  try {
    return match("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

/**
 * Past this length a message is painted as it arrives. A paced frame costs the
 * tail block only (Markdown resumes its parse and memoizes settled blocks, about
 * 0.02 ms a frame at 32,000 characters), but slicing and comparing the whole
 * string each frame still grows with its length.
 */
export const REVEAL_SNAP_CHARS = 32_000;

/** One arrival this large is a resume flood or a pasted wall, not typing. */
export const REVEAL_FLOOD_CHARS = 1200;
/** Arrivals closer than this are one burst split across commits. */
export const REVEAL_BURST_MS = 50;
/** The paint horizon before a second burst shows the provider's rhythm. */
const FIRST_HORIZON_MS = 150;
/** The longest a burst is spread over: past it the reader waits on the provider. */
const MAX_HORIZON_MS = 1500;
/** Spread each burst a little past the usual gap, so an early next one finds text still moving. */
const HORIZON_SLACK = 1.25;
/** Once the message is complete, what is left lands within this long. */
const FINISH_MS = 400;
/** Characters per millisecond: a tiny backlog still moves at a readable clip. */
const MIN_SPEED = 0.03;

/**
 * How a streaming tail is painted: `shown` of its `length` characters,
 * advancing `speed` characters a millisecond since `clock`. `burstAt` is when
 * the latest burst began and `gap` the smoothed time between bursts, null
 * until a second burst measures one. Times are `performance.now()` ms.
 */
export type RevealPace = {
  shown: number;
  length: number;
  speed: number;
  clock: number;
  burstAt: number | null;
  gap: number | null;
};

/** A tail painted in full: what a settled message, or a reveal that stops pacing, starts from. */
export function settledPace(length: number, now = 0): RevealPace {
  return { shown: length, length, speed: 0, clock: now, burstAt: null, gap: null };
}

/**
 * Where a text block's reveal starts when it first appears. It is painted
 * whole: a pane opened mid-stream, or a block remounted, never replays what
 * was already read. A block that appears STREAMING is that arrival, so it
 * starts the burst clock and the next burst measures the provider's gap.
 */
export function mountedPace(length: number, now: number, streaming: boolean): RevealPace {
  return { ...settledPace(length, now), burstAt: streaming ? now : null };
}

/**
 * The tail is now `length` long at `now`, still `streaming` or not.
 *
 * Providers do not stream at reading speed, and not evenly. Opus 5.5 through
 * Claude Code sends a dozen deltas in the same millisecond about once a
 * second; Haiku sends one every 25 ms. Painting each arrival whole makes the
 * provider's burst the reader's chunk. Instead each arrival sets a SPEED: the
 * backlog spread over the gap the provider has been leaving between bursts,
 * so a burst is still flowing when the next one lands and a steady trickle is
 * painted a frame or two behind. Bursts closer than REVEAL_BURST_MS are one
 * burst, so a burst split across two commits does not shrink the gap.
 *
 * When the message completes, what is left finishes within FINISH_MS rather
 * than snapping, so the last burst flows like the others.
 *
 * Snaps, the honesty clauses: one arrival of REVEAL_FLOOD_CHARS or more (a
 * resume flood, a replayed transcript, a pasted wall is not being typed at
 * anybody); a tail replaced by something shorter (it snaps rather than
 * rewinding); and a message past REVEAL_SNAP_CHARS, for cost.
 */
export function paceArrival(pace: RevealPace, length: number, now: number,
    streaming: boolean): RevealPace {
  if (length < pace.shown || length - pace.length >= REVEAL_FLOOD_CHARS
      || length >= REVEAL_SNAP_CHARS) {
    return { ...settledPace(length, now), burstAt: pace.burstAt, gap: pace.gap };
  }
  const idle = pace.shown >= pace.length;
  let { burstAt, gap } = pace;
  if (length > pace.length && (burstAt === null || now - burstAt >= REVEAL_BURST_MS)) {
    if (burstAt !== null) gap = gap === null ? now - burstAt : gap * 0.6 + (now - burstAt) * 0.4;
    burstAt = now;
  }
  const horizon = streaming
    ? Math.min(MAX_HORIZON_MS, gap === null ? FIRST_HORIZON_MS : gap * HORIZON_SLACK)
    : FINISH_MS;
  const backlog = length - pace.shown;
  const speed = Math.max(MIN_SPEED, backlog / horizon, streaming ? 0 : pace.speed);
  return { shown: pace.shown, length, speed, clock: idle ? now : pace.clock, burstAt, gap };
}

/** One painted frame at `now`. A long-stalled frame counts as a short one, so it never leaps. */
export function paceFrame(pace: RevealPace, now: number): RevealPace {
  const elapsed = Math.min(100, Math.max(0, now - pace.clock));
  return { ...pace, shown: Math.min(pace.length, pace.shown + pace.speed * elapsed), clock: now };
}
