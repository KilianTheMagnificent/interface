import type { ProjectFileEntry } from "../../shared/ipc";

/**
 * The command palette's file ranker.
 *
 * It was Quick Open's, in a second 573-line dialog that was the palette with
 * one dataset: same 680px width, same 640px cap, same scrim, same
 * combobox-over-listbox, same key model. The dialog is gone and Files is a
 * group in the palette; the ranking and the match positions live here, as pure
 * functions, because that is the part worth keeping and the part worth testing.
 */

export interface RankedPaletteFile {
  file: ProjectFileEntry;
  score: number;
}

export const PALETTE_FILE_MAX_CANDIDATES = 10_000;
export const PALETTE_FILE_MAX_RESULTS = 100;
export const PALETTE_QUERY_MAX_CHARS = 256;

const normalizePath = (value: string): string =>
  value.replaceAll("\\", "/").replace(/^\.\//, "").toLowerCase();

export function paletteQueryTerms(rawQuery: string): string[] {
  const query = rawQuery.slice(0, PALETTE_QUERY_MAX_CHARS).trim().toLowerCase();
  return query === "" ? [] : query.split(/\s+/).filter(Boolean);
}

const CLUTTER_DIRECTORIES = new Set([
  ".cache", ".direnv", ".git", ".next", ".output", ".parcel-cache", ".svelte-kit",
  ".superpowers", ".turbo", ".vite", "build", "coverage", "deriveddata", "dist",
  "dist-main", "dist-renderer", "node_modules", "out", "release",
]);
const CLUTTER_FILES = new Set([".ds_store", ".eslintcache", "thumbs.db"]);

function isClutterNormalized(normalized: string): boolean {
  const segments = normalized.split("/").filter(Boolean);
  const leaf = segments.at(-1) ?? "";
  return CLUTTER_FILES.has(leaf)
    || segments.slice(0, -1).some(segment => CLUTTER_DIRECTORIES.has(segment));
}

/** Defensive renderer filter; the main-process index applies the same policy first. */
export function isClutterPath(value: string): boolean {
  return isClutterNormalized(normalizePath(value));
}

export function pathDirectory(value: string): string {
  const normalized = value.replaceAll("\\", "/");
  const slash = normalized.lastIndexOf("/");
  return slash < 0 ? "" : normalized.slice(0, slash);
}

const isBoundary = (candidate: string, index: number): boolean =>
  index === 0 || "/\\._- ".includes(candidate[index - 1]);

function subsequenceScore(candidate: string, term: string, basenameStart: number): number | null {
  let previous = -1;
  let score = term.length * 10;
  for (const character of term) {
    const index = candidate.indexOf(character, previous + 1);
    if (index < 0) return null;
    if (index === previous + 1) score += 16;
    else if (previous >= 0) score -= Math.min(index - previous - 1, 12);
    if (isBoundary(candidate, index)) score += 22;
    if (index >= basenameStart) score += 5;
    previous = index;
  }
  return score;
}

function matchIndicesOn(candidate: string, terms: readonly string[]): number[] {
  const matched = new Set<number>();
  for (const term of terms) {
    let previous = -1;
    for (const character of term) {
      const index = candidate.indexOf(character, previous + 1);
      if (index < 0) return [];
      matched.add(index);
      previous = index;
    }
  }
  return [...matched].sort((a, b) => a - b);
}

/** Character positions used to show why a fuzzy result matched. */
export function fuzzyPathMatchIndices(relativePath: string, rawQuery: string): number[] {
  return matchIndicesOn(normalizePath(relativePath), paletteQueryTerms(rawQuery));
}

/**
 * Greedy subsequence scoring tuned for file paths: contiguous characters,
 * path/word boundaries, and basename prefixes win over scattered directory
 * matches. Whitespace separates terms; every term must match.
 */
function fuzzyPathScoreNormalized(candidate: string, terms: readonly string[]): number | null {
  if (terms.length === 0) return 0;
  const basenameStart = candidate.lastIndexOf("/") + 1;
  const basename = candidate.slice(basenameStart);
  let score = 0;

  for (const term of terms) {
    const termScore = subsequenceScore(candidate, term, basenameStart);
    if (termScore === null) return null;
    score += termScore;
    if (candidate === term) score += 140;
    else if (candidate.startsWith(term)) score += 35;
    if (basename === term) score += 110;
    else if (basename.startsWith(term)) score += 65;
    else if (basename.includes(term)) score += 25;
  }

  // A short path wins a close tie without overwhelming match quality.
  return score - Math.min(Math.floor(candidate.length / 4), 60);
}

export function fuzzyPathScore(relativePath: string, rawQuery: string): number | null {
  return fuzzyPathScoreNormalized(normalizePath(relativePath), paletteQueryTerms(rawQuery));
}

/** One candidate of an index: the IPC entry and its normalized path. */
export interface IndexedPaletteFile {
  readonly file: ProjectFileEntry;
  readonly normalized: string;
}

/**
 * The candidates every query is scored against: the first
 * PALETTE_FILE_MAX_CANDIDATES entries, normalized once, clutter removed. Built
 * once per file list, so a keystroke only scores.
 */
export function indexPaletteFiles(files: readonly ProjectFileEntry[]): IndexedPaletteFile[] {
  const index: IndexedPaletteFile[] = [];
  const candidateCount = Math.min(files.length, PALETTE_FILE_MAX_CANDIDATES);
  for (let at = 0; at < candidateCount; at += 1) {
    const file = files[at];
    const normalized = normalizePath(file.relativePath);
    if (!isClutterNormalized(normalized)) index.push({ file, normalized });
  }
  return index;
}

/** What `localeCompare` without arguments compares with, built once. */
const pathCollator = new Intl.Collator();

/** Higher score first; equal scores in path order. */
const compareRanked = (a: RankedPaletteFile, b: RankedPaletteFile): number =>
  b.score - a.score || pathCollator.compare(a.file.relativePath, b.file.relativePath);

/** An empty query scores every candidate 0, so it has one answer per index. */
const unqueriedRanks = new WeakMap<readonly IndexedPaletteFile[], RankedPaletteFile[]>();

/**
 * Score an index against a query and keep the best `limit`, in exactly the
 * order a stable sort of every match gives: a candidate is placed after every
 * kept one it does not rank strictly above, and one that ranks no better than
 * the last of a full list is dropped.
 *
 * Quick Open's `recentPaths` weighting is deliberately NOT here. Nothing ever
 * passed it — the dialog took the prop and App never supplied one — so it was
 * an API advertising a feature the app did not have.
 */
export function rankPaletteIndex(
  index: readonly IndexedPaletteFile[],
  query: string,
  limit = PALETTE_FILE_MAX_RESULTS,
): RankedPaletteFile[] {
  const boundedLimit = Math.max(0, Math.min(
    PALETTE_FILE_MAX_RESULTS,
    Number.isFinite(limit) ? Math.floor(limit) : PALETTE_FILE_MAX_RESULTS,
  ));
  if (boundedLimit === 0) return [];

  const terms = paletteQueryTerms(query);
  const unqueried = terms.length === 0;
  const cached = unqueried ? unqueriedRanks.get(index) : undefined;
  if (cached !== undefined) return cached.slice(0, boundedLimit);
  // The unqueried answer is kept at full length, so any limit can slice it.
  const keep = unqueried ? PALETTE_FILE_MAX_RESULTS : boundedLimit;
  const ranked: RankedPaletteFile[] = [];
  for (const { file, normalized } of index) {
    const fuzzy = fuzzyPathScoreNormalized(normalized, terms);
    if (fuzzy === null) continue;
    const entry = { file, score: fuzzy };
    if (ranked.length === keep && compareRanked(entry, ranked[keep - 1]) >= 0) continue;
    let low = 0;
    let high = ranked.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (compareRanked(entry, ranked[middle]) < 0) high = middle;
      else low = middle + 1;
    }
    ranked.splice(low, 0, entry);
    if (ranked.length > keep) ranked.pop();
  }
  if (!unqueried) return ranked;
  unqueriedRanks.set(index, ranked);
  return ranked.slice(0, boundedLimit);
}

/** Rank a bounded candidate set without mutating the IPC-owned file array. */
export function rankPaletteFiles(
  files: readonly ProjectFileEntry[],
  query: string,
  limit = PALETTE_FILE_MAX_RESULTS,
): RankedPaletteFile[] {
  return rankPaletteIndex(indexPaletteFiles(files), query, limit);
}
