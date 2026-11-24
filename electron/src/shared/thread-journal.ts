/**
 * `iface:threadJournal` (protocol 56): reads of each thread's journal, the
 * record Rust writes as a thread's prompts, answers, tool calls and turn ends
 * happen. Rust owns the journal, its search and its bounds; the renderer only
 * asks and draws. Mirrors `crates/interface-contracts/src/thread_journal.rs`;
 * both read `fixtures/thread-journal-v56.json`.
 *
 * Protocol 61 adds `visual {key, id}`: a visual reply's page (T3 section 8),
 * answered as `visual {id, html}` (absent when no longer kept). Both languages
 * read `fixtures/visual-replies-v61.json`.
 */

import type { SessionKey } from "./engine.js";

export type ThreadJournalCommand =
  | { kind: "read"; key: SessionKey; afterSeq?: number; limit?: number }
  | { kind: "tail"; key: SessionKey; limit?: number }
  | { kind: "search"; query: string }
  | { kind: "visual"; key: SessionKey; id: string };

export type TurnOrigin = "launch" | "owner" | "host" | "steer" | "import";

/** One journal line: `entry` is kept as Rust wrote it (its `kind` names it). */
export interface JournalLine {
  seq: number;
  at: number;
  turn?: number;
  entry: { kind: string } & Record<string, unknown>;
}

export interface JournalPage {
  key: SessionKey;
  entries: JournalLine[];
  firstSeq?: number;
  lastSeq: number;
  floorSeq: number;
  hasMore: boolean;
}

/** One search match: a prompt or a message, with a little text around it. */
export interface JournalHit {
  key: SessionKey;
  seq: number;
  turn?: number;
  role: "prompt" | "message";
  excerpt: string;
  at: number;
}

/** A visual reply's page, exactly as the agent gave it. */
export interface ThreadVisual {
  id: string;
  html: string;
}

export interface ThreadJournalResponse {
  page?: JournalPage;
  hits?: JournalHit[];
  visual?: ThreadVisual;
}

/** Whether `id` is a visual id as Rust makes them (`is_visual_id`). */
export function isVisualId(id: unknown): id is string {
  return typeof id === "string" && /^v\d{13}-[0-9a-f]{8}$/.test(id);
}

/** The page in a `visual` answer, or null for anything else (not kept, malformed). */
export function threadVisualOf(response: unknown, id: string): string | null {
  if (typeof response !== "object" || response === null) return null;
  const visual = (response as { visual?: unknown }).visual;
  if (typeof visual !== "object" || visual === null) return null;
  const { id: answered, html } = visual as { id?: unknown; html?: unknown };
  return answered === id && typeof html === "string" ? html : null;
}

/** The Threads search asks the journal from this many characters on. */
export const MESSAGE_SEARCH_MIN_CHARS = 3;

/** Whether `query` is long enough to search thread messages. */
export function searchesMessages(query: string): boolean {
  return query.trim().length >= MESSAGE_SEARCH_MIN_CHARS;
}

/** The well-formed hits in an answer, in Rust's order; none for anything else. */
export function journalHitsOf(response: unknown): JournalHit[] {
  if (typeof response !== "object" || response === null) return [];
  const hits = (response as { hits?: unknown }).hits;
  if (!Array.isArray(hits)) return [];
  return hits.filter((hit): hit is JournalHit => typeof hit === "object" && hit !== null
    && typeof (hit as JournalHit).key === "string" && (hit as JournalHit).key !== ""
    && typeof (hit as JournalHit).excerpt === "string"
    && typeof (hit as JournalHit).seq === "number"
    && ((hit as JournalHit).role === "prompt" || (hit as JournalHit).role === "message"));
}
