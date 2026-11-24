import { assertNever, type ConvItem, type ToolItem } from "./conversation-reducer";
import type { TranscriptRow } from "./transcript-merge";

/**
 * An imported session's markers, read back as rows (`expandImportedRows`).
 */

/* ==========================================================================
   AN IMPORTED SESSION — another tool's markers, read back as rows.

   A session imported from a different agent (Codex's own importer is the one
   in the wild) arrives as ORDINARY MESSAGE TEXT carrying the exporter's
   markers. No engine event says "this was a tool call": the whole record is
   prose, so the transcript drew it as prose — the marker as the prompt title in
   the jump strip, `[external_agent_tool_call: Bash]` as a paragraph, and the
   underscores inside `mcp__server__tool` as italics.

   The grammar, verified against the rollout files this app replays
   (the `rollout-….jsonl` files under `~/.codex`). Nothing in `crates/` emits
   the markers — the exporter is the other tool's, which is why the parser is
   tolerant:

     <EXTERNAL SESSION IMPORTED>          a message of its own, once, at the top

     [external unsupported block: image]  a line, then a blank line, then the
                                          real words; several may stack. The
                                          block itself did not survive the
                                          export.

     [external_agent_tool_call: Bash]     the call. Body lines are `key: value`
     description: …                       with values that may run over several
     command: …                           lines; the keys seen are description,
     [/external_agent_tool_call]          command, file and input.

     [external_agent_tool_result]         the output of the call in the message
     …                                    directly above. `: error` in the open
     [/external_agent_tool_result]        tag is the only variant.

   TWO RULES KEEP THE PARSER OFF ORDINARY PROSE, because this text is an
   agent's own words and an agent that WRITES about the grammar — a message
   demonstrating it in a fenced example, say — must come back unchanged:

     · a line inside an open ``` or ~~~ fence is never a marker; and
     · an opening tag is only a marker when its matching close exists later in
       the same message, outside a fence. An unterminated one stays prose
       rather than swallowing the rest of the message into a tool card.

   Neither rule costs anything on real data: over the 3,858 marker blocks in the
   rollout corpus, every open has its close and not one marker sits inside a
   fence.

   ROWS, NOT MARKUP. Each piece becomes an item the pane already draws — a tool
   card, its result, a notice, a divider — so an imported turn reads exactly
   like a native one and nothing here needs a renderer of its own.

   AFTER THE MERGE, NOT BEFORE. Splitting one item into several means minting
   ids, and a synthesized `tool` id would be an ENGINE-MINTED id by
   `isEngineId`'s rule — mergeTranscript would then dedupe two unrelated blocks
   that happened to sit at the same position in the history and live lists.
   Running over rows instead keeps the echo/dedupe contract untouched: this
   function only ever re-keys within one row's own namespace.
   ========================================================================== */

/** What the divider a `<EXTERNAL SESSION IMPORTED>` marker becomes says. */
export const IMPORTED_SESSION_LABEL = "Imported session";

const IMPORT_CALL_OPEN = /^\[external_agent_tool_call: ([^\]\n]*)\]$/u;
const IMPORT_CALL_CLOSE = "[/external_agent_tool_call]";
const IMPORT_RESULT_OPEN = /^\[external_agent_tool_result(?:: ([^\]\n]*))?\]$/u;
const IMPORT_RESULT_CLOSE = "[/external_agent_tool_result]";
const IMPORT_UNSUPPORTED = /^\[external unsupported block: ([^\]\n]*)\]$/u;
const IMPORT_SESSION_MARK = "<EXTERNAL SESSION IMPORTED>";
/** A body line that opens a named argument. Anything else continues the last. */
const IMPORT_FIELD = /^([a-z][a-z0-9_]*): ?(.*)$/u;
/** The opening run of a code fence, either flavour. */
const IMPORT_FENCE = /^(`{3,}|~{3,})/u;

type ImportSegment =
  | { kind: "prose"; text: string }
  | { kind: "call"; tool: string; body: string }
  | { kind: "result"; ok: boolean; body: string }
  | { kind: "unsupported"; block: string }
  | { kind: "imported" };

/** Only prose carries markers, and only these two item kinds carry prose. */
type ProseItem = Extract<ConvItem, { kind: "text" | "user" }>;
function isProseItem(item: ConvItem): item is ProseItem {
  return item.kind === "text" || item.kind === "user";
}

/**
 * The cheap gate, run once per message of every transcript (importParse keeps
 * the answer). Two substring scans, and the whole parse below happens only for
 * a message that passes one of them.
 */
function hasImportMarkers(text: string): boolean {
  return text.includes("[external") || text.includes(IMPORT_SESSION_MARK);
}

/**
 * Which lines sit INSIDE a code fence, and are therefore an example rather than
 * a marker. CommonMark's rule, minus the parts a marker line cannot exercise: a
 * run of three or more backticks or tildes opens, and a bare run of the same
 * character at least as long closes.
 */
function importFencedLines(lines: readonly string[]): boolean[] {
  const inside: boolean[] = [];
  let open: string | null = null;
  for (const raw of lines) {
    const line = raw.trim();
    const fence = IMPORT_FENCE.exec(line);
    if (open === null) {
      // The opening line is not itself inside, and cannot be a marker anyway.
      inside.push(false);
      if (fence !== null) open = fence[1];
      continue;
    }
    const closes = fence !== null && line === fence[1]
      && fence[1][0] === open[0] && fence[1].length >= open.length;
    inside.push(!closes);
    if (closes) open = null;
  }
  return inside;
}

/** Line-oriented, because every marker in the grammar is a whole line. */
function importSegments(text: string): ImportSegment[] {
  const segments: ImportSegment[] = [];
  const lines = text.split("\n");
  const fenced = importFencedLines(lines);
  let prose: string[] = [];
  const flushProse = (): void => {
    const said = prose.join("\n").trim();
    prose = [];
    if (said !== "") segments.push({ kind: "prose", text: said });
  };
  /** Where this block's closing tag is, or -1: an open with no close is prose. */
  const findClose = (start: number, close: string): number => {
    for (let at = start; at < lines.length; at += 1) {
      if (!fenced[at] && lines[at].trim() === close) return at;
    }
    return -1;
  };
  const readBlock = (start: number, end: number): string =>
    lines.slice(start, end).join("\n").trim();
  for (let index = 0; index < lines.length; index += 1) {
    const line = fenced[index] ? "" : lines[index].trim();
    const call = IMPORT_CALL_OPEN.exec(line);
    const callEnd = call === null ? -1 : findClose(index + 1, IMPORT_CALL_CLOSE);
    if (call !== null && callEnd >= 0) {
      flushProse();
      segments.push({ kind: "call", tool: call[1].trim(),
        body: readBlock(index + 1, callEnd) });
      index = callEnd;
      continue;
    }
    const result = IMPORT_RESULT_OPEN.exec(line);
    const resultEnd = result === null ? -1 : findClose(index + 1, IMPORT_RESULT_CLOSE);
    if (result !== null && resultEnd >= 0) {
      flushProse();
      segments.push({ kind: "result", ok: result[1] === undefined,
        body: readBlock(index + 1, resultEnd) });
      index = resultEnd;
      continue;
    }
    const unsupported = IMPORT_UNSUPPORTED.exec(line);
    if (unsupported !== null) {
      flushProse();
      segments.push({ kind: "unsupported", block: unsupported[1].trim() });
      continue;
    }
    if (line === IMPORT_SESSION_MARK) {
      flushProse();
      segments.push({ kind: "imported" });
      continue;
    }
    prose.push(lines[index]);
  }
  flushProse();
  return segments;
}

/**
 * The call's named arguments, or null when the body is not in that shape — in
 * which case the raw block is the input and is printed as the text it is.
 * Anchored on the FIRST line: a body that does not open with a field is not
 * one, and half-parsing it would move text under a key it never had.
 */
function importCallFields(body: string): Record<string, string> | null {
  const lines = body.split("\n");
  if (lines[0] === undefined || !IMPORT_FIELD.test(lines[0])) return null;
  const fields: Record<string, string> = {};
  let key = "";
  let buffer: string[] = [];
  const flush = (): void => {
    if (key !== "") fields[key] = buffer.join("\n").trim();
    buffer = [];
  };
  for (const line of lines) {
    const field = IMPORT_FIELD.exec(line);
    if (field === null) { buffer.push(line); continue; }
    flush();
    key = field[1];
    buffer.push(field[2]);
  }
  flush();
  return fields;
}

function importCallItem(id: string, tool: string, body: string): ConvItem {
  const fields = importCallFields(body);
  // The sentence the exporter kept for exactly this slot; a path is the next
  // best thing a Read or an Edit row can say about itself. `||` and not `??`:
  // `description:` with nothing after it is present but says nothing, and the
  // row should fall through to the path rather than print a blank slot.
  const summary = fields?.description || fields?.file || "";
  return { kind: "tool", id, tool, summary, input: fields ?? body };
}

/** One row this expansion produced, before it is told which key it gets. */
type ImportRow = { divider: true } | { item: ConvItem };

/**
 * What one message's markers expand to. A pure function of the message and of
 * ONE fact about the rows above it: whether the tool row directly above may
 * still take a leading `[external_agent_tool_result]`. Both answers are kept.
 */
interface ImportExpansion {
  produced: readonly ImportRow[];
  /** Which produced row keeps the message's own row key. */
  keeper: number;
  /** A leading result handed to the tool row directly above this message. */
  claimed?: { ok: boolean; summary: string };
  /** The rows keyed under the last row key and origin they were asked for. */
  keyed?: { key: string; origin: TranscriptOrigin; rows: readonly TranscriptRow[] };
  /** The row above with `claimed` merged in, by the row it replaces. */
  merged?: WeakMap<TranscriptRow, TranscriptRow>;
}

/**
 * Everything expandImportedRows learns about one message, kept per ITEM.
 * Items are immutable and retained by identity across frames, so an imported
 * session is parsed once rather than on every streamed token, and the rows it
 * produces keep their identity for the memos downstream.
 */
interface ImportParse {
  /** The cheap substring gate. */
  marked: boolean;
  /** Undefined until asked for; null when the markers were only mentioned. */
  segments?: readonly ImportSegment[] | null;
  plain?: ImportExpansion;
  claiming?: ImportExpansion;
}
type TranscriptOrigin = Extract<TranscriptRow, { kind: "item" }>["origin"];
const IMPORT_PARSES = new WeakMap<ProseItem, ImportParse>();

function importParse(item: ProseItem): ImportParse {
  let parse = IMPORT_PARSES.get(item);
  if (parse === undefined) {
    parse = { marked: hasImportMarkers(item.text) };
    IMPORT_PARSES.set(item, parse);
  }
  return parse;
}

/** The cached parse of a message that really carries markers, or null. */
function importedMessage(
  item: ProseItem,
): (ImportParse & { segments: readonly ImportSegment[] }) | null {
  const parse = importParse(item);
  if (!parse.marked) return null;
  if (parse.segments === undefined) {
    const segments = importSegments(item.text);
    // The gate is a substring test, so an agent that merely WROTE about these
    // markers reaches here. Nothing was found: the row goes back untouched
    // rather than as a re-trimmed copy of its own prose.
    parse.segments = segments.some(segment => segment.kind !== "prose") ? segments : null;
  }
  // The cached object itself, so the expansions stored on it below persist.
  return parse.segments === null
    ? null : parse as ImportParse & { segments: readonly ImportSegment[] };
}

function importExpansion(
  source: ProseItem, segments: readonly ImportSegment[], claimsAbove: boolean,
): ImportExpansion {
  const produced: ImportRow[] = [];
  let claimed: { ok: boolean; summary: string } | undefined;
  let above = claimsAbove;
  const claimResult = (result: { ok: boolean; summary: string }): boolean => {
    const last = produced[produced.length - 1];
    if (last !== undefined) {
      if (!("item" in last) || last.item.kind !== "tool"
        || last.item.result !== undefined) return false;
      produced[produced.length - 1] = { item: { ...last.item, result } };
      return true;
    }
    // Only the row directly above, and only once: it has its result after this.
    if (!above) return false;
    above = false;
    claimed = result;
    return true;
  };
  for (const segment of segments) {
    const id = `${source.id}~x${String(produced.length)}`;
    switch (segment.kind) {
      case "prose":
        produced.push({ item: { ...source, id, text: segment.text } });
        break;
      case "call":
        produced.push({ item: importCallItem(id, segment.tool, segment.body) });
        break;
      case "result":
        if (claimResult({ ok: segment.ok, summary: segment.body })) break;
        // Nothing above it to belong to. The output is still the record, so
        // it is said rather than dropped.
        produced.push({ item: { kind: "notice", id, text: segment.body,
          tone: segment.ok ? "info" : "error" } });
        break;
      case "unsupported":
        produced.push({ item: { kind: "notice", id,
          text: `${segment.block} not imported`, tone: "info" } });
        break;
      case "imported":
        produced.push({ divider: true });
        break;
      default: assertNever(segment);
    }
  }
  const owner = produced.findIndex(
    entry => "item" in entry && entry.item.kind === source.kind);
  const keeper = owner >= 0 ? owner : 0;
  return claimed === undefined ? { produced, keeper } : { produced, keeper, claimed };
}

function keyedImportRows(
  expansion: ImportExpansion, row: Extract<TranscriptRow, { kind: "item" }>,
): readonly TranscriptRow[] {
  const known = expansion.keyed;
  if (known !== undefined && known.key === row.key && known.origin === row.origin) {
    return known.rows;
  }
  const rows = expansion.produced.map((entry, index): TranscriptRow => {
    const key = index === expansion.keeper ? row.key : `${row.key}~x${String(index)}`;
    return "divider" in entry
      ? { kind: "mark", key, label: IMPORTED_SESSION_LABEL }
      : { kind: "item", key, origin: row.origin, item: entry.item };
  });
  expansion.keyed = { key: row.key, origin: row.origin, rows };
  return rows;
}

type ToolTranscriptRow = Extract<TranscriptRow, { kind: "item" }> & { item: ToolItem };

function claimedImportRow(
  expansion: ImportExpansion, result: { ok: boolean; summary: string },
  target: ToolTranscriptRow,
): TranscriptRow {
  const merged = expansion.merged ??= new WeakMap();
  const known = merged.get(target);
  if (known !== undefined) return known;
  const row: TranscriptRow = { ...target, item: { ...target.item, result } };
  merged.set(target, row);
  return row;
}

/**
 * PURE. The merged transcript in, the same transcript with any imported
 * session's markers read back as rows, out.
 *
 * KEYS. The row that carries the message's own kind keeps the key
 * mergeTranscript gave it, so a user turn stays the same anchor for the prompt
 * strip and for a turn checkpoint; the extra rows a message produced are
 * suffixed within that namespace and are unique by construction.
 *
 * A transcript with no markers is returned BY IDENTITY, so nothing downstream
 * recomputes for the shape this function exists for and never sees.
 */
export function expandImportedRows(
  rows: readonly TranscriptRow[],
): readonly TranscriptRow[] {
  const marked = rows.some(row => row.kind === "item" && isProseItem(row.item)
    && importParse(row.item).marked);
  if (!marked) return rows;

  const out: TranscriptRow[] = [];
  // Where the tool row still missing its `[external_agent_tool_result]` sits in
  // `out`. Only the row IMMEDIATELY before a result may claim it, so a seam, a
  // notice or an unrelated message between the two leaves both standing alone.
  let awaiting = -1;
  for (const row of rows) {
    const source = row.kind === "item" && isProseItem(row.item) ? row.item : null;
    const parse = source === null ? null : importedMessage(source);
    if (row.kind !== "item" || source === null || parse === null) {
      out.push(row);
      awaiting = -1;
      continue;
    }
    const target = awaiting >= 0 ? out[awaiting] : undefined;
    const above = target !== undefined && target.kind === "item"
      && target.item.kind === "tool" && target.item.result === undefined
      ? target as ToolTranscriptRow : undefined;
    // Only a LEADING result can reach past the message it sits in: anything
    // before it gives it a row of its own to claim or to stand beside.
    const expansion = above !== undefined && parse.segments[0]?.kind === "result"
      ? parse.claiming ??= importExpansion(source, parse.segments, true)
      : parse.plain ??= importExpansion(source, parse.segments, false);
    if (expansion.claimed !== undefined && above !== undefined) {
      out[awaiting] = claimedImportRow(expansion, expansion.claimed, above);
    }
    out.push(...keyedImportRows(expansion, row));
    const last = out[out.length - 1];
    awaiting = last !== undefined && last.kind === "item" && last.item.kind === "tool"
      && last.item.result === undefined && expansion.produced.length > 0 ? out.length - 1 : -1;
  }
  return out;
}
