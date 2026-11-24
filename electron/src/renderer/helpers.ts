/**
 * The primitives that had grown byte-identical copies across the storage
 * boundaries, the popover geometry and the per-session maps. Nothing here
 * decides anything: it is a home for repeated one-liners, not a settings or
 * layout store.
 */

/** A plain object, or null for anything else — arrays and null included. */
export const asRecord = (value: unknown): Record<string, unknown> | null =>
  typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : null;

/** A trimmed, non-empty, length-bounded string, or null when the input is none of those. */
export function normalizedToken(value: unknown, maxChars: number): string | null {
  if (typeof value !== "string") return null;
  const token = value.trim();
  return token !== "" && token.length <= maxChars ? token : null;
}

/** Bound a value to [low, high], tolerating an inverted range. */
export const clamp = (value: number, low: number, high: number): number =>
  Math.min(Math.max(value, low), Math.max(low, high));

/** The map without `key`, or the same map when it has no such entry. */
export function withoutKey<T>(map: Record<string, T>, key: string): Record<string, T> {
  if (!(key in map)) return map;
  const out = { ...map };
  delete out[key];
  return out;
}
