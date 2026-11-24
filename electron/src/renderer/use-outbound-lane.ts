import { useEffect, useState } from "react";
import type { SessionKey } from "../shared/engine";
import { OutboundLane, type OutboundLanePort } from "./outbound-lane";
import type { SteeringQueueItem } from "./steering-queue";
import { useStoreState } from "./use-store-state";

/**
 * App's one OutboundLane. `createPort` and `loadQueues` run once, on the first
 * render, so every port member must stay valid for App's lifetime (read
 * changing values through refs).
 */
export function useOutboundLane(createPort: () => OutboundLanePort,
    loadQueues: () => Record<SessionKey, SteeringQueueItem[]>) {
  const [lane] = useState(() => new OutboundLane(createPort(), loadQueues()));
  const snapshot = useStoreState(lane);
  // A queue map storage refused gets one more write after the render that
  // shows it; every later accepted change writes the whole map anyway.
  useEffect(() => { lane.retryUnsavedQueues(); }, [lane, snapshot.queues]);
  return { lane, snapshot };
}
