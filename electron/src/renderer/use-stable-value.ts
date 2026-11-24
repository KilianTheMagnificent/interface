import { useCallback, useLayoutEffect, useMemo, useRef } from "react";
import { equalValue } from "./stable-values";

/** Identity-only cache; dropping it cannot change rendered values or behavior. */
export function useStableValue<T>(value: T): T {
  const previous = useRef(value);
  if (!equalValue(previous.current, value)) previous.current = value;
  return previous.current;
}

/** Stable event identity with the latest committed callback, never a stale render. */
export function useCommittedCallback<Args extends unknown[], Result>(callback: (...args: Args) => Result): (...args: Args) => Result {
  const committed = useRef(callback);
  useLayoutEffect(() => { committed.current = callback; });
  return useCallback((...args: Args) => committed.current(...args), []);
}

/** `useCommittedCallback` for a whole set of callbacks: one stable object whose
 * members call the latest committed ones. Its member names must not change. */
export function useCommittedActions<T extends Record<string, (...args: never[]) => unknown>>(actions: T): T {
  const committed = useRef(actions);
  useLayoutEffect(() => { committed.current = actions; });
  return useMemo(() => Object.fromEntries(Object.keys(committed.current).map(name =>
    [name, (...args: unknown[]) =>
      (committed.current[name] as (...args: unknown[]) => unknown)(...args)])) as unknown as T, []);
}
