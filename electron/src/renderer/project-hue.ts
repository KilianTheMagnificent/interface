/**
 * PROJECT HUES (owner, 2026-10-06, "hued glow"): every project gets one of
 * eight hue pairs, picked from its folder and kept, so a thread's project is
 * recognisable at a glance and the open thread's accents match it. The pick
 * is a pure function of the normalized project root — nothing is stored, a
 * project keeps its hue across launches and windows, and two windows agree.
 * The colours themselves live in ProjectHue.css as `light-dark()` pairs, so
 * every theme of either kind gets a readable variant.
 */

/** How many hue pairs ProjectHue.css defines (`.project-hue-0` … `-7`). */
export const PROJECT_HUE_COUNT = 8;

/** The class a plain chat (No project) takes: a neutral grey, no glow. */
export const NO_PROJECT_HUE_CLASS = "project-hue-none";

function normalizeRoot(root: string): string {
  return root.trim().replace(/\/+$/u, "").toLowerCase();
}

/** FNV-1a over the UTF-16 units: small, stable, and spreads short paths well. */
function fnv1a(text: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    hash ^= text.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

/** The project's hue index, or null for No project (an empty or absent root). */
export function projectHueIndex(root: string | null | undefined): number | null {
  if (root === null || root === undefined) return null;
  const normalized = normalizeRoot(root);
  if (normalized === "") return null;
  return fnv1a(normalized) % PROJECT_HUE_COUNT;
}

/** The CSS class that sets `--project-h` / `--project-h2` for this project. */
export function projectHueClass(root: string | null | undefined): string {
  const index = projectHueIndex(root);
  return index === null ? NO_PROJECT_HUE_CLASS : `project-hue-${index}`;
}

/** The letter a project tile shows: the name's first letter or digit, upper-cased; "·" without one. */
export function projectInitial(name: string | null | undefined): string {
  const match = /[\p{L}\p{N}]/u.exec(name ?? "");
  return match === null ? "·" : match[0].toLocaleUpperCase();
}

/** What a file-type badge says and which colour class it takes. */
export interface FileBadgeKind {
  label: string;
  kind: string;
}

const FILE_BADGES: Readonly<Record<string, FileBadgeKind>> = {
  ts: { label: "TS", kind: "ts" }, tsx: { label: "TSX", kind: "ts" }, mts: { label: "TS", kind: "ts" },
  js: { label: "JS", kind: "js" }, jsx: { label: "JSX", kind: "js" }, mjs: { label: "JS", kind: "js" },
  cjs: { label: "JS", kind: "js" },
  rs: { label: "RS", kind: "rs" },
  md: { label: "MD", kind: "md" }, mdx: { label: "MDX", kind: "md" },
  json: { label: "{ }", kind: "json" },
  css: { label: "CSS", kind: "css" }, scss: { label: "CSS", kind: "css" },
  html: { label: "HTML", kind: "html" },
  py: { label: "PY", kind: "py" }, go: { label: "GO", kind: "go" }, swift: { label: "SW", kind: "swift" },
  toml: { label: "CFG", kind: "cfg" }, yaml: { label: "CFG", kind: "cfg" }, yml: { label: "CFG", kind: "cfg" },
  sh: { label: "SH", kind: "sh" }, zsh: { label: "SH", kind: "sh" },
};

/** The badge for a path: by extension; an unknown one shows up to three letters in grey. */
export function fileBadgeFor(path: string): FileBadgeKind {
  const name = path.split("/").pop() ?? path;
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return { label: "·", kind: "other" };
  const extension = name.slice(dot + 1).toLowerCase();
  return FILE_BADGES[extension] ?? { label: extension.slice(0, 3).toUpperCase(), kind: "other" };
}

export type DiffstatBlock = "added" | "removed" | "neutral";

/**
 * GitHub's five squares: how a change splits between added and removed
 * lines, at a glance. A change of any size fills all five; one with lines on
 * a side shows at least one square for it. Zero lines draws five neutral.
 */
export function diffstatBlocks(added: number, removed: number, count = 5): DiffstatBlock[] {
  const total = Math.max(0, added) + Math.max(0, removed);
  if (total === 0) return Array.from({ length: count }, () => "neutral");
  let green = Math.round((Math.max(0, added) / total) * count);
  if (added > 0 && green === 0) green = 1;
  if (removed > 0 && green === count) green = count - 1;
  return Array.from({ length: count }, (_, index) => (index < green ? "added" : "removed"));
}
