import type { OutputPreviewSnapshot } from "../shared/output-preview";

import type { OutputAnchor } from "../shared/output-preview";
export type { OutputAnchor } from "../shared/output-preview";

export function outputKey(root: string, path: string): string { return JSON.stringify([root, path]); }
export function outputPath(snapshot: Pick<OutputPreviewSnapshot, "root" | "path">): string {
  return `${snapshot.root.replace(/\/$/, "")}/${snapshot.path}`;
}
export function outputTextAnchor(text: string, quote: string): OutputAnchor | null {
  const bounded = quote.trim().slice(0, 2000);
  if (!bounded) return null;
  const at = text.indexOf(bounded);
  if (at < 0 || text.indexOf(bounded, at + 1) >= 0) return { kind: "text", quote: bounded };
  const startLine = text.slice(0, at).split("\n").length;
  return { kind: "text", quote: bounded, startLine, endLine: startLine + bounded.split("\n").length - 1 };
}
export function outputAnchorLabel(anchor: OutputAnchor): string {
  switch (anchor.kind) {
    case "file": return "Whole file";
    case "page": return `Page ${anchor.page.toString()}`;
    case "text": return anchor.startLine === undefined ? "Selected text"
      : anchor.startLine === anchor.endLine ? `Line ${anchor.startLine.toString()}`
        : `Lines ${anchor.startLine.toString()}–${anchor.endLine?.toString() ?? ""}`;
    case "region": return `Image region: x ${Math.round(anchor.x * 100).toString()}%, y ${Math.round(anchor.y * 100).toString()}%, width ${Math.round(anchor.width * 100).toString()}%, height ${Math.round(anchor.height * 100).toString()}%`;
  }
}
/** A review draft always names the immutable view, never a later disk version. */
export function outputFeedback(snapshot: OutputPreviewSnapshot, anchor: OutputAnchor, note: string): string {
  return [
    `Please revise ${JSON.stringify(outputPath(snapshot))}.`,
    `Reviewed version (SHA-256): ${snapshot.revision ?? "unavailable"}`,
    "Check the file's current hash before editing. If it differs, reconcile this feedback with the reviewed version first.",
    `Location: ${outputAnchorLabel(anchor)}.`,
    ...(anchor.kind === "text" ? [`Selected text: ${JSON.stringify(anchor.quote)}`] : []),
    ...(anchor.kind === "region" ? [`Image coordinates (0–1 from top left): ${JSON.stringify(anchor)}`] : []),
    "", note.trim(),
  ].join("\n");
}
export function outputImageUrl(mediaType: string, bytes: readonly number[]): string {
  if (!["image/png", "image/jpeg", "image/gif", "image/webp"].includes(mediaType)) return "";
  let binary = "";
  for (let at = 0; at < bytes.length; at += 8192) binary += String.fromCharCode(...bytes.slice(at, at + 8192));
  return `data:${mediaType};base64,${btoa(binary)}`;
}
export function outputRegion(start: { x: number; y: number }, end: { x: number; y: number }): OutputAnchor {
  const clamp = (n: number) => Math.round(Math.max(0, Math.min(1, n)) * 10000) / 10000;
  const x1 = clamp(start.x), y1 = clamp(start.y), x2 = clamp(end.x), y2 = clamp(end.y);
  return { kind: "region", x: Math.min(x1, x2), y: Math.min(y1, y2), width: clamp(Math.abs(x2 - x1)), height: clamp(Math.abs(y2 - y1)) };
}

/** The outputs a thread offers Preview for (review B5): PDFs, raster images and Markdown. */
const PREVIEWABLE = /\.(pdf|png|jpe?g|gif|webp|md|markdown)$/iu;

/** Whether `path` is an output the preview draws (a PDF, an image, Markdown). */
export function isPreviewableOutput(path: string): boolean {
  return PREVIEWABLE.test(path.trim());
}

/**
 * Where a thread's file previews from: `root` (the thread's folder, which Rust
 * approved) and the path relative to it. A relative path is the repository's,
 * which is the thread's folder; an absolute one must lie inside `root`. Null
 * for anything else (outside the folder, a traversal, not previewable): the
 * preview reads only approved roots, and Rust checks again.
 */
export function outputPreviewTarget(path: string, root: string): { root: string; path: string } | null {
  const trimmed = path.trim();
  const base = root.replace(/\/+$/u, "");
  if (trimmed === "" || base === "" || !isPreviewableOutput(trimmed)) return null;
  let relative = trimmed;
  if (trimmed.startsWith("/")) {
    if (!trimmed.startsWith(`${base}/`)) return null;
    relative = trimmed.slice(base.length + 1);
  }
  relative = relative.replace(/^\.\//u, "");
  if (relative === "" || relative.split("/").some(part => part === ".." || part === "")) return null;
  return { root: base, path: relative };
}

function hasUnsafePathSegments(value: string): boolean {
  if (value === "" || value.length > 4096 || value.includes("\\")) return true;
  for (let i = 0; i < value.length; i += 1) {
    const code = value.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return true;
  }
  return value.split("/").some(part => part === ".." || part === ".");
}

/**
 * Resolves an attached file path for in-app preview. Files inside `projectRoot`
 * stay relative to that project root; attached files from elsewhere (e.g.
 * Desktop, Downloads, or `pasted-images`, which Rust authorizes via exact file
 * grants) resolve against their own parent directory.
 */
export function attachmentPreviewTarget(
  path: string,
  projectRoot?: string | null,
): { root: string; path: string } | null {
  const trimmed = path.trim();
  if (hasUnsafePathSegments(trimmed)) return null;
  const base = (projectRoot ?? "").trim().replace(/\/+$/u, "");
  if (!trimmed.startsWith("/")) {
    if (!base.startsWith("/") || hasUnsafePathSegments(base)) return null;
    const relative = trimmed.replace(/^\.\//u, "");
    if (relative === "" || relative.split("/").some(part => part === "" || part === ".." || part === ".")) {
      return null;
    }
    return { root: base, path: relative };
  }
  if (base.startsWith("/") && !hasUnsafePathSegments(base) && trimmed.startsWith(`${base}/`)) {
    const relative = trimmed.slice(base.length + 1);
    if (relative === "" || relative.split("/").some(part => part === "" || part === ".." || part === ".")) {
      return null;
    }
    return { root: base, path: relative };
  }
  const slash = trimmed.lastIndexOf("/");
  if (slash <= 0 || slash === trimmed.length - 1) return null;
  const root = trimmed.slice(0, slash);
  const file = trimmed.slice(slash + 1);
  if (root.split("/").slice(1).some(part => part === "") || file.includes("/")) return null;
  return { root, path: file };
}

const INLINE_IMAGE_URL = /^data:(image\/(?:png|jpeg|gif|webp));base64,([A-Za-z0-9+/]+=*)$/u;

/** Builds an in-memory image preview snapshot from a validated `data:` URL. */
export function inlineImageSnapshot(
  root: string,
  path: string,
  dataUrl: string,
): OutputPreviewSnapshot | null {
  const match = INLINE_IMAGE_URL.exec(dataUrl.trim());
  if (!match) return null;
  try {
    const binary = atob(match[2]);
    const bytes = new Array<number>(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    return {
      root,
      path,
      byteLength: bytes.length,
      revision: null,
      content: { kind: "image", mediaType: match[1], bytes },
    };
  } catch {
    return null;
  }
}

/** Builds an in-memory text/diff preview snapshot for an attached diff or snippet. */
export function inlineTextSnapshot(
  root: string,
  path: string,
  text: string,
  format: "plain" | "markdown" | "diff" = "plain",
): OutputPreviewSnapshot {
  return {
    root,
    path,
    byteLength: new TextEncoder().encode(text).length,
    revision: null,
    content: { kind: "text", text, format },
  };
}
