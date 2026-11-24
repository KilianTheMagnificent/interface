/* ---- how a path is SHOWN -------------------------------------------------
   Pure string work, deliberately here rather than in the component: every rule
   below decides what text a row shows, and each one is a branch that has to be
   provable without a DOM. Nothing here touches the filesystem — a session's
   root is a string the shell hands over, it may name a directory that no longer
   exists, and a throw in this file takes the whole transcript down with it. */

const SEPARATORS = "/\\";

export const isSeparator = (ch: string): boolean => SEPARATORS.includes(ch);

/** "/repo/src/" -> "/repo/src". A lone "/" (or "C:\") keeps its separator. */
function withoutTrailingSeparators(value: string): string {
  let end = value.length;
  while (end > 1 && isSeparator(value[end - 1])) end -= 1;
  return value.slice(0, end);
}

/**
 * `target` expressed relative to `prefix`, or null when it is not under it.
 * The boundary check is the whole point: a bare startsWith turns
 * /…/InterfaceX/a.ts into X/a.ts, which is a path that never existed.
 */
function relativeUnder(target: string, prefix: string): string | null {
  if (!target.startsWith(prefix)) return null;
  const rest = target.slice(prefix.length);
  if (rest === "") return "";
  if (isSeparator(prefix[prefix.length - 1])) return withoutTrailingSeparators(rest);
  if (!isSeparator(rest[0])) return null;
  return withoutTrailingSeparators(rest.slice(1));
}

/** Trailing segment of a directory path: "/a/b/Interface" -> "Interface". */
export function lastSegment(value: string): string {
  const trimmed = withoutTrailingSeparators(value);
  for (let i = trimmed.length - 1; i >= 0; i -= 1) {
    if (isSeparator(trimmed[i])) return trimmed.slice(i + 1);
  }
  return trimmed;
}

/**
 * The user's home directory, DERIVED FROM THE ROOT rather than from the
 * environment — the renderer has no `process` and must never guess. A root of
 * /Users/ann/Desktop/Projects/Interface yields /Users/ann, which is what lets a
 * path in a SIBLING project shorten to ~/Desktop/… instead of staying absolute.
 */
function homeOf(root: string): string | null {
  const posix = /^(\/(?:Users|home)\/[^/\\]+)(?=[/\\]|$)/.exec(root);
  if (posix !== null) return posix[1];
  const windows = /^([A-Za-z]:[/\\]Users[/\\][^/\\]+)(?=[/\\]|$)/.exec(root);
  return windows === null ? null : windows[1];
}

/**
 * How a path is SHOWN. Claude Code's own rule (relative to the session root,
 * else ~/…, else absolute) — never a bare basename, and never cut mid-path,
 * because the identifying part of a path is spread across all of it.
 *
 * The absolute string stays reachable everywhere this is used: the row's title
 * attribute keeps it, and the find corpus indexes the raw value.
 *
 * Total by construction. No root, an empty root, a relative path, a path in
 * another tree: all return the input unchanged, which is exactly what this
 * transcript rendered before the root prop existed.
 */
export function displayPath(path: string, root?: string): string {
  if (typeof path !== "string" || path === "") return "";
  const base = withoutTrailingSeparators((root ?? "").trim());
  if (base === "") return path;
  const inside = relativeUnder(path, base);
  // "" means the path IS the root. An empty label is not an option, so the
  // directory names itself.
  if (inside !== null) return inside === "" ? lastSegment(base) : inside;
  const home = homeOf(base);
  if (home !== null) {
    const rest = relativeUnder(path, home);
    if (rest !== null && rest !== "") return `~${path[home.length]}${rest}`;
  }
  return path;
}

/**
 * A path as a terminal prints it: `~/rest` when it is inside a home directory,
 * otherwise unchanged. Reuses homeOf, so "which shapes are a home" is stated
 * once. No OS call and no IPC — the shape of the path IS the answer, which is
 * what keeps this pure and testable in the node suite.
 */
export function homeRelativePath(path: string): string {
  if (typeof path !== "string" || path === "") return "";
  // A cwd arrives with or without its trailing slash depending on who set it,
  // and `~/` is not how a terminal writes the home directory.
  const trimmed = withoutTrailingSeparators(path);
  const home = homeOf(trimmed);
  // `trimmed`, not `path`: the trailing separator is dropped for a path under
  // home and was kept for every other one, so "/opt/work/" and "/opt/work"
  // printed differently in the same column.
  if (home === null) return trimmed;
  const rest = trimmed.slice(home.length);
  return rest === "" ? "~" : `~${rest}`;
}

/* An absolute path sitting inside prose. The leading group is a delimiter, not
   part of the match, so "update /a/b.ts, update /c/d.ts" splits on the comma
   that changeList (src/engines/codex/mapper.ts) joins with. Deliberately narrow:
   a summary is engine-authored text and a blind replace over it would mangle
   sentences that merely mention a slash. */
const ABSOLUTE_IN_TEXT = /(^|[\s(,;:"'`[])((?:\/|[A-Za-z]:[/\\])[^\s,;:"'`)\]]+)/g;

/** Every absolute path inside a summary, in the order it appears. */
export function absolutePathsIn(text: string): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(ABSOLUTE_IN_TEXT)) found.push(match[2]);
  return found;
}

/**
 * displayPath over each absolute path in a summary; all other text is kept.
 * No root guard here on purpose — displayPath is already a no-op without one,
 * and a second check would be a branch no test could ever tell apart.
 */
export function shortenSummaryPaths(text: string, root?: string): string {
  return text.replace(ABSOLUTE_IN_TEXT,
    (_match, lead: string, path: string) => lead + displayPath(path, root));
}
