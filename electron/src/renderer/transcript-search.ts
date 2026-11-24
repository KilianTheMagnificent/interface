import { compactSeamLabel, type ConvItem } from "./conversation-reducer";
import { toolActivityGroupName, toolActivityName } from "./tool-activity";
import type { TranscriptRow } from "./transcript-merge";

/**
 * The transcript find bar's bounded corpus, search and navigation.
 */

/**
 * One item-level result in the transcript find bar. Results deliberately map to
 * rendered items, not individual occurrences inside one item: moving to a result
 * therefore always moves to a different visible row/card instead of apparently
 * doing nothing when the same word occurs twice in one long tool payload.
 */
export interface TranscriptSearchMatch {
  /** Stable render target, shared with Conversation's item wrapper. */
  key: string;
  kind: ConvItem["kind"];
}

/** The key used by a nested subagent child in both search and rendering. */
export function transcriptChildKey(parentKey: string, index: number): string {
  return `${parentKey}/${index}`;
}

/**
 * Hard renderer-side ceilings for transcript find. History is bounded by
 * message count, not bytes, so one legal tool payload can still be many
 * megabytes. Indexing a useful prefix keeps Cmd+F interactive; the result
 * object below discloses whenever this prefix or the result cap won.
 */
export const TRANSCRIPT_SEARCH_MAX_QUERY_CHARS = 256;
export const TRANSCRIPT_SEARCH_MAX_RESULTS = 200;
export const TRANSCRIPT_SEARCH_MAX_CORPUS_CHARS = 2 * 1024 * 1024;
export const TRANSCRIPT_SEARCH_MAX_ITEM_CHARS = 64 * 1024;
const TRANSCRIPT_SEARCH_MAX_VALUE_NODES = 10_000;
const TRANSCRIPT_SEARCH_MAX_VALUE_DEPTH = 64;

export interface TranscriptSearchCorpusEntry extends TranscriptSearchMatch {
  /** Already lower-cased once when the rendered rows changed. */
  text: string;
}

export interface TranscriptSearchCorpus {
  entries: readonly TranscriptSearchCorpusEntry[];
  /** Some item text or later rendered rows did not fit in the corpus ceiling. */
  truncated: boolean;
}

export interface TranscriptSearchResult {
  matches: readonly TranscriptSearchMatch[];
  /** At least one additional matching row exists beyond matches[]. */
  resultsTruncated: boolean;
  /** The corpus covers only a disclosed prefix of the rendered transcript. */
  corpusTruncated: boolean;
  /** A non-UI caller supplied more text than the input maxLength permits. */
  queryTruncated: boolean;
}

export interface TranscriptSearchCorpusOptions {
  maxCorpusChars?: number;
  maxItemChars?: number;
}

interface TextCollector {
  readonly parts: string[];
  readonly seen: WeakSet<object>;
  limit: number;
  length: number;
  nodes: number;
  truncated: boolean;
}

function positiveSearchLimit(value: number | undefined, fallback: number): number {
  return value === undefined || !Number.isFinite(value)
    ? fallback : Math.max(1, Math.floor(value));
}

function appendSearchText(collector: TextCollector, value: string): void {
  if (value.length === 0) return;
  const separator = collector.parts.length === 0 ? "" : "\n";
  const remaining = collector.limit - collector.length;
  if (remaining <= separator.length) {
    collector.truncated = true;
    return;
  }
  if (separator !== "") {
    collector.parts.push(separator);
    collector.length += separator.length;
  }
  const available = collector.limit - collector.length;
  collector.parts.push(value.slice(0, available));
  collector.length += Math.min(value.length, available);
  if (value.length > available) collector.truncated = true;
}

/**
 * Flatten an engine-supplied JSON-ish payload directly into a bounded collector.
 * JSON.stringify first allocates the entire payload. Keys and primitive values
 * are the searchable content visible in a folded tool card; punctuation is not
 * a separate navigable row and does not need its own index representation.
 */
function appendSearchValue(collector: TextCollector, value: unknown, depth = 0): void {
  if (collector.length >= collector.limit) {
    collector.truncated = true;
    return;
  }
  if (collector.nodes >= TRANSCRIPT_SEARCH_MAX_VALUE_NODES
      || depth >= TRANSCRIPT_SEARCH_MAX_VALUE_DEPTH) {
    collector.truncated = true;
    return;
  }
  collector.nodes += 1;

  if (value === null) {
    appendSearchText(collector, "null");
    return;
  }
  switch (typeof value) {
    case "string":
      appendSearchText(collector, value);
      return;
    case "number":
    case "boolean":
    case "bigint":
      appendSearchText(collector, String(value));
      return;
    case "undefined":
      appendSearchText(collector, "undefined");
      return;
    case "symbol":
    case "function":
      try { appendSearchText(collector, String(value)); } catch { /* unreadable value */ }
      return;
    case "object":
      break;
  }

  if (collector.seen.has(value)) {
    appendSearchText(collector, "[Circular]");
    return;
  }
  collector.seen.add(value);
  try {
    if (Array.isArray(value)) {
      for (const member of value) {
        appendSearchValue(collector, member, depth + 1);
        if (collector.truncated) break;
      }
      return;
    }
    let keys: string[];
    try { keys = Object.keys(value); } catch {
      collector.truncated = true;
      return;
    }
    for (const key of keys) {
      appendSearchText(collector, key);
      if (collector.length >= collector.limit) {
        collector.truncated = true;
        break;
      }
      try {
        appendSearchValue(collector, (value as Record<string, unknown>)[key], depth + 1);
      } catch {
        // A getter is outside the engine's JSON contract. Keep the rest of the
        // payload searchable and disclose that this item could not be complete.
        collector.truncated = true;
      }
      if (collector.truncated) break;
    }
  } finally {
    collector.seen.delete(value);
  }
}

function boundedConversationItemSearchText(item: ConvItem, limit: number): {
  text: string;
  truncated: boolean;
} {
  const collector: TextCollector = {
    parts: [], seen: new WeakSet(), limit, length: 0, nodes: 0, truncated: false,
  };
  const add = (value: string | undefined): void => {
    if (value !== undefined) appendSearchText(collector, value);
  };
  switch (item.kind) {
    case "text":
    case "user":
    case "notice":
      add(item.text);
      break;
    case "compact":
      add(compactSeamLabel(item));
      break;
    case "peer":
      // The engine name and the objective come first because they are how a
      // reader looks one of these up — "what did I ask codex" — and the reply
      // itself is bounded by the same limit as any other body.
      add(item.sourceEngine);
      add(item.objective);
      add(item.text);
      break;
    case "tool":
      add(item.tool);
      // Index the words the transcript actually paints as well as the raw
      // provider identifier. A row shown as "JavaScript" and a folded run
      // shown as "Called Node REPL…" must remain findable by those labels.
      add(toolActivityName(item.tool));
      add(toolActivityGroupName(item.tool));
      add(`${item.result === undefined ? "Calling" : "Called"} ${toolActivityGroupName(item.tool)}`);
      add(item.summary);
      appendSearchValue(collector, item.input);
      add(item.result?.summary);
      break;
    case "approval":
      add(item.tool);
      add(item.summary);
      appendSearchValue(collector, item.input);
      add(item.resolved);
      break;
    case "diff":
      add(item.path);
      break;
    case "visual":
      add(item.title);
      break;
    case "subagent":
      add(item.subagentType);
      add(item.description);
      add(item.model);
      add(item.status);
      add(item.lastActivity);
      break;
    default: {
      const unhandled: never = item;
      return unhandled;
    }
  }
  return { text: collector.parts.join(""), truncated: collector.truncated };
}

// Items are immutable and retained by identity while a transcript streams, so
// each one is collected and lower-cased once per item ceiling, not once per
// streamed frame while the find bar is open. The corpus ceiling is applied at
// use, by slicing.
const ITEM_SEARCH_TEXT = new WeakMap<ConvItem, { maxItemChars: number; text: string; truncated: boolean }>();

function itemSearchText(item: ConvItem, maxItemChars: number): { text: string; truncated: boolean } {
  const known = ITEM_SEARCH_TEXT.get(item);
  if (known !== undefined && known.maxItemChars === maxItemChars) return known;
  const indexed = boundedConversationItemSearchText(item, maxItemChars);
  const entry = { maxItemChars, text: indexed.text.toLowerCase(), truncated: indexed.truncated };
  ITEM_SEARCH_TEXT.set(item, entry);
  return entry;
}

/**
 * Build the bounded, lower-cased search corpus once per rendered row set. The
 * history/live seam is chrome rather than conversation text and is skipped;
 * nested subagent children retain the same depth-first order as rendering.
 */
export function buildTranscriptSearchCorpus(
  rows: readonly TranscriptRow[],
  options: TranscriptSearchCorpusOptions = {},
): TranscriptSearchCorpus {
  const maxCorpusChars = positiveSearchLimit(options.maxCorpusChars,
    TRANSCRIPT_SEARCH_MAX_CORPUS_CHARS);
  const maxItemChars = positiveSearchLimit(options.maxItemChars,
    TRANSCRIPT_SEARCH_MAX_ITEM_CHARS);
  const entries: TranscriptSearchCorpusEntry[] = [];
  let used = 0;
  let truncated = false;

  const visit = (item: ConvItem, key: string): boolean => {
    const remaining = maxCorpusChars - used;
    if (remaining <= 0) {
      truncated = true;
      return false;
    }
    const indexed = itemSearchText(item, maxItemChars);
    const text = indexed.text.slice(0, Math.min(maxItemChars, remaining));
    entries.push({ key, kind: item.kind, text });
    used += text.length;
    if (indexed.truncated || text.length < indexed.text.length) truncated = true;
    if (item.kind === "subagent") {
      for (let index = 0; index < item.children.length; index += 1) {
        if (!visit(item.children[index], transcriptChildKey(key, index))) return false;
      }
    }
    return true;
  };

  for (const row of rows) {
    if (row.kind === "item" && !visit(row.item, row.key)) break;
  }
  return { entries, truncated };
}

/** Cheap per-keystroke phase over the corpus built above. */
export function searchTranscriptCorpus(
  corpus: TranscriptSearchCorpus,
  rawQuery: string,
  maxResults = TRANSCRIPT_SEARCH_MAX_RESULTS,
): TranscriptSearchResult {
  const query = rawQuery.slice(0, TRANSCRIPT_SEARCH_MAX_QUERY_CHARS);
  const queryTruncated = query.length !== rawQuery.length;
  if (query.length === 0) {
    return { matches: [], resultsTruncated: false,
      corpusTruncated: corpus.truncated, queryTruncated };
  }
  const needle = query.toLowerCase();
  const limit = Math.min(TRANSCRIPT_SEARCH_MAX_RESULTS,
    positiveSearchLimit(maxResults, TRANSCRIPT_SEARCH_MAX_RESULTS));
  const matches: TranscriptSearchMatch[] = [];
  let resultsTruncated = false;
  for (const entry of corpus.entries) {
    if (!entry.text.includes(needle)) continue;
    if (matches.length === limit) {
      resultsTruncated = true;
      break;
    }
    matches.push({ key: entry.key, kind: entry.kind });
  }
  return { matches, resultsTruncated, corpusTruncated: corpus.truncated, queryTruncated };
}

/** The visible/accessibility count, including every limit that weakens it. */
export function transcriptSearchStatus(
  query: string,
  index: number,
  result: TranscriptSearchResult,
): string {
  if (query.length === 0) return "Type to search";
  const queryNote = result.queryTruncated ? " · query cap" : "";
  const corpusNote = result.corpusTruncated ? " · partial" : "";
  if (result.matches.length === 0) {
    return `0 matches${corpusNote}${queryNote}`;
  }
  const current = Math.min(Math.max(index, 0), result.matches.length - 1) + 1;
  const total = `${result.matches.length}${result.resultsTruncated ? "+" : ""}`;
  return `${current} of ${total}${corpusNote}${queryNote}`;
}

/** Wrap-around navigation shared by Enter, Shift+Enter, and the two buttons. */
export function moveTranscriptSearchIndex(
  index: number,
  count: number,
  direction: -1 | 1,
): number {
  const total = Number.isFinite(count) ? Math.max(0, Math.floor(count)) : 0;
  if (total === 0) return 0;
  const last = total - 1;
  const current = Number.isFinite(index)
    ? Math.min(Math.max(Math.floor(index), 0), last) : 0;
  return (current + direction + last + 1) % (last + 1);
}
