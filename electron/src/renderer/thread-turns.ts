import { parseProviderCommandLike } from "../shared/provider-commands";
import type { ThreadTurn } from "../shared/threads";
import type { TurnRestorePoint } from "./conversation-types";
import type { TranscriptRow } from "./transcript-merge";
import { COMPACTED_NOTICE } from "./live-events";

/**
 * Which transcript user turn is which Rust thread turn.
 *
 * Rust records one `ThreadTurn` per prompt sent through Interface, with the
 * prompt's digest: the first 16 hex characters of SHA-256 over the prompt with
 * "\r\n" folded to "\n" and then trimmed. The transcript holds the prompts as
 * the provider stored them. The two are joined by that digest alone, so a row
 * whose text was never recorded (a prompt typed in a terminal, a history page
 * that is not loaded) simply gets no turn, and therefore no control: a missing
 * control is the safe failure, a control on the wrong turn is not.
 *
 * The k-th row carrying a digest is the k-th turn carrying it (the spec's
 * rule). That rule assumes both lists start at the beginning of the thread.
 * When either does not (older history not loaded, a compacted live log, or
 * Rust's store keeping only the newest 200 turns), the lists are aligned from
 * their newest ends instead, which is where both still agree.
 */

// SHA-256 (FIPS 180-4), synchronous because the mapping runs during render.
// WebCrypto's digest is async only, and the transcript must not wait a frame
// to know which controls it draws.
const K = Uint32Array.of(
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
);

const rotr = (value: number, bits: number): number => (value >>> bits) | (value << (32 - bits));

/** Lower-case hex SHA-256 of the string's UTF-8 bytes. */
export function sha256Hex(text: string): string {
  const bytes = new TextEncoder().encode(text);
  const blocks = Math.ceil((bytes.length + 9) / 64);
  const padded = new Uint8Array(blocks * 64);
  padded.set(bytes);
  padded[bytes.length] = 0x80;
  const view = new DataView(padded.buffer);
  const bits = bytes.length * 8;
  view.setUint32(padded.length - 8, Math.floor(bits / 0x1_0000_0000));
  view.setUint32(padded.length - 4, bits >>> 0);
  const hash = Uint32Array.of(0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19);
  const w = new Uint32Array(64);
  for (let offset = 0; offset < padded.length; offset += 64) {
    for (let i = 0; i < 16; i += 1) w[i] = view.getUint32(offset + i * 4);
    for (let i = 16; i < 64; i += 1) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3);
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10);
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
    }
    let [a, b, c, d, e, f, g, h] = hash;
    for (let i = 0; i < 64; i += 1) {
      const t1 = (h + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g))
        + K[i] + w[i]) | 0;
      const t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
      h = g; g = f; f = e; e = (d + t1) | 0;
      d = c; c = b; b = a; a = (t1 + t2) | 0;
    }
    hash[0] += a; hash[1] += b; hash[2] += c; hash[3] += d;
    hash[4] += e; hash[5] += f; hash[6] += g; hash[7] += h;
  }
  return [...hash].map(word => word.toString(16).padStart(8, "0")).join("");
}

// Rust's `str::trim` (Unicode White_Space), which is what the host digests
// with. JavaScript's own trim differs in two code points: it also strips
// U+FEFF and keeps U+0085.
const RUST_WHITESPACE = "\\t\\n\\v\\f\\r \\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000";
const RUST_TRIM = new RegExp(`^[${RUST_WHITESPACE}]+|[${RUST_WHITESPACE}]+$`, "gu");

/** The prompt exactly as Rust digests it: CRLF folded to LF, then trimmed. */
export function normalizeThreadPrompt(text: string): string {
  return text.replace(/\r\n/gu, "\n").replace(RUST_TRIM, "");
}

const DIGESTS = new Map<string, string>();

/** First 16 hex of SHA-256 over the normalized prompt (`ThreadTurn.promptDigest`). */
export function threadPromptDigest(text: string): string {
  const known = DIGESTS.get(text);
  if (known !== undefined) return known;
  const digest = sha256Hex(normalizeThreadPrompt(text)).slice(0, 16);
  // Bounded: a long session re-renders the same few hundred prompts.
  if (DIGESTS.size >= 2048) DIGESTS.clear();
  DIGESTS.set(text, digest);
  return digest;
}

/** One top-level user row of the transcript, in transcript order. */
export interface ThreadPromptRow { key: string; text: string }

// What Claude Code writes into its transcript as a user message when the
// owner stops a turn, and the echo of an interactive `!` bash command. The
// history page shows them as user rows, but nobody sent them through
// Interface and the host never counts them as turns (engine-claude's
// `is_written_by_claude`), so they never take a turn control.
const INTERRUPT_MARKERS = new Set(["[Request interrupted by user]",
  "[Request interrupted by user for tool use]"]);
const BASH_ECHO_BLOCKS = [["<bash-input>", "</bash-input>"], ["<bash-stdout>", "</bash-stdout>"],
  ["<bash-stderr>", "</bash-stderr>"]] as const;

function withoutBlocks(text: string, open: string, close: string): string {
  let output = "";
  let rest = text;
  for (let start = rest.indexOf(open); start >= 0; start = rest.indexOf(open)) {
    output += rest.slice(0, start);
    const end = rest.indexOf(close, start + open.length);
    if (end < 0) return output + rest.slice(start);
    rest = rest.slice(end + close.length);
  }
  return output + rest;
}

/** A user row the provider's CLI wrote itself: a stop's marker, or a `!` bash echo. */
export function isCliWrittenPrompt(text: string): boolean {
  const trimmed = normalizeThreadPrompt(text);
  if (INTERRUPT_MARKERS.has(trimmed)) return true;
  let rest = trimmed;
  let echoed = false;
  for (const [open, close] of BASH_ECHO_BLOCKS) {
    if (!rest.includes(open)) continue;
    rest = withoutBlocks(rest, open, close);
    echoed = true;
  }
  return echoed && normalizeThreadPrompt(rest) === "";
}

/**
 * The transcript's own user turns (never a subagent's prompt, which is
 * nested, and never a row the CLI wrote itself).
 */
export function threadPromptRows(rows: readonly TranscriptRow[]): ThreadPromptRow[] {
  const prompts: ThreadPromptRow[] = [];
  for (const row of rows) {
    if (row.kind === "item" && row.item.kind === "user" && !isCliWrittenPrompt(row.item.text)) {
      prompts.push({ key: row.key, text: row.item.text });
    }
  }
  return prompts;
}

/** The quiet row Rust's history puts where a switched thread changed provider (protocol 59). */
export const SWITCHED_NOTICE_PREFIX = "Switched to ";

/**
 * A switched thread's own prompt rows (protocol 59): those after the last
 * "Switched to …" row, the stretch the thread's current session ran. Earlier
 * stretches are other sessions' turns, which draw no control here. With no
 * such row every prompt row is a candidate, as in an ordinary thread.
 */
export function headStretchPromptRows(rows: readonly TranscriptRow[]): ThreadPromptRow[] {
  let start = 0;
  rows.forEach((row, index) => {
    if (row.kind === "item" && row.item.kind === "notice"
        && row.item.text.startsWith(SWITCHED_NOTICE_PREFIX)) start = index + 1;
  });
  return threadPromptRows(rows.slice(start));
}

const COMPACTED_TEXT = COMPACTED_NOTICE.kind === "notice" ? COMPACTED_NOTICE.message : null;

/** True when the live log dropped its oldest events (it then leads with the notice). */
export function transcriptWasCompacted(rows: readonly TranscriptRow[]): boolean {
  for (const row of rows) {
    if (row.kind !== "item") continue;
    if (row.item.kind === "notice" && row.item.text === COMPACTED_TEXT) return true;
  }
  return false;
}

/**
 * Row key → the turn it is. `rowsFromStart` says the rows begin at the start
 * of the conversation; the turns do when their first ordinal is 0. Unmatched
 * rows are absent from the map.
 *
 * Turns happen in order, so matched ordinals must strictly increase down the
 * transcript. Each digest is paired on its own, and a prompt the provider
 * stored differently from what Interface recorded shifts that digest's pairs
 * against the others; the two rows of such a crossing cannot both be right,
 * and nothing says which one is. So a row whose turn is not above every
 * matched turn before it and below every one after it gets no control.
 */
export function matchThreadTurns(prompts: readonly ThreadPromptRow[],
    turns: readonly ThreadTurn[], rowsFromStart: boolean): Map<string, ThreadTurn> {
  const matched = new Map<string, ThreadTurn>();
  if (prompts.length === 0 || turns.length === 0) return matched;
  const ordered = [...turns].sort((a, b) => a.ordinal - b.ordinal);
  const turnsFromStart = ordered[0].ordinal === 0;
  const byDigest = new Map<string, ThreadTurn[]>();
  for (const turn of ordered) {
    const list = byDigest.get(turn.promptDigest);
    if (list === undefined) byDigest.set(turn.promptDigest, [turn]);
    else list.push(turn);
  }
  const rowsByDigest = new Map<string, string[]>();
  for (const prompt of prompts) {
    const digest = threadPromptDigest(prompt.text);
    if (!byDigest.has(digest)) continue;
    const list = rowsByDigest.get(digest);
    if (list === undefined) rowsByDigest.set(digest, [prompt.key]);
    else list.push(prompt.key);
  }
  const forward = rowsFromStart && turnsFromStart;
  for (const [digest, keys] of rowsByDigest) {
    const candidates = byDigest.get(digest) ?? [];
    if (forward) {
      for (let k = 0; k < keys.length && k < candidates.length; k += 1) {
        matched.set(keys[k], candidates[k]);
      }
    } else {
      for (let k = 1; k <= keys.length && k <= candidates.length; k += 1) {
        matched.set(keys[keys.length - k], candidates[candidates.length - k]);
      }
    }
  }
  return withoutCrossings(prompts, matched);
}

/** `matched` less every row whose ordinal is out of order with another matched row's. */
function withoutCrossings(prompts: readonly ThreadPromptRow[],
    matched: Map<string, ThreadTurn>): Map<string, ThreadTurn> {
  const keys = prompts.map(prompt => prompt.key).filter(key => matched.has(key));
  const ordinals = keys.map(key => matched.get(key)!.ordinal);
  const lowestAfter = new Array<number>(keys.length + 1).fill(Number.POSITIVE_INFINITY);
  for (let at = keys.length - 1; at >= 0; at -= 1) {
    lowestAfter[at] = Math.min(ordinals[at], lowestAfter[at + 1]);
  }
  let highestBefore = Number.NEGATIVE_INFINITY;
  keys.forEach((key, at) => {
    const ordinal = ordinals[at];
    if (ordinal <= highestBefore || ordinal >= lowestAfter[at + 1]) matched.delete(key);
    highestBefore = Math.max(highestBefore, ordinal);
  });
  return matched;
}

/**
 * Where each matched turn's answer ends: the last item row before the next
 * user row (or before the end). Row key → the turn that ends there. A turn
 * still running has no end yet, and one with no row after its prompt has
 * nothing to hang a control on. A row the CLI wrote itself (a stop's marker,
 * a `!` bash echo) neither ends a turn nor is one's end: the host's fork cut
 * runs to the last answer before the next genuine prompt.
 */
export function threadTurnEnds(rows: readonly TranscriptRow[],
    matched: ReadonlyMap<string, ThreadTurn>): Map<string, ThreadTurn> {
  const ends = new Map<string, ThreadTurn>();
  let open: ThreadTurn | undefined;
  let last: string | null = null;
  const close = (): void => {
    if (open !== undefined && last !== null && open.state !== "running") ends.set(last, open);
  };
  for (const row of rows) {
    if (row.kind !== "item") continue;
    if (row.item.kind === "user") {
      if (isCliWrittenPrompt(row.item.text)) continue;
      close();
      open = matched.get(row.key);
      last = null;
      continue;
    }
    last = row.key;
  }
  close();
  return ends;
}

/**
 * The other direction of `matchThreadTurns`: turn ordinal → the row key of its
 * prompt. The pane reports it to the shell, whose restore points are keyed by
 * row, so a control that knows only a turn (the diff panel's Undo) can find
 * the one restore the transcript would offer for it.
 */
export function threadTurnRowKeys(matched: ReadonlyMap<string, ThreadTurn>): Map<number, string> {
  const keys = new Map<number, string>();
  for (const [rowKey, turn] of matched) keys.set(turn.ordinal, rowKey);
  return keys;
}

/**
 * Turn `ordinal`'s restore point, through the row its pane last reported for
 * it. Either half missing (no matched row, no point on that row) is no Undo:
 * the transcript would offer none there either.
 */
export function turnRestoreForOrdinal(ordinal: number,
    rowKeys: ReadonlyMap<number, string> | undefined,
    points: ReadonlyMap<string, TurnRestorePoint> | undefined,
): { turnKey: string; point: TurnRestorePoint } | null {
  const turnKey = rowKeys?.get(ordinal);
  const point = turnKey === undefined ? undefined : points?.get(turnKey);
  return turnKey === undefined || point === undefined ? null : { turnKey, point };
}

/** The newest failed/interrupted prompt row eligible for receipt-bound Continue.
 * The callback must obtain the matching durable receipt; this join never grants
 * permission to replay the row's text. Older turns and provider commands get none. */
export function threadRetryKey(rows: readonly TranscriptRow[],
    matched: ReadonlyMap<string, ThreadTurn>, turns: readonly ThreadTurn[]): string | null {
  let newest: ThreadTurn | undefined;
  for (const turn of turns) if (newest === undefined || turn.ordinal > newest.ordinal) newest = turn;
  if (newest === undefined || newest.state !== "failed" && newest.state !== "interrupted") return null;
  for (const [rowKey, turn] of matched) {
    if (turn.ordinal !== newest.ordinal) continue;
    const row = rows.find(candidate => candidate.key === rowKey);
    const text = row?.kind === "item" && row.item.kind === "user" ? row.item.text : "";
    return normalizeThreadPrompt(text) !== "" && parseProviderCommandLike(text) === null ? rowKey : null;
  }
  return null;
}

/** "+41 −12 · 3 files", or the parts known, or "" when Rust measured nothing. */
export function threadTurnChangeText(turn: Pick<ThreadTurn, "added" | "removed" | "filesChanged">): string {
  const parts: string[] = [];
  if (turn.added !== undefined && turn.removed !== undefined
      && (turn.added > 0 || turn.removed > 0)) {
    parts.push(`+${turn.added} −${turn.removed}`);
  }
  if (turn.filesChanged !== undefined && turn.filesChanged > 0) {
    parts.push(`${turn.filesChanged} ${turn.filesChanged === 1 ? "file" : "files"}`);
  }
  return parts.join(" · ");
}
