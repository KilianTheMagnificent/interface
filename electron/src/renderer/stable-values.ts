/** Structural equality for immutable renderer projections, not canonical stores. */
export function equalValue(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true;
  if (left === null || right === null || typeof left !== "object" || typeof right !== "object") return false;
  if (Object.getPrototypeOf(left) !== Object.getPrototypeOf(right)) return false;
  if (Array.isArray(left) && Array.isArray(right) && left.length !== right.length) return false;
  if (left instanceof Set && right instanceof Set) {
    return left.size === right.size && [...left].every(value => right.has(value));
  }
  if (left instanceof Map && right instanceof Map) {
    return left.size === right.size && [...left].every(([key, value]) => right.has(key) && equalValue(value, right.get(key)));
  }
  if (!Array.isArray(left) && Object.getPrototypeOf(left) !== Object.prototype
      && Object.getPrototypeOf(left) !== null) return false;
  const keys = Object.keys(left);
  const a = left as Record<string, unknown>;
  const b = right as Record<string, unknown>;
  return keys.length === Object.keys(right).length
    && keys.every(key => Object.hasOwn(b, key) && equalValue(a[key], b[key]));
}

/**
 * `next`, with every item that equals the previous item of the same key
 * replaced by that previous object, and `previous` itself when every item is
 * kept in order. A rebuilt list then hands memoized children only the items
 * that changed. Identity-only: dropping it cannot change what is rendered.
 */
export function retainEqualItems<T>(previous: readonly T[], next: readonly T[],
    key: (item: T) => string): readonly T[] {
  const byKey = new Map(previous.map(item => [key(item), item]));
  let same = previous.length === next.length;
  const retained = next.map((item, index) => {
    const kept = byKey.get(key(item));
    const value = kept !== undefined && equalValue(kept, item) ? kept : item;
    if (value !== previous[index]) same = false;
    return value;
  });
  return same ? previous : retained;
}
