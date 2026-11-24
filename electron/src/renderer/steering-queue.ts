import {
  normalizePromptContextItems,
  type PromptContextItem,
} from "./prompt-context";
import { parseProviderCommandLike } from "../shared/provider-commands";
import { eventEndsTurn, eventFailsTurn } from "./turn-events";

export const DEFAULT_STEERING_QUEUE_LIMIT = 20;
export const MAX_STEERING_QUEUE_LIMIT = 50;
export const MAX_STEERING_QUEUE_SESSIONS = 200;

export interface SteeringQueueItem {
  id: string;
  text: string;
  /** Typed context captured with this prompt; never expanded in the queue UI. */
  context?: PromptContextItem[];
  /** Epoch milliseconds, used only for stable caller-owned metadata. */
  createdAt?: number;
  /**
   * Persisted before transmission. A restored in-flight row is never
   * auto-delivered because Interface cannot know whether the provider accepted
   * it before the prior process exited.
   */
  deliveryState?: "inflight" | "paused";
}

export interface SteeringQueueAutoDeliveryState {
  dispatching: boolean;
  waitingForTurn: boolean;
  hasError: boolean;
  live: boolean;
  status: string | undefined;
  /** Restored in-flight and explicitly paused rows require a user decision. */
  firstDeliveryState?: SteeringQueueItem["deliveryState"];
  /**
   * The next row is open in the queue's editor. Delivery holds at it until
   * the owner saves or cancels, so the unedited words never leave behind it.
   */
  firstBeingEdited?: boolean;
}

/**
 * Automatic draining is intentionally conservative. In particular, a failed
 * validation/send leaves the item parked until the user explicitly retries;
 * otherwise an idle session would spin on the same rejected item forever.
 */
export function steeringQueueCanAutoDeliver(queueLength: number,
    state: SteeringQueueAutoDeliveryState): boolean {
  return queueLength > 0 && !state.dispatching && !state.waitingForTurn
    && state.firstDeliveryState === undefined && state.firstBeingEdited !== true
    && !state.hasError && state.live && state.status === "idle";
}

/**
 * Claude does not emit `status.changed: working` for every follow-up. Observable
 * response activity is equally strong evidence that the accepted outbound turn
 * has started, and lets the subsequent idle edge advance the queue.
 */
export function steeringQueueEventStartsTurn(event: {
  kind: string;
  status?: string;
  active?: boolean;
}): boolean {
  if (event.kind === "status.changed") return event.status === "working";
  if (event.kind === "thinking.signal") return event.active === true;
  return event.kind === "message.delta" || event.kind === "tool.call"
    || event.kind === "tool.result" || event.kind === "approval.requested"
    || event.kind === "diff.produced" || event.kind === "subagent.started"
    || event.kind === "subagent.finished";
}

/** A turn-ending event (turn-events.ts) settles an in-flight steering send. */
export const steeringQueueEventEndsTurn = eventEndsTurn;

/**
 * The subset of turn-ending events that end it BADLY — a dead session, a failed
 * turn, or a non-retryable error — as opposed to a clean idle. A clean idle lets
 * the queue drain; a bad end must durably pause it so nothing is auto-delivered
 * into a broken or resurrected session, and so the user is shown a reason and a
 * recovery path instead of rows that look identical to healthy pending ones.
 */
export const steeringQueueTerminalFailure = eventFailsTurn;

/**
 * Capture only lifecycle edges emitted after a provider transmission has been
 * armed. Keeping this boundary explicit prevents an old turn's terminal edge,
 * observed during asynchronous context validation, from settling a later send.
 */
export function captureOutboundSendEvent<T extends {
  kind: string;
  status?: string;
  retryable?: boolean;
}>(buffer: { attempt: number; events: T[] } | undefined,
    activeAttempt: number | undefined, event: T): void {
  if (buffer === undefined || activeAttempt !== buffer.attempt) return;
  if (event.kind === "status.changed" && event.status === "working"
      || steeringQueueEventEndsTurn(event)) {
    buffer.events.push(event);
  }
}

/**
 * A provider may emit working + terminal from inside send() before Electron
 * resolves the invoke. Replay that bounded sequence at acceptance so the
 * renderer does not arm a latch after its only terminal edge has passed.
 */
export function outboundSettledDuringSend(events: readonly {
  kind: string;
  status?: string;
  retryable?: boolean;
}[]): boolean {
  let started = false;
  let settled = false;
  for (const event of events) {
    // A closed session is terminal regardless of any malformed later input.
    if (event.kind === "session.closed") return true;
    if (event.kind === "status.changed" && event.status === "working") {
      started = true;
      settled = false;
    } else if (started && steeringQueueEventEndsTurn(event)) {
      settled = true;
    }
  }
  return settled;
}

export type SteeringQueueMove = "up" | "down" | "first" | "last";

export function steeringQueueLimit(requested = DEFAULT_STEERING_QUEUE_LIMIT): number {
  if (!Number.isFinite(requested)) return DEFAULT_STEERING_QUEUE_LIMIT;
  return Math.min(MAX_STEERING_QUEUE_LIMIT, Math.max(1, Math.floor(requested)));
}

function promptContextCandidates(value: unknown): PromptContextItem[] {
  if (!Array.isArray(value)) return [];
  return value.filter((candidate): candidate is PromptContextItem => {
    if (candidate === null || typeof candidate !== "object" || Array.isArray(candidate)) {
      return false;
    }
    const item = candidate as Record<string, unknown>;
    return typeof item.id === "string" && typeof item.kind === "string"
      && typeof item.label === "string" && typeof item.value === "string"
      && (item.detail === undefined || typeof item.detail === "string")
      && (item.safeContent === undefined || typeof item.safeContent === "string")
      && (item.estimatedTokens === undefined || typeof item.estimatedTokens === "number");
  });
}

function normalizeItem(item: unknown): SteeringQueueItem | null {
  if (item === null || typeof item !== "object" || Array.isArray(item)) return null;
  const candidate = item as Record<string, unknown>;
  if (typeof candidate.id !== "string" || typeof candidate.text !== "string") return null;
  const context = normalizePromptContextItems(promptContextCandidates(candidate.context));
  const createdAt = candidate.createdAt;
  const deliveryState = candidate.deliveryState === "inflight"
      || candidate.deliveryState === "paused"
    ? candidate.deliveryState : undefined;
  const id = candidate.id.trim();
  const text = candidate.text.trim();
  if (id === "" || text === "") return null;
  return {
    id,
    text,
    ...(context.length === 0 ? {} : { context }),
    ...(typeof createdAt !== "number" || !Number.isFinite(createdAt)
        || createdAt < 0
      ? {}
      : { createdAt: Math.floor(createdAt) }),
    ...(deliveryState === undefined ? {} : { deliveryState }),
  };
}

/** Invalid rows and duplicate ids are dropped; repeated prompt text is allowed. */
export function normalizeSteeringQueue(items: readonly SteeringQueueItem[],
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  const limit = steeringQueueLimit(requestedLimit);
  const ids = new Set<string>();
  const result: SteeringQueueItem[] = [];
  for (const candidate of items) {
    const item = normalizeItem(candidate);
    if (item === null || ids.has(item.id)) continue;
    ids.add(item.id);
    result.push(item);
    if (result.length === limit) break;
  }
  return result;
}

export type EnqueueSteeringOutcome = "added" | "duplicate-id" | "full" | "invalid";

export function enqueueSteeringPrompt(items: readonly SteeringQueueItem[],
    candidate: SteeringQueueItem,
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): {
      items: SteeringQueueItem[];
      outcome: EnqueueSteeringOutcome;
    } {
  const limit = steeringQueueLimit(requestedLimit);
  const current = normalizeSteeringQueue(items, limit);
  const item = normalizeItem(candidate);
  if (item === null) return { items: current, outcome: "invalid" };
  if (current.some(existing => existing.id === item.id)) {
    return { items: current, outcome: "duplicate-id" };
  }
  if (current.length >= limit) return { items: current, outcome: "full" };
  return { items: [...current, item], outcome: "added" };
}

export function editSteeringPrompt(items: readonly SteeringQueueItem[], id: string, text: string,
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  const trimmed = text.trim();
  const current = normalizeSteeringQueue(items, requestedLimit);
  if (trimmed === "" || parseProviderCommandLike(trimmed) !== null) return current;
  return current.map(item => item.id === id ? { ...item, text: trimmed } : item);
}

export function removeSteeringPrompt(items: readonly SteeringQueueItem[], id: string,
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  return normalizeSteeringQueue(items, requestedLimit).filter(item => item.id !== id);
}

export function clearSteeringQueue(): SteeringQueueItem[] {
  return [];
}

/** Stop is durable: a restart must not silently resume prompts the user paused. */
export function pauseSteeringQueue(items: readonly SteeringQueueItem[],
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  return normalizeSteeringQueue(items, requestedLimit).map(item =>
    item.deliveryState === "inflight" ? item : { ...item, deliveryState: "paused" });
}

/** Persist this marker before provider transmission to make crash recovery at-most-once. */
export function markSteeringPromptInflight(items: readonly SteeringQueueItem[], id: string,
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  return normalizeSteeringQueue(items, requestedLimit).map(item =>
    item.id === id ? { ...item, deliveryState: "inflight" } : item);
}

/** A rejected transmission is safe to retry; only the selected marker is released. */
export function releaseSteeringPrompt(items: readonly SteeringQueueItem[], id: string,
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  return normalizeSteeringQueue(items, requestedLimit).map(item =>
    item.id === id ? { ...item, deliveryState: undefined } : item);
}

export function moveSteeringPrompt(items: readonly SteeringQueueItem[], id: string,
    move: SteeringQueueMove,
    requestedLimit = DEFAULT_STEERING_QUEUE_LIMIT): SteeringQueueItem[] {
  const current = normalizeSteeringQueue(items, requestedLimit);
  const from = current.findIndex(item => item.id === id);
  if (from < 0 || current.length < 2) return current;
  const to = move === "first" ? 0
    : move === "last" ? current.length - 1
    : move === "up" ? Math.max(0, from - 1)
    : Math.min(current.length - 1, from + 1);
  if (to === from) return current;
  const result = [...current];
  const [item] = result.splice(from, 1);
  result.splice(to, 0, item);
  return result;
}

export type SteeringQueueKeyAction = { type: "move"; move: SteeringQueueMove };

/** Alt + arrows/Home/End provide a discoverable non-drag reorder path. */
export function steeringQueueKeyAction(index: number, count: number, key: string,
    altKey: boolean): SteeringQueueKeyAction | null {
  if (!altKey || count < 2 || index < 0 || index >= count) return null;
  if (key === "ArrowUp" && index > 0) return { type: "move", move: "up" };
  if (key === "ArrowDown" && index < count - 1) return { type: "move", move: "down" };
  if (key === "Home" && index > 0) return { type: "move", move: "first" };
  if (key === "End" && index < count - 1) return { type: "move", move: "last" };
  return null;
}

export function steeringQueueItemName(item: SteeringQueueItem, maxLength = 56): string {
  const oneLine = item.text.replace(/\s+/g, " ").trim();
  if (oneLine.length <= maxLength) return oneLine;
  return `${oneLine.slice(0, Math.max(1, maxLength - 1)).trimEnd()}…`;
}

function steeringQueueSessionLimit(requested = MAX_STEERING_QUEUE_SESSIONS): number {
  if (!Number.isFinite(requested)) return MAX_STEERING_QUEUE_SESSIONS;
  return Math.min(MAX_STEERING_QUEUE_SESSIONS, Math.max(1, Math.floor(requested)));
}

function normalizeSteeringQueueMap(value: unknown,
    requestedQueueLimit = DEFAULT_STEERING_QUEUE_LIMIT,
    requestedSessionLimit = MAX_STEERING_QUEUE_SESSIONS): Record<string, SteeringQueueItem[]> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return {};
  const result: Record<string, SteeringQueueItem[]> = {};
  const sessionLimit = steeringQueueSessionLimit(requestedSessionLimit);
  let sessionCount = 0;
  for (const [rawSessionId, rawQueue] of Object.entries(value)) {
    if (sessionCount >= sessionLimit) break;
    const sessionId = rawSessionId.trim();
    if (sessionId === "" || sessionId === "__proto__" || sessionId === "constructor"
        || sessionId === "prototype" || Object.hasOwn(result, sessionId)
        || !Array.isArray(rawQueue)) continue;
    const queue = normalizeSteeringQueue(rawQueue as SteeringQueueItem[], requestedQueueLimit);
    if (queue.length > 0) {
      result[sessionId] = queue;
      sessionCount += 1;
    }
  }
  return result;
}

/** Tolerant localStorage boundary. Malformed, foreign, or unbounded data fails closed. */
export function parseSteeringQueueMap(raw: string | null,
    requestedQueueLimit = DEFAULT_STEERING_QUEUE_LIMIT,
    requestedSessionLimit = MAX_STEERING_QUEUE_SESSIONS): Record<string, SteeringQueueItem[]> {
  if (raw === null || raw.trim() === "") return {};
  try {
    return normalizeSteeringQueueMap(
      JSON.parse(raw) as unknown, requestedQueueLimit, requestedSessionLimit);
  } catch {
    return {};
  }
}

/** Serialize only the same normalized, bounded shape accepted by the parser. */
export function serializeSteeringQueueMap(
    queues: Readonly<Record<string, readonly SteeringQueueItem[]>>,
    requestedQueueLimit = DEFAULT_STEERING_QUEUE_LIMIT,
    requestedSessionLimit = MAX_STEERING_QUEUE_SESSIONS): string {
  return JSON.stringify(normalizeSteeringQueueMap(
    queues, requestedQueueLimit, requestedSessionLimit));
}
