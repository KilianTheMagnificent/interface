import { useLayoutEffect, useState } from "react";

/** A plain controller App reads: a snapshot and a change subscription. */
export interface SnapshotStore<T> {
  snapshot(): T;
  subscribe(listener: () => void): () => void;
}

/**
 * A controller's snapshot mirrored into ordinary React state, rather than read
 * with useSyncExternalStore: a change then renders at the same priority and
 * batching as the setState calls the controller replaced in App, which the
 * effects that follow those renders were written against.
 */
export function useStoreState<T>(store: SnapshotStore<T>): T {
  const [state, setState] = useState(() => store.snapshot());
  useLayoutEffect(() => {
    const unsubscribe = store.subscribe(() => { setState(store.snapshot()); });
    setState(store.snapshot());
    return unsubscribe;
  }, [store]);
  return state;
}
