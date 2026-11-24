import {
  Fragment, memo, useEffect, useReducer, useRef, useState, type ReactElement, type ReactNode,
} from "react";
import { equalValue } from "./stable-values";

/**
 * The providers all hand the renderer one opaque string for assistant prose.
 * This is intentionally a small display parser, not an HTML parser: every
 * character eventually becomes a React text node, so model-authored HTML and
 * scripts are escaped by React and can never become DOM.
 */

export type MarkdownInline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string }
  | { kind: "strong"; children: MarkdownInline[] }
  | { kind: "emphasis"; children: MarkdownInline[] }
  | { kind: "strike"; children: MarkdownInline[] }
  | { kind: "link"; href?: string; fileTarget?: string; children: MarkdownInline[] }
  | { kind: "annotation"; index?: string };

export type MarkdownBlock =
  | { kind: "paragraph"; lines: MarkdownInline[][] }
  | { kind: "heading"; level: number; children: MarkdownInline[] }
  | { kind: "list"; ordered: boolean; start?: number; items: MarkdownInline[][][];
      /** Per item, a task box: true ticked, false open, null not a task. Absent when no item is one. */
      tasks?: (boolean | null)[];
      /** Per item, the lists indented under it. Absent when no item has one. */
      nested?: MarkdownBlock[][] }
  | { kind: "table"; header: MarkdownInline[][];
      alignments: Array<"left" | "center" | "right" | undefined>;
      rows: MarkdownInline[][][] }
  | { kind: "quote"; children: MarkdownBlock[] }
  | { kind: "code"; language?: string; text: string }
  | { kind: "rule" }
  | { kind: "code-comment"; title: string; body?: string; file?: string;
      start?: string; end?: string; priority?: string };

type Directive = { name: string; attributes: Record<string, string> };

const MAX_INLINE_DEPTH = 8;
const MAX_BLOCK_DEPTH = 6;
const MAX_LANGUAGE_CHARS = 40;
const WHITESPACE = /\s/;
const DIRECTIVE_KEY_CHAR = /[a-z0-9_-]/i;
const INLINE_TEXT_STOPS = "\\`[*_<:~";
const FENCE_CLOSE = new Map<string, RegExp>();

function pushText(tokens: MarkdownInline[], text: string): void {
  if (text.length === 0) return;
  const last = tokens[tokens.length - 1];
  if (last?.kind === "text") last.text += text;
  else tokens.push({ kind: "text", text });
}

/** Parse the attribute grammar emitted by Codex UI directives. */
export function parseMarkdownDirective(source: string): Directive | undefined {
  const colonCount = source.startsWith("::") ? 2 : source.startsWith(":") ? 1 : 0;
  if (colonCount === 0 || !source.endsWith("}")) return undefined;
  const open = source.indexOf("{", colonCount);
  if (open < 0) return undefined;
  const name = source.slice(colonCount, open);
  if (!/^[a-z][a-z0-9-]*$/i.test(name)) return undefined;

  const attributes: Record<string, string> = {};
  const body = source.slice(open + 1, -1);
  let at = 0;
  while (at < body.length) {
    while (WHITESPACE.test(body[at] ?? "")) at += 1;
    if (at >= body.length) break;
    const keyStart = at;
    while (DIRECTIVE_KEY_CHAR.test(body[at] ?? "")) at += 1;
    const key = body.slice(keyStart, at);
    if (key.length === 0) return undefined;
    while (WHITESPACE.test(body[at] ?? "")) at += 1;
    if (body[at] !== "=") return undefined;
    at += 1;
    while (WHITESPACE.test(body[at] ?? "")) at += 1;

    let value = "";
    const quote = body[at];
    if (quote === "\"" || quote === "'") {
      at += 1;
      let closed = false;
      while (at < body.length) {
        const char = body[at];
        at += 1;
        if (char === quote) {
          closed = true;
          break;
        }
        if (char !== "\\" || at >= body.length) {
          value += char;
          continue;
        }
        const escaped = body[at];
        at += 1;
        value += escaped === "n" ? "\n"
          : escaped === "r" ? "\r"
          : escaped === "t" ? "\t" : escaped;
      }
      if (!closed) return undefined;
    } else {
      const valueStart = at;
      while (at < body.length && !WHITESPACE.test(body[at]!)) at += 1;
      value = body.slice(valueStart, at);
      if (value.length === 0) return undefined;
    }
    attributes[key] = value;
  }
  return { name, attributes };
}

/**
 * Only navigation schemes that cannot execute script or open a local file are
 * links. Unsafe/relative destinations keep their human label but lose href.
 */
export function safeMarkdownHref(source: string): string | undefined {
  const value = source.trim();
  if (value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:" || url.protocol === "mailto:"
      ? url.href : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Extracts a safe local file path from a Markdown link destination (`file://`
 * URL, absolute POSIX path, or workspace-relative path without `..` traversal)
 * so an opted-in transcript can open it in the in-app file viewer.
 */
export function safeMarkdownFileTarget(source: string): string | undefined {
  const value = source.trim().replace(/^<|>$/gu, "").trim();
  if (value.length === 0 || value.length > 4096 || value.includes("\\") || /[\u0000-\u001f\u007f]/u.test(value)) {
    return undefined;
  }
  let candidate = value;
  if (/^file:\/\//iu.test(value)) {
    try {
      const url = new URL(value);
      if (url.protocol !== "file:" || (url.hostname !== "" && url.hostname !== "localhost")) {
        return undefined;
      }
      candidate = decodeURIComponent(url.pathname);
    } catch {
      return undefined;
    }
  } else if (/^[a-z][a-z0-9+.-]*:/iu.test(value)) {
    return undefined;
  }
  const hashAt = candidate.indexOf("#");
  if (hashAt >= 0) candidate = candidate.slice(0, hashAt);
  const queryAt = candidate.indexOf("?");
  if (queryAt >= 0) candidate = candidate.slice(0, queryAt);
  candidate = candidate.replace(/:(\d+)(?::(\d+))?$/u, "").trim();
  if (candidate.length === 0 || candidate.includes("\\") || /[\u0000-\u001f\u007f]/u.test(candidate)) {
    return undefined;
  }
  if (candidate.startsWith("/")) {
    const trimmed = candidate.length > 1 ? candidate.replace(/\/+$/u, "") : candidate;
    const parts = trimmed.split("/").slice(1);
    if (parts.length === 0 || parts.some(part => part === "" || part === "." || part === "..")) {
      return undefined;
    }
    return `/${parts.join("/")}`;
  }
  const relative = candidate.replace(/^\.\//u, "").replace(/\/+$/u, "");
  if (relative.length === 0) return undefined;
  const parts = relative.split("/");
  if (parts.some(part => part === "" || part === "." || part === "..")) return undefined;
  return parts.join("/");
}

type InlineBudget = { remaining: number };

function findDirectiveEnd(source: string, start: number, budget: InlineBudget): number {
  let quote = "";
  let escaped = false;
  for (let at = start; at < source.length && budget.remaining > 0; at += 1) {
    budget.remaining -= 1;
    const char = source[at];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (char === "\\" && quote.length > 0) {
      escaped = true;
      continue;
    }
    if (quote.length > 0) {
      if (char === quote) quote = "";
      continue;
    }
    if (char === "\"" || char === "'") quote = char;
    else if (char === "}") return at;
  }
  return -1;
}

const PUNCTUATION = /[\p{P}\p{S}]/u;

/** The whole character before `at`, a surrogate pair included; none at the start. */
function charBefore(source: string, at: number): string | undefined {
  if (at <= 0) return undefined;
  const low = source.charCodeAt(at - 1);
  return at >= 2 && low >= 0xdc00 && low <= 0xdfff ? source.slice(at - 2, at) : source[at - 1];
}

function charAt(source: string, at: number): string | undefined {
  const code = source.codePointAt(at);
  return code === undefined ? undefined : String.fromCodePoint(code);
}

/**
 * CommonMark's flanking rules for the delimiter run holding `at` (the edge of
 * the source counts as whitespace). Without them `max_tokens and top_p` lost
 * both underscores to an emphasis, and `2 * 3 * 4` italicised the 3. `*` and
 * `~~` open when left-flanking and close when right-flanking; `_` also never
 * opens or closes inside a word. Judged on the whole run, so the second `_`
 * of `my__init__py` does not count the first as punctuation before it. The
 * run walk is charged to the budget, so a line of ten thousand `*` stays a
 * linear parse.
 */
function delimiterCan(role: "open" | "close", source: string, at: number,
    budget: InlineBudget): boolean {
  const mark = source[at];
  let start = at;
  while (start > 0 && source[start - 1] === mark) start -= 1;
  let end = at + 1;
  while (end < source.length && source[end] === mark) end += 1;
  budget.remaining -= end - start;
  const before = charBefore(source, start);
  const after = charAt(source, end);
  const underscore = mark === "_";
  const spaceBefore = before === undefined || WHITESPACE.test(before);
  const spaceAfter = after === undefined || WHITESPACE.test(after);
  const punctuationBefore = before !== undefined && PUNCTUATION.test(before);
  const punctuationAfter = after !== undefined && PUNCTUATION.test(after);
  const left = !spaceAfter && (!punctuationAfter || spaceBefore || punctuationBefore);
  const right = !spaceBefore && (!punctuationBefore || spaceAfter || punctuationAfter);
  if (role === "open") return left && (!underscore || !right || punctuationBefore);
  return right && (!underscore || !left || punctuationAfter);
}

/** Inline Markdown used by paragraphs, headings, list items and links. */
export function parseMarkdownInline(source: string, depth = 0): MarkdownInline[] {
  return parseInline(source, depth, { remaining: Math.max(4096, source.length * 8) });
}

function parseInline(source: string, depth: number, budget: InlineBudget): MarkdownInline[] {
  if (source.length === 0) return [];
  if (depth >= MAX_INLINE_DEPTH) return [{ kind: "text", text: source }];
  const tokens: MarkdownInline[] = [];
  let at = 0;
  // Share a linear search allowance through recursive labels/emphasis. On
  // pathological partial markup the untouched suffix remains escaped plain text.
  const find = (marker: string, from: number): number => {
    if (budget.remaining <= 0) return -1;
    const end = source.indexOf(marker, from);
    budget.remaining -= (end < 0 ? source.length : end + marker.length) - from;
    return end;
  };

  while (at < source.length) {
    if (budget.remaining <= 0) { pushText(tokens, source.slice(at)); break; }
    // Backslash escaping is presentation-only; it never becomes HTML.
    if (source[at] === "\\" && at + 1 < source.length
        && /[\\`*_[\]{}()#+.!>~|\-]/.test(source[at + 1])) {
      pushText(tokens, source[at + 1]);
      at += 2;
      continue;
    }

    if (source[at] === "`") {
      let width = 1;
      while (source[at + width] === "`") width += 1;
      const marker = "`".repeat(width);
      const end = find(marker, at + width);
      if (end >= 0) {
        let value = source.slice(at + width, end).replace(/\n/g, " ");
        if (/^\s.*\s$/.test(value) && value.trim().length > 0) value = value.slice(1, -1);
        tokens.push({ kind: "code", text: value });
        at = end + width;
        continue;
      }
      pushText(tokens, marker);
      at += width;
      continue;
    }

    if (source.startsWith(":codex-annotation{", at)) {
      const end = findDirectiveEnd(source, at + ":codex-annotation{".length, budget);
      if (end >= 0) {
        const directive = parseMarkdownDirective(source.slice(at, end + 1));
        if (directive?.name === "codex-annotation") {
          const raw = directive.attributes.index;
          tokens.push({ kind: "annotation",
            ...(raw !== undefined && /^\d{1,6}$/.test(raw) ? { index: raw } : {}) });
          at = end + 1;
          continue;
        }
      }
    }

    if (source[at] === "[" && source[at - 1] !== "!") {
      const labelEnd = find("](", at + 1);
      if (labelEnd >= 0) {
        const destinationEnd = find(")", labelEnd + 2);
        if (destinationEnd >= 0) {
          const destination = source.slice(labelEnd + 2, destinationEnd).trim();
          // A minimal optional-title split. URLs containing spaces are invalid
          // here; a quoted title is display metadata and needs no DOM surface.
          const titleAt = destination.search(/\s+["']/);
          const hrefSource = (titleAt < 0 ? destination : destination.slice(0, titleAt))
            .replace(/^<|>$/g, "");
          const href = safeMarkdownHref(hrefSource);
          const fileTarget = href === undefined ? safeMarkdownFileTarget(hrefSource) : undefined;
          tokens.push({
            kind: "link",
            ...(href === undefined ? {} : { href }),
            ...(fileTarget === undefined ? {} : { fileTarget }),
            children: parseInline(source.slice(at + 1, labelEnd), depth + 1, budget),
          });
          at = destinationEnd + 1;
          continue;
        }
      }
    }

    if (source[at] === "<") {
      const end = find(">", at + 1);
      if (end >= 0) {
        const candidate = source.slice(at + 1, end);
        const href = safeMarkdownHref(candidate);
        const fileTarget = href === undefined && /^file:\/\//iu.test(candidate.trim())
          ? safeMarkdownFileTarget(candidate) : undefined;
        if (href !== undefined || fileTarget !== undefined) {
          tokens.push({
            kind: "link",
            ...(href === undefined ? {} : { href }),
            ...(fileTarget === undefined ? {} : { fileTarget }),
            children: [{ kind: "text", text: candidate }],
          });
          at = end + 1;
          continue;
        }
      }
    }

    const paired = ([marker, kind]: [string, "strong" | "emphasis" | "strike"]): boolean => {
      if (!source.startsWith(marker, at)) return false;
      if (!delimiterCan("open", source, at, budget)) return false;
      let end = find(marker, at + marker.length);
      if (end <= at + marker.length) return false;
      while (!delimiterCan("close", source, end, budget)) {
        end = find(marker, end + 1);
        if (end < 0) return false;
      }
      tokens.push({ kind, children: parseInline(
        source.slice(at + marker.length, end), depth + 1, budget) });
      at = end + marker.length;
      return true;
    };
    if (paired(["**", "strong"]) || paired(["__", "strong"])
        || paired(["~~", "strike"]) || paired(["*", "emphasis"])
        || paired(["_", "emphasis"])) continue;

    // Consume ordinary text in one node. ':' and '<' remain boundaries because
    // either can begin a supported construct later in the same string.
    let end = at + 1;
    while (end < source.length && !INLINE_TEXT_STOPS.includes(source[end]!)) end += 1;
    pushText(tokens, source.slice(at, end));
    at = end;
  }
  return tokens;
}

type Fence = { marker: string; language?: string };

function fenceAt(line: string): Fence | undefined {
  const match = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
  if (match === null) return undefined;
  const info = match[2].trim().split(/\s+/, 1)[0] ?? "";
  const language = /^[a-z0-9_+.#-]+$/i.test(info) && info.length <= MAX_LANGUAGE_CHARS
    ? info : undefined;
  return { marker: match[1], ...(language === undefined ? {} : { language }) };
}

function fenceClosePattern(marker: string): RegExp {
  const key = `${marker[0]}${marker.length}`;
  const cached = FENCE_CLOSE.get(key);
  if (cached !== undefined) return cached;
  const pattern = new RegExp(`^ {0,3}\\${marker[0]}{${marker.length},}\\s*$`);
  FENCE_CLOSE.set(key, pattern);
  return pattern;
}

function closesFence(line: string, fence: Fence): boolean {
  return fenceClosePattern(fence.marker).test(line);
}

type ListLine = { ordered: boolean; start?: number; text: string };

function listAt(line: string): ListLine | undefined {
  const unordered = /^ {0,3}[-+*][ \t]+(.+)$/.exec(line);
  if (unordered !== null) return { ordered: false, text: unordered[1] };
  const ordered = /^ {0,3}(\d{1,9})[.)][ \t]+(.+)$/.exec(line);
  if (ordered === null) return undefined;
  return { ordered: true, start: Number(ordered[1]), text: ordered[2] };
}

function indentOf(line: string): number {
  return line.length - line.trimStart().length;
}

function headingAt(line: string): { level: number; text: string } | undefined {
  const match = /^ {0,3}(#{1,6})[ \t]+(.*)$/.exec(line);
  if (match === null) return undefined;
  return { level: match[1].length, text: match[2].replace(/[ \t]+#+[ \t]*$/, "") };
}

function codeCommentAt(line: string): Extract<MarkdownBlock, { kind: "code-comment" }>
    | undefined {
  const directive = parseMarkdownDirective(line.trim());
  if (directive?.name !== "code-comment" || !line.trim().startsWith("::")) return undefined;
  const attr = directive.attributes;
  return {
    kind: "code-comment", title: attr.title?.trim() || "Code comment",
    ...(attr.body === undefined ? {} : { body: attr.body }),
    ...(attr.file === undefined ? {} : { file: attr.file }),
    ...(attr.start === undefined ? {} : { start: attr.start }),
    ...(attr.end === undefined ? {} : { end: attr.end }),
    ...(attr.priority === undefined ? {} : { priority: attr.priority }),
  };
}

function isRule(line: string): boolean {
  return /^ {0,3}(?:(?:\*\s*){3,}|(?:-\s*){3,}|(?:_\s*){3,})$/.test(line);
}

/**
 * Split a GFM-style table row without treating escaped pipes or pipes inside
 * an inline-code span as cell boundaries. The original Markdown escapes stay
 * in each cell so the normal inline parser remains the single presentation
 * path. A row without a real pipe is not a table row.
 */
export function splitMarkdownTableRow(line: string): string[] | undefined {
  const source = line.trim();
  // Without a pipe nothing below can set sawPipe. Every prose line is probed as
  // a possible table header, so this is the common case by far.
  if (source.length === 0 || !source.includes("|")) return undefined;
  const cells: string[] = [];
  let cell = "";
  let codeWidth = 0;
  let sawPipe = false;

  for (let at = 0; at < source.length;) {
    const char = source[at];
    if (char === "\\" && at + 1 < source.length) {
      cell += source.slice(at, at + 2);
      at += 2;
      continue;
    }
    if (char === "`") {
      let width = 1;
      while (source[at + width] === "`") width += 1;
      if (codeWidth === 0) codeWidth = width;
      else if (codeWidth === width) codeWidth = 0;
      cell += "`".repeat(width);
      at += width;
      continue;
    }
    if (char === "|" && codeWidth === 0) {
      cells.push(cell.trim());
      cell = "";
      sawPipe = true;
      at += 1;
      continue;
    }
    cell += char;
    at += 1;
  }
  cells.push(cell.trim());
  if (!sawPipe) return undefined;
  if (source.startsWith("|")) cells.shift();
  // Only an actual trailing delimiter creates an empty final cell. An escaped
  // pipe at end-of-line is content and leaves a non-empty cell here.
  if (cells[cells.length - 1] === "") cells.pop();
  return cells.length >= 2 ? cells : undefined;
}

type MarkdownTable = Extract<MarkdownBlock, { kind: "table" }> & { next: number };

function tableAt(lines: readonly string[], index: number): MarkdownTable | undefined {
  const headerCells = splitMarkdownTableRow(lines[index] ?? "");
  const delimiterCells = splitMarkdownTableRow(lines[index + 1] ?? "");
  if (headerCells === undefined || delimiterCells === undefined
      || headerCells.length !== delimiterCells.length) return undefined;

  const alignments: MarkdownTable["alignments"] = [];
  for (const rawCell of delimiterCells) {
    const cell = rawCell.replace(/\s+/g, "");
    if (!/^:?-{3,}:?$/.test(cell)) return undefined;
    alignments.push(cell.startsWith(":") && cell.endsWith(":") ? "center"
      : cell.endsWith(":") ? "right"
      : cell.startsWith(":") ? "left" : undefined);
  }

  // Require one complete body row. During streaming, a header and delimiter
  // alone remain readable prose instead of flashing into a half-built table.
  const firstBody = splitMarkdownTableRow(lines[index + 2] ?? "");
  if (firstBody === undefined || firstBody.length !== headerCells.length) return undefined;

  const rawRows: string[][] = [];
  let next = index + 2;
  while (next < lines.length && lines[next].trim().length > 0) {
    const row = splitMarkdownTableRow(lines[next]);
    if (row === undefined || row.length !== headerCells.length) break;
    rawRows.push(row);
    next += 1;
  }
  return {
    kind: "table",
    header: headerCells.map(cell => parseMarkdownInline(cell)),
    alignments,
    rows: rawRows.map(row => row.map(cell => parseMarkdownInline(cell))),
    next,
  };
}

function startsBlock(line: string): boolean {
  return fenceAt(line) !== undefined || headingAt(line) !== undefined
    || listAt(line) !== undefined || /^ {0,3}>/.test(line)
    || codeCommentAt(line) !== undefined || isRule(line);
}

function normalizeLineEndings(source: string): string {
  return source.includes("\r") ? source.replace(/\r\n?/g, "\n") : source;
}

/** Parse the supported block subset without interpreting raw HTML. */
export function parseMarkdown(source: string, depth = 0): MarkdownBlock[] {
  return parseBlocks(normalizeLineEndings(source).split("\n"), depth);
}

/**
 * Where the top-level parse stood on a blank line that is not the last line:
 * `offset` starts the line after it, and the first `blocks` blocks are final
 * whatever text follows. A blank line ends every paragraph, list, quote and
 * table; no lookahead reads past one (a table probe stops at it); and a fence
 * that is still open never lets the top level reach one. Nothing reads
 * backwards either, so what follows parses exactly as a document of its own.
 */
interface MarkdownCheckpoint { readonly offset: number; readonly blocks: number }

const DOCUMENT_START: MarkdownCheckpoint = { offset: 0, blocks: 0 };

/** A top-level parse that a longer version of the same text can resume. */
export interface MarkdownParse {
  /** The text with its line endings normalized. */
  readonly source: string;
  readonly blocks: readonly MarkdownBlock[];
  /** Ascending. */
  readonly checkpoints: readonly MarkdownCheckpoint[];
}

/** The last checkpoint of `previous` that `source` still shares its text up to. */
function resumeIndex(previous: MarkdownParse, source: string): number {
  const points = previous.checkpoints;
  if (points.length === 0) return -1;
  if (source.startsWith(previous.source)) return points.length - 1;
  // Sharing the text up to an offset implies sharing it up to every earlier one.
  let low = 0;
  let high = points.length - 1;
  let found = -1;
  while (low <= high) {
    const middle = (low + high) >> 1;
    const offset = points[middle].offset;
    if (source.length >= offset && source.startsWith(previous.source.slice(0, offset))) {
      found = middle;
      low = middle + 1;
    } else high = middle - 1;
  }
  return found;
}

/**
 * parseMarkdown(text), reusing `previous` — the parse of an earlier version of
 * the same message. A streaming message only grows, so everything above its
 * last blank line is kept (the same block objects, which is what lets the
 * memoized block views skip them) and only the tail after it is parsed again.
 * A tail block equal to the one it replaces keeps the old object too. The
 * blocks always equal parseMarkdown(text); any text that does not share a
 * checkpoint's prefix is simply parsed whole.
 */
export function parseMarkdownIncremental(text: string, previous?: MarkdownParse): MarkdownParse {
  const source = normalizeLineEndings(text);
  if (previous === undefined) {
    const checkpoints: MarkdownCheckpoint[] = [];
    return { source, blocks: parseBlocks(source.split("\n"), 0, checkpoints), checkpoints };
  }
  if (previous.source === source) return previous;
  const index = resumeIndex(previous, source);
  const { offset, blocks: kept } = index < 0 ? DOCUMENT_START : previous.checkpoints[index];
  const tailPoints: MarkdownCheckpoint[] = [];
  const tail = parseBlocks(source.slice(offset).split("\n"), 0, tailPoints);
  const blocks = previous.blocks.slice(0, kept);
  tail.forEach((block, at) => {
    const before = previous.blocks[kept + at];
    blocks.push(before !== undefined && equalValue(before, block) ? before : block);
  });
  const checkpoints = previous.checkpoints.slice(0, index + 1);
  for (const point of tailPoints) {
    checkpoints.push({ offset: point.offset + offset, blocks: point.blocks + kept });
  }
  return { source, blocks, checkpoints };
}

function parseBlocks(lines: readonly string[], depth: number,
    checkpoints?: MarkdownCheckpoint[]): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  let at = 0;
  // The source offset of line `scanned`, advanced only when a checkpoint needs it.
  let scanned = 0;
  let scannedOffset = 0;
  while (at < lines.length) {
    if (lines[at].trim().length === 0) {
      at += 1;
      if (checkpoints !== undefined && at < lines.length) {
        for (; scanned < at; scanned += 1) scannedOffset += lines[scanned].length + 1;
        checkpoints.push({ offset: scannedOffset, blocks: blocks.length });
      }
      continue;
    }

    const table = tableAt(lines, at);
    if (table !== undefined) {
      const { next, ...block } = table;
      blocks.push(block);
      at = next;
      continue;
    }

    const fence = fenceAt(lines[at]);
    if (fence !== undefined) {
      at += 1;
      const body: string[] = [];
      while (at < lines.length && !closesFence(lines[at], fence)) {
        body.push(lines[at]);
        at += 1;
      }
      if (at < lines.length) at += 1;
      blocks.push({ kind: "code", text: body.join("\n"),
        ...(fence.language === undefined ? {} : { language: fence.language }) });
      continue;
    }

    const heading = headingAt(lines[at]);
    if (heading !== undefined) {
      blocks.push({ kind: "heading", level: heading.level,
        children: parseMarkdownInline(heading.text) });
      at += 1;
      continue;
    }

    const comment = codeCommentAt(lines[at]);
    if (comment !== undefined) {
      blocks.push(comment);
      at += 1;
      continue;
    }

    if (isRule(lines[at])) {
      blocks.push({ kind: "rule" });
      at += 1;
      continue;
    }

    const list = listAt(lines[at]);
    if (list !== undefined) {
      const items: MarkdownInline[][][] = [];
      const tasks: (boolean | null)[] = [];
      const nested: MarkdownBlock[][] = [];
      const ordered = list.ordered;
      const start = list.start;
      while (at < lines.length) {
        const member = listAt(lines[at]);
        if (member === undefined || member.ordered !== ordered) break;
        const marker = indentOf(lines[at]);
        // GFM task items: "[ ] " or "[x] " opens the item's text.
        const task = /^\[([ xX])\][ \t]+(\S.*)$/.exec(member.text);
        const itemLines = [task === null ? member.text : task[2]];
        const children: MarkdownBlock[] = [];
        at += 1;
        while (at < lines.length && /^ {2,}\S/.test(lines[at])) {
          const indent = indentOf(lines[at]);
          // A list indented past this item's marker nests under it, with every
          // deeper line after it; one at the marker's own depth is a sibling.
          if (indent > marker && listAt(lines[at].slice(indent)) !== undefined) {
            const chunk: string[] = [];
            while (at < lines.length && /^ {2,}\S/.test(lines[at]) && indentOf(lines[at]) > marker) {
              chunk.push(lines[at].slice(Math.min(indent, indentOf(lines[at]))));
              at += 1;
            }
            children.push(...depth >= MAX_BLOCK_DEPTH
              ? [{ kind: "paragraph" as const, lines: chunk.map(line => parseMarkdownInline(line.trim())) }]
              : parseBlocks(chunk, depth + 1));
            continue;
          }
          if (children.length > 0 || startsBlock(lines[at].replace(/^ {2,4}/, ""))) break;
          itemLines.push(lines[at].trim());
          at += 1;
        }
        items.push(itemLines.map(line => parseMarkdownInline(line)));
        tasks.push(task === null ? null : task[1] !== " ");
        nested.push(children);
      }
      blocks.push({ kind: "list", ordered, ...(ordered ? { start } : {}), items,
        ...(tasks.some(task => task !== null) ? { tasks } : {}),
        ...(nested.some(children => children.length > 0) ? { nested } : {}) });
      continue;
    }

    if (/^ {0,3}>/.test(lines[at])) {
      const quoted: string[] = [];
      while (at < lines.length) {
        const match = /^ {0,3}>[ \t]?(.*)$/.exec(lines[at]);
        if (match === null) break;
        quoted.push(match[1]);
        at += 1;
      }
      const quoteSource = quoted.join("\n");
      blocks.push({ kind: "quote", children: depth >= MAX_BLOCK_DEPTH
        ? [{ kind: "paragraph", lines: [[{ kind: "text", text: quoteSource }]] }]
        : parseMarkdown(quoteSource, depth + 1) });
      continue;
    }

    const paragraph: MarkdownInline[][] = [];
    while (at < lines.length && lines[at].trim().length > 0
        && (paragraph.length === 0
          || !startsBlock(lines[at]) && tableAt(lines, at) === undefined)) {
      paragraph.push(parseMarkdownInline(lines[at]));
      at += 1;
    }
    blocks.push({ kind: "paragraph", lines: paragraph });
  }
  return blocks;
}

/* ==========================================================================
   SETTLING IN (Threads, owner 2026-10-07). While a paced reveal paints a
   streaming tail, the prose each paint adds is drawn in a span that fades in
   once, on mount (`.conv-md-fresh`, scoped to the Threads layout in
   Conversation.css). Everything here is measured in PROSE characters — the
   text tokens' own characters, in the order BlockView draws them — never in
   source offsets, so no syntax, escape or link destination has to be mapped
   back, and code (fenced or inline) is never prose: it paints at once.

   A span has to outlive the frame that made it, or it would be unwrapped 16 ms
   into a 240 ms fade and its words would pop. So each paint is a RANGE that
   keeps its own span, keyed by where it starts, until FRESH_HOLD_MS has
   passed; the paint after it adds the next range beside it. Only blocks the
   paint changed are measured (a settled block keeps its parsed object, so
   nothing about it is read or redrawn), and text that was laid out again
   rather than extended — a `**` closing, a paragraph becoming a table — is
   painted as it now is, never faded a second time.
   ========================================================================== */

/** How long a fresh range stays wrapped: its 240 ms fade, and a margin for the commit. */
export const FRESH_HOLD_MS = 320;
/** Prose painted this soon after a range began joins it, so the span count stays small at 120 Hz. */
export const FRESH_JOIN_MS = 32;
/** More new prose than this in one paint was not typed in: it is painted as it is. */
export const FRESH_MAX_CHARS = 400;

/** Prose characters [from, to) of one block (see blockProse), first painted at `at` ms. */
export interface FreshRange { readonly from: number; readonly to: number; readonly at: number }
/** The ranges of block `block` still settling in: ascending and disjoint. */
export interface FreshBlock { readonly block: number; readonly ranges: readonly FreshRange[] }
/**
 * What settling in has measured: the blocks the next paint is compared with
 * (none until a paint is paced, so whatever is on screen then stays as it is)
 * and every range still fading in. Ranges are kept for unchanged blocks by
 * identity, which is what lets BlockView's memo hold across a frame.
 */
export interface FreshState {
  readonly blocks: readonly MarkdownBlock[] | undefined;
  readonly fresh: readonly FreshBlock[];
}

export const NO_FRESH: FreshState = { blocks: undefined, fresh: [] };

function inlineProse(tokens: readonly MarkdownInline[]): string {
  let prose = "";
  for (const token of tokens) {
    if (token.kind === "text") prose += token.text;
    else if (token.kind !== "code" && token.kind !== "annotation") prose += inlineProse(token.children);
  }
  return prose;
}

/** How many prose characters one token draws: inlineProse([token]).length, without the string. */
function tokenLength(token: MarkdownInline): number {
  if (token.kind === "text") return token.text.length;
  return token.kind === "code" || token.kind === "annotation" ? 0 : inlineLength(token.children);
}

function inlineLength(tokens: readonly MarkdownInline[]): number {
  let length = 0;
  for (const token of tokens) length += tokenLength(token);
  return length;
}

const BLOCK_PROSE = new WeakMap<MarkdownBlock, string>();

/**
 * A block's prose: the characters of its text tokens, strong, emphasis,
 * strike and link text included, in exactly the order BlockView draws them.
 * Code blocks, inline code, annotations, rules and code comments have none.
 * Cached per block object, which never changes once parsed.
 */
export function blockProse(block: MarkdownBlock): string {
  const cached = BLOCK_PROSE.get(block);
  if (cached !== undefined) return cached;
  let prose = "";
  switch (block.kind) {
    case "paragraph": for (const line of block.lines) prose += inlineProse(line); break;
    case "heading": prose = inlineProse(block.children); break;
    case "list":
      block.items.forEach((item, index) => {
        for (const line of item) prose += inlineProse(line);
        for (const child of block.nested?.[index] ?? []) prose += blockProse(child);
      });
      break;
    case "table":
      for (const cell of block.header) prose += inlineProse(cell);
      for (const row of block.rows) for (const cell of row) prose += inlineProse(cell);
      break;
    case "quote": for (const child of block.children) prose += blockProse(child); break;
    case "code": case "rule": case "code-comment": break;
  }
  BLOCK_PROSE.set(block, prose);
  return prose;
}

/** The prose of `blocks` from `first` on, and where each of those blocks starts in it. */
function proseFrom(blocks: readonly MarkdownBlock[], first: number): { text: string; starts: number[] } {
  let text = "";
  const starts: number[] = [];
  for (let index = first; index < blocks.length; index += 1) {
    starts.push(text.length);
    text += blockProse(blocks[index]);
  }
  return { text, starts };
}

function sharedPrefix(a: string, b: string): number {
  if (b.startsWith(a)) return a.length;
  const end = Math.min(a.length, b.length);
  let at = 0;
  while (at < end && a.charCodeAt(at) === b.charCodeAt(at)) at += 1;
  return at;
}

const GRAPHEMES = new Intl.Segmenter(undefined, { granularity: "grapheme" });

/**
 * `at`, moved back to the start of the character it falls inside: a surrogate
 * pair, a flag, a ZWJ family or a letter and its accent is never split between
 * two spans, where each half would draw as a broken glyph for a whole fade.
 */
export function graphemeStart(text: string, at: number): number {
  if (at <= 0 || at >= text.length) return at;
  const from = Math.max(0, at - 32);
  let start = at;
  for (const { index } of GRAPHEMES.segment(text.slice(from, Math.min(text.length, at + 32)))) {
    if (from + index > at) break;
    start = from + index;
  }
  return start;
}

/** `ranges` cut off at `end`: the same array when nothing reaches it. */
function clampRanges(ranges: readonly FreshRange[], end: number): readonly FreshRange[] {
  if (ranges.length === 0 || ranges[ranges.length - 1].to <= end) return ranges;
  const kept: FreshRange[] = [];
  for (const range of ranges) {
    if (range.to <= end) kept.push(range);
    else if (range.from < end) kept.push({ ...range, to: end });
  }
  return kept;
}

/** Every range whose fade is over, dropped: the same array when none is. */
function expireFresh(fresh: readonly FreshBlock[], now: number): readonly FreshBlock[] {
  let changed = false;
  const kept: FreshBlock[] = [];
  const live = (range: FreshRange): boolean => now - range.at < FRESH_HOLD_MS;
  for (const entry of fresh) {
    if (entry.ranges.every(live)) { kept.push(entry); continue; }
    changed = true;
    const ranges = entry.ranges.filter(live);
    if (ranges.length > 0) kept.push({ block: entry.block, ranges });
  }
  return changed ? kept : fresh;
}

/**
 * The ranges after a paint that changed blocks `first` on. A range survives on
 * a block of the same kind that starts where it did, cut at `end` (the prose
 * the two paints share); one on a block that vanished, moved or changed kind
 * is dropped, and its words are simply painted.
 */
function trimFresh(fresh: readonly FreshBlock[], before: readonly MarkdownBlock[],
    blocks: readonly MarkdownBlock[], first: number, wasStarts: readonly number[],
    isStarts: readonly number[], end: number): readonly FreshBlock[] {
  let changed = false;
  const kept: FreshBlock[] = [];
  for (const entry of fresh) {
    if (entry.block < first) { kept.push(entry); continue; }
    const at = entry.block - first;
    const block = blocks[entry.block];
    const ranges = block === undefined || before[entry.block]?.kind !== block.kind
      || wasStarts[at] !== isStarts[at]
      ? [] : clampRanges(entry.ranges, Math.min(end - isStarts[at], blockProse(block).length));
    if (ranges === entry.ranges) { kept.push(entry); continue; }
    changed = true;
    if (ranges.length > 0) kept.push({ block: entry.block, ranges });
  }
  return changed ? kept : fresh;
}

/** `fresh` with prose [from, to) of the changed region (blocks `first` on) painted at `now`. */
function addFresh(fresh: readonly FreshBlock[], blocks: readonly MarkdownBlock[], first: number,
    starts: readonly number[], from: number, to: number, now: number): FreshBlock[] {
  const next = [...fresh];
  for (let index = first; index < blocks.length; index += 1) {
    const start = starts[index - first];
    const local = Math.max(from, start) - start;
    const localEnd = Math.min(to, start + blockProse(blocks[index]).length) - start;
    if (local >= localEnd) continue;
    const at = next.findIndex(entry => entry.block === index);
    const ranges = at < 0 ? [] : next[at].ranges;
    const last = ranges[ranges.length - 1];
    const grown = last !== undefined && last.to === local && now - last.at < FRESH_JOIN_MS
      ? [...ranges.slice(0, -1), { ...last, to: localEnd }]
      : [...ranges, { from: local, to: localEnd, at: now }];
    if (at < 0) next.push({ block: index, ranges: grown });
    else next[at] = { block: index, ranges: grown };
  }
  return next.sort((a, b) => a.block - b.block);
}

/**
 * Settling in, one paint later: `blocks` is the parse just painted, `now` its
 * time, and `settle` whether this paint is a paced step of a streaming tail.
 *
 * A paced paint that only EXTENDED the prose adds what it painted as a range,
 * unless that is more than FRESH_MAX_CHARS. A paint that is not paced (a snap,
 * a flood, a pane that stopped pacing) adds nothing but lets the ranges
 * already fading finish — cutting one short would pop its words. A first paced
 * paint only takes its measure. Pure: the same arguments give the same state.
 */
export function advanceFresh(previous: FreshState, blocks: readonly MarkdownBlock[], now: number,
    settle: boolean): FreshState {
  const fresh = expireFresh(previous.fresh, now);
  const before = previous.blocks;
  if (before === undefined) return settle ? { blocks, fresh: [] } : NO_FRESH;
  if (!settle && fresh.length === 0) return NO_FRESH;
  if (before === blocks) return fresh === previous.fresh ? previous : { blocks, fresh };
  let first = 0;
  const common = Math.min(before.length, blocks.length);
  while (first < common && before[first] === blocks[first]) first += 1;
  const was = proseFrom(before, first);
  const is = proseFrom(blocks, first);
  const shared = sharedPrefix(was.text, is.text);
  // Prose that only grew settles in from where it stood (a character the last
  // paint split joins the new range whole). Prose laid out again is painted.
  const extended = shared === was.text.length;
  const from = extended ? graphemeStart(is.text, shared) : shared;
  let next = trimFresh(fresh, before, blocks, first, was.starts, is.starts, from);
  if (settle && extended && is.text.length > from && is.text.length - from <= FRESH_MAX_CHARS) {
    next = addFresh(next, blocks, first, is.starts, from, is.text.length, now);
  }
  return { blocks, fresh: next };
}

/** How long until every range in `fresh` has expired, as of `now`. */
function freshExpiry(fresh: readonly FreshBlock[], now: number): number {
  let latest = -Infinity;
  for (const entry of fresh) for (const range of entry.ranges) latest = Math.max(latest, range.at);
  return Math.max(0, latest + FRESH_HOLD_MS - now) + 1;
}

/**
 * One text token's characters, which start at prose offset `start`, cut where
 * `ranges` begin and end. A piece with `from` is fresh (that is its key: where
 * it starts, so the span keeps its element, and its fade, from paint to paint);
 * the rest is plain.
 */
export function freshParts(text: string, start: number,
    ranges: readonly FreshRange[]): { text: string; from?: number }[] {
  const end = start + text.length;
  const parts: { text: string; from?: number }[] = [];
  let at = start;
  for (const range of ranges) {
    const from = Math.max(range.from, at);
    const to = Math.min(range.to, end);
    if (from >= to) continue;
    if (from > at) parts.push({ text: text.slice(at - start, from - start) });
    parts.push({ text: text.slice(from - start, to - start), from });
    at = to;
  }
  if (at < end) parts.push({ text: text.slice(at - start) });
  return parts;
}

function freshNow(): number {
  return typeof performance === "undefined" ? 0 : performance.now();
}

/**
 * `fresh` is the ranges of the block being drawn, `base` where this run of
 * tokens starts in that block's prose. Without `fresh` a text token is the
 * plain string it always was.
 */
function Inline({ tokens, fresh, base = 0, onOpenFile }: {
  tokens: readonly MarkdownInline[]; fresh?: readonly FreshRange[]; base?: number;
  onOpenFile?: (path: string) => void;
}): ReactElement {
  let offset = base;
  return <>{tokens.map((token, index): ReactNode => {
    const key = `${token.kind}-${index}`;
    const at = offset;
    if (fresh !== undefined) offset += tokenLength(token);
    switch (token.kind) {
      case "text": return fresh === undefined ? token.text : <Fragment key={key}>
        {freshParts(token.text, at, fresh).map(part => part.from === undefined ? part.text
          : <span key={`f${part.from}`} className="conv-md-fresh">{part.text}</span>)}
      </Fragment>;
      case "code": return <code key={key} className="conv-md-inline-code">{token.text}</code>;
      case "strong": return <strong key={key}>
        <Inline tokens={token.children} fresh={fresh} base={at} onOpenFile={onOpenFile} /></strong>;
      case "emphasis": return <em key={key}>
        <Inline tokens={token.children} fresh={fresh} base={at} onOpenFile={onOpenFile} /></em>;
      case "strike": return <del key={key}>
        <Inline tokens={token.children} fresh={fresh} base={at} onOpenFile={onOpenFile} /></del>;
      case "annotation": {
        const label = token.index === undefined ? "Annotation" : `Annotation ${token.index}`;
        return <span key={key} className="conv-md-annotation" title="Response annotation">
          {label}
        </span>;
      }
      case "link": {
        if (token.href !== undefined) {
          return <a key={key} className="conv-md-link" href={token.href}
              target="_blank" rel="noreferrer noopener">
            <Inline tokens={token.children} fresh={fresh} base={at} onOpenFile={onOpenFile} /></a>;
        }
        if (token.fileTarget !== undefined && onOpenFile !== undefined) {
          const target = token.fileTarget;
          return <a key={key} className="conv-md-link" href="#preview-file"
              title={`Preview ${target}`}
              onClick={(event?: { preventDefault?: () => void }) => {
                event?.preventDefault?.();
                onOpenFile(target);
              }}>
            <Inline tokens={token.children} fresh={fresh} base={at} onOpenFile={onOpenFile} /></a>;
        }
        return <span key={key} className="conv-md-link-disabled"
            title="Unsafe or local link omitted">
          <Inline tokens={token.children} fresh={fresh} base={at} onOpenFile={onOpenFile} /></span>;
      }
      default: {
        const unhandled: never = token;
        return unhandled;
      }
    }
  })}</>;
}

function Lines({ lines, fresh, base = 0, caret, onOpenFile }: {
  lines: readonly MarkdownInline[][]; fresh?: readonly FreshRange[]; base?: number; caret?: boolean;
  onOpenFile?: (path: string) => void;
}): ReactElement {
  let offset = base;
  return <>{lines.map((line, index) => {
    const at = offset;
    if (fresh !== undefined) offset += inlineLength(line);
    return <Fragment key={index}>
      {index > 0 && <br />}
      <Inline tokens={line} fresh={fresh} base={at} onOpenFile={onOpenFile} />
      {caret === true && index === lines.length - 1 && CARET}
    </Fragment>;
  })}</>;
}

/*
 * THE THREADS CARET, inline (owner, 2026-10-07): a Threads pane draws its
 * streaming caret right after the last painted character, inside the last
 * block's final line, when that block is prose — a paragraph, a heading, a
 * list, or a quote ending in one. After code, a table, a rule or a code
 * comment it stands where every pane's caret does, on the line after the
 * block. It is a word joiner (zero width, and no line break allowed on either
 * side, so the caret never wraps away from the word it follows) under the
 * bar Conversation.css draws, and it is hidden from assistive technology.
 */
const CARET = <span className="conv-caret" aria-hidden="true">{"\u2060"}</span>;

/** Whether a streaming caret fits inline at the end of `block` (see THE THREADS CARET). */
function caretFits(block: MarkdownBlock | undefined): boolean {
  if (block === undefined) return false;
  if (block.kind === "quote") return caretFits(block.children[block.children.length - 1]);
  if (block.kind === "list") {
    const children = block.nested?.[block.items.length - 1] ?? [];
    return children.length === 0 || caretFits(children[children.length - 1]);
  }
  return block.kind === "paragraph" || block.kind === "heading";
}

/** Whether a message ending in `blocks` takes its caret inline, in its last block. */
export function caretInline(blocks: readonly MarkdownBlock[]): boolean {
  return caretFits(blocks[blocks.length - 1]);
}

/**
 * The one control in rendered prose, and the most-wanted one in an agent IDE:
 * the commands the agent emitted are the text a reader copies.
 *
 * The confirmation is the button's own label for ~1.5s rather than a toast —
 * this app has no toast system, and the answer belongs on the thing that was
 * pressed. A refused or unavailable clipboard leaves the label alone, which is
 * the honest report: nothing was copied.
 */
function CopyCodeButton({ text }: { text: string }): ReactElement {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = window.setTimeout(() => { setCopied(false); }, 1_500);
    return () => { window.clearTimeout(id); };
  }, [copied]);
  return (
    <button type="button" className="conv-md-code-copy"
      aria-label={copied ? "Copied" : "Copy code"}
      onClick={() => {
        // Guarded twice over: a context with no clipboard at all, and one whose
        // clipboard cannot write text. Either leaves the label alone, which is
        // the honest report — nothing was copied.
        navigator.clipboard?.writeText?.(text)?.then(
          () => { setCopied(true); }, () => undefined);
      }}>
      {copied ? "Copied" : "Copy"}
    </button>
  );
}

/** Nested blocks (a quote's), whose prose runs on from `base` in the ranges of the block holding them. */
function Blocks({ blocks, fresh, base = 0, caret, onOpenFile }: {
  blocks: readonly MarkdownBlock[]; fresh?: readonly FreshRange[]; base?: number; caret?: boolean;
  onOpenFile?: (path: string) => void;
}): ReactElement {
  let offset = base;
  return <>{blocks.map((block, index) => {
    const at = offset;
    if (fresh !== undefined) offset += blockProse(block).length;
    return <BlockView key={`${block.kind}-${index}`} block={block} fresh={fresh} base={at}
      caret={caret === true && index === blocks.length - 1 ? true : undefined}
      onOpenFile={onOpenFile} />;
  })}</>;
}

/**
 * A message's blocks, each with the ranges still settling in on it. A block
 * with none takes `fresh` undefined, the same props it always had, so its
 * memo holds; settling in only ever redraws the blocks a paint touched.
 * `caret` puts the streaming caret inline in the last block, which must be
 * one it fits (caretInline); no other block ever sees the prop.
 */
export function MarkdownBlocks({ blocks, fresh, caret, onOpenFile }: {
  blocks: readonly MarkdownBlock[]; fresh: readonly FreshBlock[]; caret?: boolean;
  onOpenFile?: (path: string) => void;
}): ReactElement {
  return <>{blocks.map((block, index) => <BlockView key={`${block.kind}-${index}`} block={block}
    fresh={fresh.length === 0 ? undefined : fresh.find(entry => entry.block === index)?.ranges}
    caret={caret === true && index === blocks.length - 1 ? true : undefined}
    onOpenFile={onOpenFile} />)}</>;
}

/**
 * One block, memoized on the block object. Blocks are never mutated once
 * parsed, and parseMarkdownIncremental hands every settled block of a growing
 * message its previous object, so a streamed token re-renders only the tail.
 * `fresh` (see SETTLING IN) is the same array from paint to paint while it is
 * unchanged, so a block whose words are still fading keeps its memo too; code
 * and code comments never read it. Only the last block of a streaming Threads
 * message gets `caret` (see THE THREADS CARET), and only while it streams.
 */
const BlockView = memo(function BlockView({ block, fresh, base = 0, caret, onOpenFile }: {
  block: MarkdownBlock; fresh?: readonly FreshRange[]; base?: number; caret?: boolean;
  onOpenFile?: (path: string) => void;
}): ReactElement {
  switch (block.kind) {
    case "paragraph":
      return <p className="conv-md-p">
        <Lines lines={block.lines} fresh={fresh} base={base} caret={caret} onOpenFile={onOpenFile} /></p>;
    case "heading": {
      const Heading = `h${block.level}` as "h1" | "h2" | "h3" | "h4" | "h5" | "h6";
      return <Heading className={`conv-md-heading conv-md-h${block.level}`}>
        <Inline tokens={block.children} fresh={fresh} base={base} onOpenFile={onOpenFile} />
        {caret === true && CARET}
      </Heading>;
    }
    case "list": {
      let offset = base;
      const items = block.items.map((lines, itemIndex) => {
        const at = offset;
        if (fresh !== undefined) for (const line of lines) offset += inlineLength(line);
        const children = block.nested?.[itemIndex] ?? [];
        const childrenAt = offset;
        if (fresh !== undefined) for (const child of children) offset += blockProse(child).length;
        const last = caret === true && itemIndex === block.items.length - 1;
        const task = block.tasks?.[itemIndex] ?? null;
        return <li key={itemIndex} className={task === null ? undefined : "conv-md-task"}>
          {task !== null && <span className="conv-md-task-box" role="checkbox" aria-checked={task}
            aria-disabled="true" aria-label={task ? "Done" : "Not done"}>
            {task && <span className="codicon codicon-check" aria-hidden="true" />}</span>}
          <Lines lines={lines} fresh={fresh} base={at} caret={last && children.length === 0}
            onOpenFile={onOpenFile} />
          {children.length > 0 && <Blocks blocks={children} fresh={fresh} base={childrenAt}
            caret={last ? true : undefined} onOpenFile={onOpenFile} />}
        </li>;
      });
      return block.ordered
        ? <ol className="conv-md-list" start={block.start}>{items}</ol>
        : <ul className="conv-md-list">{items}</ul>;
    }
    case "table": {
      let offset = base;
      const cellBase = (cell: readonly MarkdownInline[]): number => {
        const at = offset;
        if (fresh !== undefined) offset += inlineLength(cell);
        return at;
      };
      return <div className="conv-md-table-wrap" role="region"
        aria-label="Scrollable table" tabIndex={0}>
        <table className="conv-md-table">
          <thead><tr>{block.header.map((cell, cellIndex) => {
            const alignment = block.alignments[cellIndex];
            return <th key={cellIndex} scope="col"
              className={alignment === undefined ? undefined : `is-align-${alignment}`}>
              <Inline tokens={cell} fresh={fresh} base={cellBase(cell)} onOpenFile={onOpenFile} />
            </th>;
          })}</tr></thead>
          <tbody>{block.rows.map((row, rowIndex) => <tr key={rowIndex}>
            {row.map((cell, cellIndex) => {
              const alignment = block.alignments[cellIndex];
              return <td key={cellIndex}
                className={alignment === undefined ? undefined : `is-align-${alignment}`}>
                <Inline tokens={cell} fresh={fresh} base={cellBase(cell)} onOpenFile={onOpenFile} />
              </td>;
            })}
          </tr>)}</tbody>
        </table>
      </div>;
    }
    case "quote":
      return <blockquote className="conv-md-quote">
        <Blocks blocks={block.children} fresh={fresh} base={base} caret={caret} onOpenFile={onOpenFile} />
      </blockquote>;
    // The header row is unconditional now that it holds the copy control; a
    // fence with no info string simply names no language. The `pre` and not
    // its wrapper carries the region: the `pre` is the element that scrolls,
    // so it is the element a keyboard has to be able to reach (WCAG 2.1.1),
    // exactly as .conv-md-table-wrap is above.
    case "code":
      return <div className="conv-md-code">
        <div className="conv-md-code-language">
          <span>{block.language ?? ""}</span>
          <CopyCodeButton text={block.text} />
        </div>
        <pre tabIndex={0} role="region" aria-label="Scrollable code">
          <code>{block.text}</code></pre>
      </div>;
    case "rule": return <hr className="conv-md-rule" />;
    case "code-comment": {
      const line = block.start === undefined ? ""
        : block.end !== undefined && block.end !== block.start
          ? `:${block.start}–${block.end}` : `:${block.start}`;
      const fileTarget = block.file === undefined || onOpenFile === undefined
        ? undefined : safeMarkdownFileTarget(block.file);
      return <aside className="conv-md-code-comment" aria-label="Code comment">
        <div className="conv-md-code-comment-head">
          <strong>{block.title}</strong>
          {block.priority !== undefined
            && <span className="conv-md-priority">P{block.priority}</span>}
        </div>
        {block.body !== undefined
          && <div className="conv-md-code-comment-body">
            <Blocks blocks={parseMarkdown(block.body)} onOpenFile={onOpenFile} />
          </div>}
        {block.file !== undefined
          && <code className="conv-md-code-comment-path">
            {fileTarget !== undefined && onOpenFile !== undefined
              ? <a className="conv-md-link" href="#preview-file" title={`Preview ${fileTarget}`}
                  onClick={(event?: { preventDefault?: () => void }) => {
                    event?.preventDefault?.();
                    onOpenFile(fileTarget);
                  }}>{block.file}{line}</a>
              : <>{block.file}{line}</>}
          </code>}
      </aside>;
    }
    default: {
      const unhandled: never = block;
      return unhandled;
    }
  }
});

/**
 * `settleIn` is true while `text` is a paced step of a streaming tail in a
 * Threads pane: the prose it adds settles in (see SETTLING IN). `caret` is a
 * Threads pane's streaming caret, which Markdown places (see THE THREADS
 * CARET). Every other caller leaves both out and pays nothing for them.
 */
export const Markdown = memo(function Markdown({ text, settleIn, caret, onOpenFile }: {
  text: string; settleIn?: boolean; caret?: boolean; onOpenFile?: (path: string) => void;
}): ReactElement {
  // The previous parse of this message, for the next, longer version of it. A
  // cache and nothing more: a render React discards leaves a parse that is
  // still exactly right for its own text.
  const parsed = useRef<MarkdownParse | undefined>(undefined);
  parsed.current = parseMarkdownIncremental(text, parsed.current);
  const blocks = parsed.current.blocks;
  // The same kind of cache, and as safe to leave behind: advanceFresh is pure,
  // so a render React repeats finds nothing new to add, and one it discards
  // leaves a measure that is still right for its own text.
  const settling = useRef<FreshState>(NO_FRESH);
  if (settleIn === true || settling.current !== NO_FRESH) {
    settling.current = advanceFresh(settling.current, blocks, freshNow(), settleIn === true);
  }
  const { fresh } = settling.current;
  // A fresh span left in place after its fade would fade AGAIN whenever an
  // ancestor is redisplayed (a covered pane is display:none), so once the
  // paints stop, one render after the last range expires puts the words back
  // as plain text.
  const [, expire] = useReducer((count: number) => count + 1, 0);
  useEffect(() => {
    if (fresh.length === 0) return;
    const id = window.setTimeout(expire, freshExpiry(fresh, freshNow()));
    return () => { window.clearTimeout(id); };
  }, [fresh]);
  const inline = caret === true && caretInline(blocks);
  return <>
    <div className="conv-markdown">
      <MarkdownBlocks blocks={blocks} fresh={fresh} caret={inline} onOpenFile={onOpenFile} />
    </div>
    {caret === true && !inline && CARET}
  </>;
});
