/**
 * The effort vocabulary every effort surface shares: display names, the fixed
 * faster-to-smarter ladder, the top-tier rung that celebrates, and when a
 * range change commits. Pure; `components/EffortBar.tsx` draws it.
 */

const KNOWN_EFFORT_LABELS: Readonly<Record<string, string>> = {
  default: "Default",
  none: "None",
  minimal: "Minimal",
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
  ultra: "Ultra",
  ultracode: "Ultracode",
};

/** Humanize provider vocabulary without changing the value sent back to it. */
export function effortDisplayName(value: string): string {
  const clean = value.trim();
  if (clean === "") return "Effort";
  const known = KNOWN_EFFORT_LABELS[clean.toLocaleLowerCase()];
  if (known !== undefined) return known;
  const words = clean.replace(/[_-]+/g, " ");
  return `${words.charAt(0).toLocaleUpperCase()}${words.slice(1)}`;
}

/* Providers advertise their ladder in whatever order their catalogue happens
   to use — Grok's arrives smartest-first. The picker's axis is fixed (left is
   always faster), so the order is normalized here, in the one component every
   effort surface shares, rather than per caller or per adapter. */
const EFFORT_LADDER = [
  "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
  "ultracode",
] as const;

/** Canonical faster→smarter order; unknown vocabulary keeps provider order, last. */
export function orderEffortLevels(levels: readonly string[]): string[] {
  const rank = (level: string): number => {
    const at = EFFORT_LADDER.indexOf(
      level.trim().toLocaleLowerCase() as typeof EFFORT_LADDER[number]);
    return at < 0 ? EFFORT_LADDER.length : at;
  };
  return [...levels].sort((a, b) => rank(a) - rank(b));
}

const TOP_TIER = new Set(["max", "ultra", "ultracode"]);

/** The one rung that earns the accent/starfield treatment: the ladder's LAST
 *  level, and only when it is a named top tier. Max stays plain on a ladder
 *  that continues past it (Claude: … max < ultracode; Codex: … max < ultra). */
export function topTierLevel(levels: readonly string[]): string | null {
  const last = levels[levels.length - 1];
  return last !== undefined && TOP_TIER.has(last.trim().toLocaleLowerCase())
    ? last : null;
}

/** Last rung, when it is a named top tier or Grok's xhigh. Hairlines and the
 *  moving glyph play only on the transition onto this rung — not while it sits. */
export function effortLevelCelebrates(level: string, levels: readonly string[]): boolean {
  if (level === "") return false;
  if (level === topTierLevel(levels)) return true;
  return levels[levels.length - 1] === level
    && level.trim().toLocaleLowerCase() === "xhigh";
}

/** One-shot window: play when the committed value lands on a celebratory
 *  rung from a quieter one. Mount (previous === null) never celebrates. */
export function shouldCelebrateEffortChange(
  previous: string | null,
  next: string,
  levels: readonly string[],
): boolean {
  if (previous === null || previous === next) return false;
  return effortLevelCelebrates(next, levels)
    && !effortLevelCelebrates(previous, levels);
}

export type EffortInputInteraction = "pointer" | "keyboard" | null;

/**
 * Assistive-technology range actions fire a value change without a matching
 * pointer or keyboard boundary. Those changes must commit immediately; direct
 * manipulation still commits once on release so dragging never floods IPC.
 */
export function effortChangeCommitsImmediately(
  interaction: EffortInputInteraction,
): boolean {
  return interaction === null;
}
