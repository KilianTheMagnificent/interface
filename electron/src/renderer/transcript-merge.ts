import type { PromptTurnImage } from "../shared/prompt-images";
import type { PromptTurnAttachment } from "./prompt-context";
import { liveItemOrdinal, type ConvItem } from "./conversation-reducer";

/**
 * The rendered transcript's one order: history, the live stream and this
 * window's own echoes (`PendingEcho`), merged by `mergeTranscript`.
 */

/* ==========================================================================
   The rendered transcript: two engine-fed lists plus this window's own echoes,
   in one order. Pure, and deliberately here rather than in the component — it
   decides what is on screen, so it is the part that has to be tested.
   ========================================================================== */

/**
 * A message THIS window sent, held locally because no engine will ever send it
 * back: `user.message` exists only in a replayed transcript (engine.ts:147). It
 * is the live half of the user's side of the conversation, and without it the
 * same sentence is visible after a reload but not at the moment it is typed.
 */
export interface PendingEcho {
  id: string;
  text: string;
  /**
   * The count of user turns with this exact text that were ALREADY visible when
   * the message was sent. It is what makes the drop rule in mergeTranscript
   * safe: sending "run the tests" a second time must not be swallowed by the
   * "run the tests" already sitting in the history.
   */
  seen: number;
  /**
   * WHERE IT GOES (`echoPosition`): one past the creation ordinal of the last
   * LIVE item that existed when this was queued — so it renders after those
   * and BEFORE everything the agent said in reply. Appending echoes to the end
   * of the live list instead (which is what this field replaced) slid every
   * question the user asked below the answer to it the moment the agent
   * started replying, permanently: after four exchanges the pane read as four
   * replies followed by four detached questions.
   *
   * An ordinal rather than a count because the live list is not append-only:
   * compaction drops its head, and a count then pointed further down the list
   * each time, sliding the echo toward the end. An item's ordinal survives
   * compaction (`liveItemOrdinal`). For items built by hand ordinals are their
   * indices, so this is the count it used to be. Negative infinity is "before
   * every live item", the compaction notice included.
   */
  at: number;
  /**
   * True when the past transcript had already been read and counted into
   * `seen`. FALSE while that read is in flight or after it failed — `seen` is
   * then a count of the live list alone, and the drop rule must not consult
   * history for this echo at all. Otherwise sending "continue" to a session
   * whose history is still loading deletes the message the moment the read
   * lands, because a "continue" from last week satisfies a threshold that was
   * frozen against an uncounted transcript.
   *
   * The cost of not consulting history is one duplicate row in the narrow case
   * where that single read (one per session per app run — Conversation's
   * history contract §3) captured the message that was just sent. A message
   * shown twice is a smaller lie than a message the user sent and cannot see.
   */
  historyKnown: boolean;
  /**
   * Thumbnails for the images attached to this message, so the transcript
   * shows what was sent the moment it is sent.
   *
   * These are still the ECHO's own thumbnails and Interface persists none of
   * them: keeping its own copy of every screenshot ever attached is a store
   * nothing here is asking for. What survives a reload instead is whatever the
   * ENGINE's store kept, replayed as `user.message.images`, which is why the
   * same picture can come back without this app having saved anything.
   *
   * The two do not always agree, and that asymmetry is real. Claude keeps a
   * pasted image inline in its session JSONL, so it comes back; Codex records
   * an attached path as `localImage`, so it comes back as `[image: <path>]` and
   * this thumbnail is the only time that message ever had a picture.
   */
  images?: readonly PromptTurnImage[];
  /**
   * File and folder references attached to this message when sent, so clicking
   * an attachment in the transcript can open it in the output preview panel.
   */
  attachments?: readonly PromptTurnAttachment[];
  /**
   * The send was REFUSED — `iface.send` rejected, so these words never left the
   * app. Already a sentence for a human. A failed echo is never dropped by the
   * rule below (the transcript can never supply a message that was not sent, so
   * only an unrelated identical sentence could cancel it, taking the failure
   * notice with it) and it renders with that notice underneath, because a user
   * turn that looks delivered over a message the engine rejected is worse than
   * the silence this echo replaced.
   */
  error?: string;
}

/**
 * How many top-level user turns in `items` carry exactly this text. Trimmed on
 * both sides: the echo holds the trimmed draft, and an engine may store the
 * same prompt with a trailing newline. Nested subagent children are not
 * counted — a user turn inside a subagent is the parent agent's prompt to it,
 * not something the human typed.
 */
const isUserTurnOf = (item: ConvItem, want: string): boolean =>
  item.kind === "user" && item.text.trim() === want;

export function countUserTurns(items: readonly ConvItem[], text: string): number {
  const want = text.trim();
  let n = 0;
  for (const item of items) if (isUserTurnOf(item, want)) n++;
  return n;
}

/**
 * countUserTurns for every text at once: trimmed top-level user text → count.
 * mergeTranscript builds one per list instead of walking both lists once per
 * echo on every frame.
 */
function userTurnCounts(items: readonly ConvItem[]): ReadonlyMap<string, number> {
  const counts = new Map<string, number>();
  for (const item of items) {
    if (item.kind !== "user") continue;
    const text = item.text.trim();
    counts.set(text, (counts.get(text) ?? 0) + 1);
  }
  return counts;
}

/**
 * The ROW KEY that supplied one echo, or null while nothing has. This is the
 * other side of mergeTranscript's rule 3 — the same comparison, asked for the
 * one item rather than the count — and it exists for a caller that hung
 * something on `e:<id>` and has to follow that turn when the transcript itself
 * supplies it.
 *
 * That is not a rare case: every ACP engine streams the user's own message
 * back as a live `user.message`, so a row that was `e:0` at the send is
 * `l:user-3` a second later, and anything still naming the old key is naming a
 * row that no longer exists.
 *
 * The item is the (`seen` + 1)-th top-level user turn with this echo's text,
 * counted history-then-live and skipping history entirely when the echo was
 * frozen against an uncounted one — exactly the terms rule 3 drops on. A
 * refused echo is never settled: it was never delivered, so nothing can
 * legitimately supply it.
 */
export function settledEchoRowKey(history: readonly ConvItem[], live: readonly ConvItem[],
    echo: PendingEcho): string | null {
  if (echo.error !== undefined) return null;
  const want = echo.text.trim();
  let remaining = echo.seen;
  if (echo.historyKnown) {
    for (const item of history) {
      if (!isUserTurnOf(item, want)) continue;
      if (remaining === 0) return `h:${item.id}`;
      remaining -= 1;
    }
  }
  for (const item of live) {
    if (!isUserTurnOf(item, want)) continue;
    if (remaining === 0) return `l:${item.id}`;
    remaining -= 1;
  }
  return null;
}

/**
 * The `seen` to freeze into a new echo of `text`: every copy of it that already
 * exists, whether the transcript holds it or an earlier echo is still standing
 * in for it.
 *
 * The echo term is the one that is easy to miss and is a real off-by-one. Send
 * "ok" twice in a row and neither is in the transcript yet, so counting only
 * the transcript would freeze the SAME number into both; one copy arriving from
 * a history read then satisfies `count > seen` for both and two messages
 * collapse into one. Counting the echo already queued makes the second one's
 * threshold one higher, so each send is cancelled by exactly one supply.
 *
 * `history` is whatever the caller has actually READ. When the read has not
 * landed it is empty, and the resulting `seen` counts only the live list — the
 * echo must then carry `historyKnown: false`, or mergeTranscript will compare
 * that number against a transcript it was never taken from.
 */
export function echoSeen(
  text: string,
  history: readonly ConvItem[],
  live: readonly ConvItem[],
  echoes: readonly PendingEcho[],
): number {
  const want = text.trim();
  const queued = echoes.reduce((n, echo) => (echo.text.trim() === want ? n + 1 : n), 0);
  return countUserTurns(history, text) + countUserTurns(live, text) + queued;
}

/**
 * Build the echo for a message being sent right now. It exists as a function
 * rather than an object literal in the composer because the composer cannot be
 * driven in this test environment — no DOM, so no send button to press — and
 * "the message went out but its screenshot did not come with it" is precisely
 * the kind of omission that would otherwise ship unnoticed.
 *
 * `images` is dropped when empty rather than stored as `[]`, so a plain message
 * is byte-for-byte the echo it has always been.
 */
export function newPendingEcho(fields: {
  id: string;
  text: string;
  seen: number;
  at: number;
  historyKnown: boolean;
  images?: readonly PromptTurnImage[];
  attachments?: readonly PromptTurnAttachment[];
}): PendingEcho {
  const { images, attachments, ...rest } = fields;
  return {
    ...rest,
    ...(images === undefined || images.length === 0 ? {} : { images }),
    ...(attachments === undefined || attachments.length === 0 ? {} : { attachments }),
  };
}

/**
 * The first prompt of a session started in this window. createSession never
 * emits `user.message`, and `startedHere` skips the history read that would
 * replay it later, so without this echo the launch text exists only as the
 * sidebar title.
 */
export function launchPendingEcho(
  text: string,
  images?: readonly PromptTurnImage[],
  attachments?: readonly PromptTurnAttachment[],
): PendingEcho {
  return newPendingEcho({
    id: "launch", text, seen: 0, at: Number.NEGATIVE_INFINITY, historyKnown: true, images, attachments,
  });
}

/**
 * The `at` for an echo sent now: after every live item already drawn, before
 * the first one that arrives later. An item created afterwards has a higher
 * ordinal than any existing one, and a streaming text run keeps the ordinal of
 * its first delta, so a reply that continues it stays above the echo, as it did
 * when `at` was a count.
 */
export function echoPosition(live: readonly ConvItem[]): number {
  const last = live.length - 1;
  return last < 0 ? Number.NEGATIVE_INFINITY : liveItemOrdinal(live[last], last) + 1;
}

/**
 * One rendered position in the transcript. `key` is the React key and is NOT the
 * item id: see mergeTranscript for why the three lists have to be namespaced.
 */
export type TranscriptRow =
  | { kind: "item"; key: string; origin: "history" | "live" | "echo"; item: ConvItem }
  /** A divider. Absent `label` is the live seam, which is what a mark used to be. */
  | { kind: "mark"; key: string; label?: string };

// Engine-minted ids: a tool item's id is its callId, an approval's is its
// approvalId, a subagent card's is its agentKey, and a visual's is the id
// Rust gave its page (T3 section 8: the same in the live event and in history
// read from the journal). Those four are stable identities, so the same one
// appearing in both lists is the same turn twice. Every other id (`text-3`, `user-5`, `diff-7`, `notice-9`) is
// generated from the event's position (its live-log ordinal, or its distance
// from a history page's end), so it is only unique WITHIN one list — never
// compare those across lists.
// Exported so the Agent surface's Progress panel, which merges the same two
// lists for a different purpose, dedupes by the same rule instead of a second
// copy of it that could drift.
export const isEngineId = (item: ConvItem): boolean =>
  item.kind === "tool" || item.kind === "approval" || item.kind === "subagent"
  || item.kind === "visual";

/*
 * ROW IDENTITY SURVIVES A FRAME. A row is a pure function of its item (or
 * echo) and its origin, and items are immutable and retained by identity while
 * a session streams (conversation-projection.ts), so the same item always gets
 * the same row object back. Only the row for the item that actually changed is
 * new, which is what lets the display pass and the row memos downstream skip
 * everything a streamed token did not touch.
 */
const HISTORY_ROWS = new WeakMap<ConvItem, TranscriptRow>();
const LIVE_ROWS = new WeakMap<ConvItem, TranscriptRow>();
const ECHO_ROWS = new WeakMap<PendingEcho, readonly TranscriptRow[]>();
const LIVE_SEAM_ROW: TranscriptRow = { kind: "mark", key: "mark:live" };

function itemRow(
  cache: WeakMap<ConvItem, TranscriptRow>, origin: "history" | "live", item: ConvItem,
): TranscriptRow {
  const known = cache.get(item);
  if (known !== undefined) return known;
  const row: TranscriptRow = { kind: "item",
    key: `${origin === "history" ? "h" : "l"}:${item.id}`, origin, item };
  cache.set(item, row);
  return row;
}

function echoRows(echo: PendingEcho): readonly TranscriptRow[] {
  const known = ECHO_ROWS.get(echo);
  if (known !== undefined) return known;
  const rows: TranscriptRow[] = [{ kind: "item", key: `e:${echo.id}`, origin: "echo",
    item: { kind: "user", id: `echo-${echo.id}`, text: echo.text,
      ...(echo.images === undefined || echo.images.length === 0
        ? {} : { images: echo.images }),
      ...(echo.attachments === undefined || echo.attachments.length === 0
        ? {} : { attachments: echo.attachments }) } }];
  // The one thing that keeps a refused send from looking delivered. A
  // notice, not a badge, so it reads in the transcript the same way an
  // engine error does — and it sits directly under the words it is about.
  if (echo.error !== undefined) {
    rows.push({ kind: "item", key: `e:${echo.id}:err`, origin: "echo",
      item: { kind: "notice", id: `echo-err-${echo.id}`,
        text: `Not delivered — ${echo.error}` } });
  }
  ECHO_ROWS.set(echo, rows);
  return rows;
}

/**
 * PURE. The history list, the live list and this window's un-echoed sends in,
 * the exact render order out.
 *
 * Four rules, and nothing else happens here:
 *
 *  1. ORDER IS PRESERVED. History first, in the order given; then the seam if
 *     both halves have content; then the live items, in the order given, with
 *     each echo standing where it was sent — after the live items that existed
 *     at that moment (those created before ordinal `at`), and therefore ABOVE
 *     the reply it prompted.
 *     Nothing is sorted by timestamp and nothing engine-fed is moved.
 *
 *     Echoes used to be appended after the whole live list, which is true only
 *     for the one frame between the send and the first `message.delta`: no
 *     engine emits `user.message` live, so the drop rule never fired and the
 *     live half grew forever underneath a stack of questions that never
 *     shrank. Four exchanges rendered as four answers followed by four
 *     detached questions — the "one side of a phone call" failure the user
 *     turn was added to fix, inverted, and permanent.
 *  2. LIVE WINS ON IDENTITY. A history item whose engine-minted id also appears
 *     in the live list is dropped from the history half — that is the same turn
 *     read twice (a caller that re-read the transcript after live events had
 *     started), and the live copy is the one carrying live state. Removal is
 *     only ever from history, so the live stream can never lose a row. Prose
 *     cannot be deduped this way and is not attempted: two text blocks with the
 *     same words are usually two real turns, so the contract in Conversation.tsx
 *     says do not re-read history once live events exist.
 *  3. AN ECHO YIELDS TO THE REAL RECORD — BUT ONLY TO A RECORD IT WAS COUNTED
 *     AGAINST. An echo is dropped once the transcript holds MORE user turns
 *     with its text than it held when it was sent, which happens when a history
 *     read lands after the send and includes it. Sending the same words twice
 *     on purpose is not that: `seen` was higher when the second one was sent,
 *     so it survives. An echo queued while the history read was still in flight
 *     (or after it failed) has `historyKnown: false` and history is left out of
 *     the comparison entirely — its `seen` was frozen against an uncounted
 *     transcript, so any older copy of those words would otherwise delete a
 *     message the user just sent. A REFUSED echo (`error`) is never dropped at
 *     all: it was never delivered, so nothing can legitimately supply it.
 *     Echoes are never merged into either engine list, so nothing the engine
 *     said can be lost to this rule.
 *  4. KEYS ARE NAMESPACED. `h:`/`l:`/`e:` prefixes, because the generated ids
 *     restart at the same values in each list: history's `text-0` and live's
 *     `text-0` are different blocks, and one React key for both would show one
 *     of them with the other's state. Prefixing (rather than keying by index)
 *     also keeps each row's key stable as the live list grows, so an expanded
 *     tool card stays expanded. A refused echo adds one more row under its own
 *     (`e:<id>:err`), which is how a rejected send says so on screen.
 */
export function mergeTranscript(
  history: readonly ConvItem[],
  live: readonly ConvItem[],
  echoes: readonly PendingEcho[] = [],
): TranscriptRow[] {
  const liveIds = new Set<string>();
  for (const item of live) if (isEngineId(item)) liveIds.add(item.id);

  const rows: TranscriptRow[] = [];
  for (const item of history) {
    if (isEngineId(item) && liveIds.has(item.id)) continue;
    rows.push(itemRow(HISTORY_ROWS, "history", item));
  }
  let historyTurns: ReadonlyMap<string, number> | undefined;
  let liveTurns: ReadonlyMap<string, number> | undefined;
  const kept = echoes.filter(echo => {
    if (echo.error !== undefined) return true;      // never delivered: rule 3
    const want = echo.text.trim();
    const supplied = (echo.historyKnown
      ? (historyTurns ??= userTurnCounts(history)).get(want) ?? 0 : 0)
      + ((liveTurns ??= userTurnCounts(live)).get(want) ?? 0);
    return supplied <= echo.seen;
  });
  // The seam is a row like any other, so this function fully determines what is
  // on screen: it exists only when there is something on BOTH sides of it. An
  // echo counts as something — it is a message this window sent after opening,
  // which is exactly what the seam separates from the session's past.
  if (rows.length > 0 && live.length + kept.length > 0) {
    rows.push(LIVE_SEAM_ROW);
  }

  // Sorted by position, on a copy. Echoes arrive in send order and `at` never
  // decreases, so this is a no-op on the real input — it is here so the
  // ordering rule holds by construction rather than by an assumption about the
  // caller, and Array.sort is stable, so two sends made without an intervening
  // live item keep the order they were typed. Compared, not subtracted: two
  // launch echoes at negative infinity subtract to NaN.
  const placed = kept.slice().sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
  let next = 0;
  const echoesBefore = (position: number): void => {
    while (next < placed.length && placed[next].at <= position) {
      rows.push(...echoRows(placed[next++]));
    }
  };
  for (let i = 0; i < live.length; i++) {
    echoesBefore(liveItemOrdinal(live[i], i));
    rows.push(itemRow(LIVE_ROWS, "live", live[i]));
  }
  // Everything still queued belongs after the last live item, including any
  // `at` that outran a live list the caller rebuilt shorter.
  echoesBefore(Number.POSITIVE_INFINITY);
  return rows;
}
