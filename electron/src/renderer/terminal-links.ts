export type TerminalLinkTarget =
  | { kind: "file"; path: string }
  | { kind: "url"; url: string };

export interface TerminalLineLink {
  /** 0-based inclusive start character index in the line. */
  startIndex: number;
  /** 0-based exclusive end character index in the line. */
  endIndex: number;
  /** Display text covered by the link. */
  text: string;
  /** Raw target string to pass to `resolveTerminalLinkTarget`. */
  target: string;
  /** Resolved target for activation. */
  resolved: TerminalLinkTarget;
}

const KNOWN_FILE_EXTENSIONS = new Set([
  "md",
  "markdown",
  "pdf",
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "svg",
  "txt",
  "json",
  "jsonl",
  "yaml",
  "yml",
  "toml",
  "rs",
  "ts",
  "tsx",
  "js",
  "jsx",
  "mjs",
  "cjs",
  "css",
  "scss",
  "html",
  "htm",
  "py",
  "sh",
  "zsh",
  "bash",
  "go",
  "c",
  "h",
  "cpp",
  "hpp",
  "java",
  "kt",
  "swift",
  "rb",
  "sql",
  "lock",
  "log",
  "csv",
  "xml",
  "diff",
  "patch",
]);

function hasControlOrBackslash(value: string): boolean {
  if (value === "" || value.length > 4096 || value.includes("\\")) return true;
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return false;
}

function stripFragmentQueryAndLineCol(value: string): string {
  let cleaned = value.trim();
  const hashIndex = cleaned.indexOf("#");
  if (hashIndex > 0) cleaned = cleaned.slice(0, hashIndex);
  const queryIndex = cleaned.indexOf("?");
  if (queryIndex > 0) cleaned = cleaned.slice(0, queryIndex);
  cleaned = cleaned.replace(/:(\d+)(?::(\d+))?$/u, "");
  return cleaned;
}

function normalizeSafeAbsolutePath(rawPath: string): string | null {
  if (!rawPath.startsWith("/") || hasControlOrBackslash(rawPath)) return null;
  const trimmed = rawPath.length > 1 ? rawPath.replace(/\/+$/u, "") : rawPath;
  const parts = trimmed.split("/").slice(1);
  if (parts.length === 0 || parts.some(part => part === "" || part === "." || part === "..")) {
    return null;
  }
  return `/${parts.join("/")}`;
}

function resolveRelativeFilePath(relativeRaw: string, cwd?: string | null): string | null {
  if (!cwd) return null;
  const base = normalizeSafeAbsolutePath(cwd.trim());
  if (!base) return null;
  const stripped = relativeRaw.replace(/^\.\//u, "");
  if (stripped === "" || hasControlOrBackslash(stripped)) return null;
  const parts = stripped.split("/");
  if (parts.some(part => part === "" || part === "." || part === "..")) return null;
  return `${base}/${parts.join("/")}`;
}

/**
 * Resolves a clicked terminal link (from an OSC 8 hyperlink or buffer link
 * provider) into either a local file path for `OutputPreviewPanel` or an
 * external `http(s)` URL.
 */
export function resolveTerminalLinkTarget(
  raw: string,
  cwd?: string | null,
): TerminalLinkTarget | null {
  const trimmed = raw.trim();
  if (hasControlOrBackslash(trimmed)) return null;

  if (/^https?:\/\//iu.test(trimmed)) {
    try {
      const parsed = new URL(trimmed);
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
      return { kind: "url", url: parsed.toString() };
    } catch {
      return null;
    }
  }

  if (/^file:\/\//iu.test(trimmed)) {
    try {
      const parsed = new URL(trimmed);
      if (parsed.protocol !== "file:") return null;
      if (parsed.hostname !== "" && parsed.hostname !== "localhost") return null;
      const decoded = decodeURIComponent(parsed.pathname);
      const cleaned = stripFragmentQueryAndLineCol(decoded);
      const normalized = normalizeSafeAbsolutePath(cleaned);
      return normalized ? { kind: "file", path: normalized } : null;
    } catch {
      return null;
    }
  }

  // Reject any other URI scheme (javascript:, data:, mailto:, artifact:, etc.)
  if (/^[a-z][a-z0-9+.-]*:/iu.test(trimmed)) return null;

  const cleaned = stripFragmentQueryAndLineCol(trimmed);
  if (cleaned.startsWith("/")) {
    const normalized = normalizeSafeAbsolutePath(cleaned);
    return normalized ? { kind: "file", path: normalized } : null;
  }

  const resolved = resolveRelativeFilePath(cleaned, cwd);
  return resolved ? { kind: "file", path: resolved } : null;
}

function hasKnownFileExtension(filePath: string): boolean {
  const cleaned = stripFragmentQueryAndLineCol(filePath);
  const slash = cleaned.lastIndexOf("/");
  const base = slash >= 0 ? cleaned.slice(slash + 1) : cleaned;
  const dot = base.lastIndexOf(".");
  if (dot <= 0 || dot === base.length - 1) return false;
  const stem = base.slice(0, dot);
  const ext = base.slice(dot + 1).toLowerCase();
  if (!/[a-z]/iu.test(stem)) return false;
  return KNOWN_FILE_EXTENSIONS.has(ext);
}

function trimTrailingLinkPunctuation(value: string): string {
  let result = value;
  while (result.length > 0) {
    const last = result[result.length - 1];
    if (last === "." || last === "," || last === ";" || last === "!" || last === "?") {
      result = result.slice(0, -1);
      continue;
    }
    if (last === ")" && !result.includes("(")) {
      result = result.slice(0, -1);
      continue;
    }
    if (last === "]" && !result.includes("[")) {
      result = result.slice(0, -1);
      continue;
    }
    break;
  }
  return result;
}

/**
 * Scans a single terminal buffer line for clickable file references and URLs
 * when the CLI did not wrap them in OSC 8 hyperlinks.
 */
export function detectTerminalLineLinks(
  line: string,
  cwd?: string | null,
): TerminalLineLink[] {
  if (!line || line.length > 4096) return [];
  const links: TerminalLineLink[] = [];
  const occupied: Array<{ start: number; end: number }> = [];

  const overlaps = (start: number, end: number): boolean =>
    occupied.some(span => start < span.end && end > span.start);

  // 1. Markdown links printed in plain text: [label](target)
  const markdownPattern = /\[([^\]\n]+)\]\(([^)\s]+)\)/gu;
  for (const match of line.matchAll(markdownPattern)) {
    const start = match.index ?? 0;
    const full = match[0];
    const target = match[2];
    const end = start + full.length;
    const resolved = resolveTerminalLinkTarget(target, cwd);
    if (resolved && !overlaps(start, end)) {
      occupied.push({ start, end });
      links.push({ startIndex: start, endIndex: end, text: full, target, resolved });
    }
  }

  // 2. Explicit file:// or http(s):// URLs
  const urlPattern = /\b(?:file|https?):\/\/[^\s<>()"'`]+/giu;
  for (const match of line.matchAll(urlPattern)) {
    const start = match.index ?? 0;
    const trimmed = trimTrailingLinkPunctuation(match[0]);
    const end = start + trimmed.length;
    if (overlaps(start, end)) continue;
    const resolved = resolveTerminalLinkTarget(trimmed, cwd);
    if (resolved) {
      occupied.push({ start, end });
      links.push({ startIndex: start, endIndex: end, text: trimmed, target: trimmed, resolved });
    }
  }

  // 3. Plain-text file paths (absolute or relative with a known file extension)
  const pathPattern = /(?:^|(?<=[\s"'`([<]))((?:\/|\.\.?\/)?(?:[A-Za-z0-9._-]+\/)*[A-Za-z0-9._-]+\.[A-Za-z0-9]+(?::\d+(?::\d+)?)?(?:#L\d+(?:-L?\d+)?)?)/gu;
  for (const match of line.matchAll(pathPattern)) {
    const start = match.index ?? 0;
    const rawCandidate = trimTrailingLinkPunctuation(match[1]);
    const end = start + rawCandidate.length;
    if (!rawCandidate || rawCandidate.startsWith("..")) continue;
    if (overlaps(start, end)) continue;
    if (start > 0 && line[start - 1] === "@") continue;
    if (!hasKnownFileExtension(rawCandidate)) continue;
    const resolved = resolveTerminalLinkTarget(rawCandidate, cwd);
    if (resolved && resolved.kind === "file") {
      occupied.push({ start, end });
      links.push({
        startIndex: start,
        endIndex: end,
        text: rawCandidate,
        target: rawCandidate,
        resolved,
      });
    }
  }

  return links.sort((a, b) => a.startIndex - b.startIndex);
}

export interface TerminalLinkBufferLine {
  readonly isWrapped?: boolean;
  translateToString(trimRight?: boolean): string;
}

export interface TerminalLinkBuffer {
  readonly length: number;
  getLine(y: number): TerminalLinkBufferLine | undefined;
}

export interface ProvidedTerminalBufferLink {
  text: string;
  range: {
    start: { x: number; y: number };
    end: { x: number; y: number };
  };
  activate: () => void;
}

const MAX_WRAPPED_LINK_ROWS = 8;

/**
 * Computes xterm `ILink` descriptors for 1-based `bufferLineNumber`, joining
 * wrapped buffer rows so long file paths and URLs remain clickable across
 * line wraps.
 */
export function provideTerminalBufferLinks(
  buffer: TerminalLinkBuffer,
  bufferLineNumber: number,
  cwd: string | undefined,
  onOpenLink: ((link: string) => void) | undefined,
): ProvidedTerminalBufferLink[] | undefined {
  if (!onOpenLink) return undefined;
  const targetRow = bufferLineNumber - 1;
  if (targetRow < 0 || targetRow >= buffer.length) return undefined;
  const currentLine = buffer.getLine(targetRow);
  if (!currentLine) return undefined;

  let startRow = targetRow;
  let stepsUp = 0;
  while (
    startRow > 0 &&
    stepsUp < MAX_WRAPPED_LINK_ROWS &&
    buffer.getLine(startRow)?.isWrapped === true
  ) {
    startRow -= 1;
    stepsUp += 1;
  }

  let endRow = targetRow;
  let stepsDown = 0;
  while (
    endRow + 1 < buffer.length &&
    stepsDown < MAX_WRAPPED_LINK_ROWS &&
    buffer.getLine(endRow + 1)?.isWrapped === true
  ) {
    endRow += 1;
    stepsDown += 1;
  }

  const rowSegments: Array<{ row: number; startOffset: number; length: number }> = [];
  let logicalText = "";
  for (let row = startRow; row <= endRow; row += 1) {
    const line = buffer.getLine(row);
    const segment = line?.translateToString(row === endRow) ?? "";
    rowSegments.push({ row, startOffset: logicalText.length, length: segment.length });
    logicalText += segment;
  }

  const detected = detectTerminalLineLinks(logicalText, cwd);
  if (detected.length === 0) return undefined;

  const offsetToPosition = (offset: number): { x: number; y: number } => {
    for (let i = 0; i < rowSegments.length; i += 1) {
      const seg = rowSegments[i];
      if (offset < seg.startOffset + seg.length || i === rowSegments.length - 1) {
        return {
          x: Math.max(1, offset - seg.startOffset + 1),
          y: seg.row + 1,
        };
      }
    }
    return { x: 1, y: bufferLineNumber };
  };

  const results: ProvidedTerminalBufferLink[] = [];
  for (const item of detected) {
    if (item.endIndex <= item.startIndex) continue;
    const start = offsetToPosition(item.startIndex);
    const end = offsetToPosition(item.endIndex - 1);
    if (bufferLineNumber < start.y || bufferLineNumber > end.y) continue;
    results.push({
      text: item.text,
      range: { start, end },
      activate: () => {
        onOpenLink(item.target);
      },
    });
  }

  return results.length > 0 ? results : undefined;
}

