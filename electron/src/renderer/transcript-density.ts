import type { TranscriptRow } from "./transcript-merge";

/**
 * The transcript density dial: its steps, the per-session store and what each
 * step folds.
 */

/* ==========================================================================
   TRANSCRIPT DENSITY — how much of a tool-heavy turn one tile has to paint.

   The tiled workspace is the whole point of this app, and a single verbose
   agent floods its pane: the audit's 383px tile cannot show three agents'
   reasoning while one of them is printing forty Read rows. Tool cards were
   already individually collapsible and already defaulted closed, so what was
   missing was the GLOBAL dial — one control that folds or unfolds the lot.

   Three steps, and each one is a real difference in what is on screen rather
   than three names for the same view:

     compact   every card folded, and EVERY stretch of work folds into one work
               line — including the run of one and the stretch of pure edits
               that balanced exempts. Summaries only.
     balanced  every card folded, and a stretch folds only where folding removes
               repetition: a lone call and a plain file change keep their own
               named rows. The shipped default, so an untouched session behaves
               exactly as it did before this existed.
     detailed  every card unfolded, and runs never form at all, so each call is
               its own open card with its arguments and result showing.

   The dial is a mode AND an action: moving it re-seeds every card's own fold,
   which is what makes "collapse all" / "expand all" out of a three-way switch.
   A card the user toggles afterwards still wins, until the dial moves again.
   ========================================================================== */

export type TranscriptDensity = "compact" | "balanced" | "detailed";

/** Absence means this. A session that was never touched must not read as one
 *  the user configured, and it must render exactly as it did before. */
export const DEFAULT_TRANSCRIPT_DENSITY: TranscriptDensity = "balanced";

/** localStorage key, alongside the sidebar's own view preferences. */
export const TRANSCRIPT_DENSITY_KEY = "interface.transcript-density";

/**
 * How many sessions' choices are kept. The map only ever holds sessions whose
 * density is NOT the default (see nextTranscriptDensities), so this ceiling is
 * reached only by a user who really has configured that many panes; the oldest
 * entries are dropped first, in insertion order.
 */
export const TRANSCRIPT_DENSITY_MAX_SESSIONS = 100;

/** sessionKey -> that session's dial. A missing key IS the default. */
export type TranscriptDensityMap = Readonly<Record<string, TranscriptDensity>>;

const NO_DENSITIES: TranscriptDensityMap = {};

export function isTranscriptDensity(value: unknown): value is TranscriptDensity {
  return value === "compact" || value === "balanced" || value === "detailed";
}

/**
 * Tolerant localStorage boundary, same rule as the sidebar's collapse memory:
 * a corrupt, hand-edited or foreign value degrades to "nothing configured"
 * rather than white-screening a pane. Only recognised densities survive, so a
 * value written by a future version with a fourth step is ignored rather than
 * rendered as an unknown mode.
 */
export function parseTranscriptDensities(raw: string | null): TranscriptDensityMap {
  if (raw === null) return NO_DENSITIES;
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { return NO_DENSITIES; }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    return NO_DENSITIES;
  }
  const out: Record<string, TranscriptDensity> = {};
  let kept = 0;
  for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (key === "" || !isTranscriptDensity(value)) continue;
    if (value === DEFAULT_TRANSCRIPT_DENSITY) continue;   // absence says this already
    if (kept >= TRANSCRIPT_DENSITY_MAX_SESSIONS) break;
    out[key] = value;
    kept += 1;
  }
  return out;
}

/** What `sessionKey` is set to, which for most sessions is nothing at all. */
export function transcriptDensityFor(
  densities: TranscriptDensityMap, sessionKey: string,
): TranscriptDensity {
  const stored: unknown = densities[sessionKey];
  return isTranscriptDensity(stored) ? stored : DEFAULT_TRANSCRIPT_DENSITY;
}

/**
 * The map after one pane's dial moves. Setting a session back to the default
 * REMOVES it rather than storing the word: absence and "balanced" are the same
 * state, and storing both would let the file grow one entry per session ever
 * opened while saying nothing extra.
 *
 * A pane with no sessionKey (the `key={sessionKey}` remount route) is kept in
 * memory under "" so its control still works, and dropped at serialization —
 * persisting it would hand one pane's choice to every keyless pane after it.
 */
export function nextTranscriptDensities(
  current: TranscriptDensityMap, sessionKey: string, density: TranscriptDensity,
): TranscriptDensityMap {
  const next: Record<string, TranscriptDensity> = { ...current };
  delete next[sessionKey];                    // re-inserted below as the newest
  if (density !== DEFAULT_TRANSCRIPT_DENSITY) next[sessionKey] = density;
  const keys = Object.keys(next);
  for (let i = 0; i < keys.length - TRANSCRIPT_DENSITY_MAX_SESSIONS; i += 1) {
    delete next[keys[i]];
  }
  return next;
}

/** What goes to disk. "" is this run's keyless pane and is never written. */
export function serializeTranscriptDensities(densities: TranscriptDensityMap): string {
  const out: Record<string, TranscriptDensity> = {};
  for (const [key, value] of Object.entries(densities)) {
    if (key === "" || value === DEFAULT_TRANSCRIPT_DENSITY) continue;
    out[key] = value;
  }
  return JSON.stringify(out);
}

/**
 * The `minimum` handed to collapseSuccessfulToolRuns — how many rows a stretch
 * of work has to reach before it folds into one line.
 *
 * Both ends are deliberately unreachable numbers rather than thresholds.
 * Infinity is not a trick: a run has to reach it to fold, so no run ever does
 * and every call keeps its own row — which is exactly what "detailed" means.
 * ZERO is the mirror image, and it is why compact is no longer balanced under a
 * second name: a run always has at least one member, so 0 and 1 both clear the
 * length test, and what 0 says on top of that is "no minimum at all". The fold
 * pass reads it as licence to drop the exemptions balanced keeps — the lone
 * inspection call and the stretch of pure edits — so at compact every run
 * folds, including a run of one.
 */
export function toolRunMinimum(density: TranscriptDensity): number {
  if (density === "detailed") return Number.POSITIVE_INFINITY;
  return density === "compact" ? 0 : 1;
}

/** The fold every card is re-seeded to when the dial moves. */
export function transcriptDensityOpensItems(density: TranscriptDensity): boolean {
  return density === "detailed";
}

/**
 * Whether this transcript has anything the dial would change. A pane holding
 * nothing but prose gets no control at all — a toolbar whose three buttons all
 * produce the same screen is chrome charging rent for nothing.
 *
 * Top-level rows only: a subagent card IS foldable, so a transcript whose only
 * tool calls are nested inside one already qualifies through the card itself.
 */
export function transcriptHasFoldableRows(rows: readonly TranscriptRow[]): boolean {
  for (const row of rows) {
    if (row.kind !== "item") continue;
    if (row.item.kind === "tool" || row.item.kind === "subagent") return true;
  }
  return false;
}
