import {
  conversationItems, itemAtOrdinal, keepItemOrdinal, runningSubagentStreams, streamSubagentDelta,
  type ConvItem, type SubagentItem, type SubagentStream,
} from "./conversation-reducer";
import type { EngineEvent } from "../shared/engine";
import { liveEventOrdinal } from "./live-events";
import { equalValue } from "./stable-values";

/** Reuse values, never mutate the reducer's input or its previous result. */
export function retainConversationItems(previous: ConvItem[], next: ConvItem[]): ConvItem[] {
  const byId = new Map(previous.map(item => [`${item.kind}:${item.id}`, item]));
  const retained = next.map(item => {
    const before = byId.get(`${item.kind}:${item.id}`);
    if (before === undefined) return item;
    const candidate = item.kind === "subagent" && before.kind === "subagent"
      ? keepItemOrdinal({ ...item, children: retainConversationItems(before.children, item.children) },
        item) : item;
    return equalValue(before, candidate) ? before : candidate;
  });
  return retained.length === previous.length && retained.every((item, index) => item === previous[index])
    ? previous : retained;
}

/** A session's event log in, its conversation items out; see createConversationProjection. */
export type ConversationProjection = (events: EngineEvent[]) => ConvItem[];

type Delta = Extract<EngineEvent, { kind: "message.delta" }>;
const NO_STREAMS: ReadonlyMap<string, SubagentStream> = new Map();

/** The path of indices to the card `id` in `items`, newest first; null when absent. */
function cardPath(items: readonly ConvItem[], id: string): number[] | null {
  for (let index = items.length - 1; index >= 0; index--) {
    const item = items[index];
    if (item.kind !== "subagent") continue;
    if (item.id === id) return [index];
    const inner = cardPath(item.children, id);
    if (inner !== null) return [index, ...inner];
  }
  return null;
}

/** `items` with the card at `path` replaced by `update(card)`, every ancestor
 * copied; null when the update refuses. */
function replaceCard(items: readonly ConvItem[], path: readonly number[],
    update: (card: SubagentItem) => SubagentItem | null, depth = 0): ConvItem[] | null {
  const card = items[path[depth]] as SubagentItem;
  let updated: SubagentItem | null;
  if (depth === path.length - 1) updated = update(card);
  else {
    const children = replaceCard(card.children, path, update, depth + 1);
    updated = children === null ? null : { ...card, children };
  }
  if (updated === null) return null;
  const next = [...items];
  next[path[depth]] = keepItemOrdinal(updated, card);
  return next;
}

/** Streaming appends and coalesced tail replacement reuse the settled rows,
 * main-thread text and a running subagent's alike. Structural changes
 * (including out-of-order subagents, tool completion and compaction) go
 * through the full reducer, which remains the reference. */
export function createConversationProjection(): ConversationProjection {
  let previous: ConvItem[] = [];
  let before: EngineEvent[] = [];
  let lastRef: readonly EngineEvent[] = before;
  let hasSubagents = false;
  // Running cards after the last full reduction; their deltas stream in place.
  let streams = NO_STREAMS;
  return events => {
    if (events === lastRef && events.length === before.length
        && (events.length === 0 || events[events.length - 1] === before[before.length - 1])) {
      return previous;
    }
    let shared = 0;
    while (shared < before.length && shared < events.length && before[shared] === events[shared]) shared++;
    // Main-thread deltas land in the MAIN bucket past every card placement,
    // and deltas routed to a card never close MAIN's open run, so appending
    // matches the reducer's `text-${ordinal}` item. A delta of a running card
    // that the last reduction announced extends that card the same way (see
    // streamSubagentDelta). Anything else that touches a card takes the reducer.
    const mainThread = (event: EngineEvent | undefined): boolean => event?.kind === "message.delta"
      && (!hasSubagents || event.agentKey === undefined);
    const cardDelta = (event: EngineEvent | undefined): event is Delta & { agentKey: string } =>
      event?.kind === "message.delta" && hasSubagents && event.agentKey !== undefined
      && streams.has(event.agentKey);
    const replacing = before[shared];
    const replacement = events[shared];
    const replacingTail = shared === before.length - 1
      && (mainThread(replacing) || cardDelta(replacing)) && replacement?.kind === "message.delta"
      && (!hasSubagents || (replacing as Delta).agentKey === replacement.agentKey);
    const incremental = (shared === before.length || replacingTail)
      && events.length >= before.length && events.slice(shared).every(event =>
        mainThread(event) || cardDelta(event) || event.kind === "commands.available");
    let next: ConvItem[] | null = incremental ? [...previous] : null;
    // A delta that replaced the log's last one takes the old text back off its
    // run before the new one is appended, in whichever thread it streamed.
    let replaced = replacingTail ? (replacing as Delta) : null;
    for (let index = shared; next !== null && index < events.length; index++) {
      const event = events[index];
      if (event.kind !== "message.delta") continue;
      const id = `text-${liveEventOrdinal(event, index)}`;
      const old = replaced?.text;
      replaced = null;
      if (cardDelta(event)) {
        const stream = streams.get(event.agentKey)!;
        const path = cardPath(next, event.agentKey);
        next = path === null ? null
          : replaceCard(next, path, card => streamSubagentDelta(card, event.text, id, stream, old));
        continue;
      }
      let last = next.at(-1);
      if (old !== undefined && last?.kind === "text" && last.streaming) {
        last = keepItemOrdinal({ ...last, text: last.text.slice(0, last.text.length - old.length) }, last);
      }
      if (last?.kind === "text" && last.streaming) {
        next[next.length - 1] = keepItemOrdinal({ ...last, text: last.text + event.text }, last);
      } else {
        next.push(itemAtOrdinal({ kind: "text", id, text: event.text, streaming: true },
          liveEventOrdinal(event, index)));
      }
    }
    if (next !== null) {
      const settled = previous;
      previous = next.length === settled.length && next.every((item, index) => item === settled[index])
        ? settled : next;
    } else {
      previous = retainConversationItems(previous, conversationItems(events));
      hasSubagents = events.some(event => event.kind === "subagent.started");
      streams = hasSubagents ? runningSubagentStreams(previous, events) : NO_STREAMS;
    }
    // Some fixture/embedding callers append to the same array. Snapshot the
    // references so that append detection never depends on array immutability.
    lastRef = events;
    before = events.slice();
    return previous;
  };
}

const SHARED_PROJECTIONS = new Map<string, ConversationProjection>();
const MAX_SHARED_PROJECTIONS = 32;

/**
 * THE projection of one session's live log, shared by every view of it: the
 * pane, the subagent detail column and the Agent task's inspector. A streamed
 * frame is reduced once however many of them are open, and a projector called
 * again with the log it just saw returns the same items at once.
 *
 * Each caller hands it that session's CURRENT log (App's `state.events[key]`).
 * The projector remembers one previous log, so two callers alternating two
 * different logs through it would re-reduce on every call — which is why a
 * covered pane that holds an older log stops calling instead (see
 * ProjectedConversation). Correctness never depends on the sharing: any call
 * sequence returns what conversationItems would.
 *
 * Bounded, least recently used out first. An evicted projector keeps working
 * for whoever still holds it; the next lookup simply starts a cold one.
 */
export function conversationProjectionFor(sessionKey: string): ConversationProjection {
  let projection = SHARED_PROJECTIONS.get(sessionKey);
  if (projection === undefined) projection = createConversationProjection();
  else SHARED_PROJECTIONS.delete(sessionKey);
  SHARED_PROJECTIONS.set(sessionKey, projection);
  if (SHARED_PROJECTIONS.size > MAX_SHARED_PROJECTIONS) {
    const oldest = SHARED_PROJECTIONS.keys().next().value;
    if (oldest !== undefined) SHARED_PROJECTIONS.delete(oldest);
  }
  return projection;
}

/** Drops a session's shared projection with the rest of its client state. */
export function releaseConversationProjection(sessionKey: string): void {
  SHARED_PROJECTIONS.delete(sessionKey);
}

/**
 * True when `next` is `held` plus streamed text and nothing else: the same
 * events by reference, then message deltas, where the last held event may have
 * been a delta that live-events has since replaced with its longer merge. This
 * is the one change a covered pane may leave undrawn; anything else (a tool, an
 * approval, a status, a compaction) is a transition the pane reacts to.
 */
export function onlyStreamedTextSince(
  held: readonly EngineEvent[], next: readonly EngineEvent[],
): boolean {
  if (next === held) return true;
  if (next.length < held.length) return false;
  let shared = 0;
  while (shared < held.length && held[shared] === next[shared]) shared++;
  if (shared < held.length - 1) return false;
  if (shared === held.length - 1 && held[shared].kind !== "message.delta") return false;
  for (let index = shared; index < next.length; index++) {
    if (next[index].kind !== "message.delta") return false;
  }
  return true;
}
