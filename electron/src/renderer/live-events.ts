import type { EngineEvent, SessionKey } from "../shared/engine";
import { slimDiffEvent } from "./diff-stats";
import { eventEndsTurn } from "./turn-events";

export const MAX_LIVE_EVENTS_PER_SESSION = 10_000;
export const LIVE_HISTORY_LIMITS = {
  sessionBytes: 4 * 1024 * 1024,
  totalBytes: 32 * 1024 * 1024,
  textBytes: 1024 * 1024,
  events: MAX_LIVE_EVENTS_PER_SESSION,
};
type Limits = typeof LIVE_HISTORY_LIMITS;
type Logs = Record<SessionKey, EngineEvent[]>;
export const COMPACTED_NOTICE: EngineEvent = {
  kind: "notice", tone: "warning",
  message: "Earlier live events were compacted to keep this session responsive. Its provider history remains unchanged.",
};

// Each live event's ordinal: the position it was appended at, kept for as long
// as the event stays in its session's log. The conversation mints its
// generated item ids (`text-<n>`, `user-<n>`) and so its row keys from it.
// They used to come from the event's index, and compaction (which drops the
// log's head) or the omission notice (put in front) shifted every later index:
// every generated row was renamed, re-rendered and re-parsed, and anything
// keyed by a row (a finished turn's clock, a turn's restore point) moved to a
// different message. A log never compacted has ordinals equal to its indices,
// so its ids are what they always were. The notice, the one event every log
// shares, comes before all of them. Weak keys, like the size caches below.
const ordinals = new WeakMap<EngineEvent, number>([[COMPACTED_NOTICE, -1]]);
const nextOrdinals = new WeakMap<EngineEvent[], number>();

/** The event's ordinal in its live log; `index` for one that never entered a log. */
export function liveEventOrdinal(event: EngineEvent, index: number): number {
  return ordinals.get(event) ?? index;
}

function nextOrdinal(log: EngineEvent[]): number {
  const known = nextOrdinals.get(log);
  if (known !== undefined) return known;
  const last = log.length - 1;
  return last < 0 ? 0 : liveEventOrdinal(log[last], last) + 1;
}

// Conservative retained-content accounting, not a JS heap measurement. Counting
// UTF-16 avoids allocating an encoded copy of every streamed string. Weak keys
// do not keep deleted sessions, replaced logs or event payloads alive.
const sizes = new WeakMap<object, number>();
const logSizes = new WeakMap<EngineEvent[], number>();
function retainedBytes(value: unknown): number {
  if (typeof value === "string") return 2 * value.length;
  if (value === null || typeof value !== "object") return 8;
  const cached = sizes.get(value);
  if (cached !== undefined) return cached;
  let size = 32;
  for (const [key, child] of Object.entries(value)) size += 2 * key.length + retainedBytes(child);
  sizes.set(value, size);
  return size;
}
export function liveHistoryBytes(log: EngineEvent[]): number {
  let size = logSizes.get(log);
  if (size === undefined) {
    size = log.reduce((sum, event) => sum + retainedBytes(event), 0);
    logSizes.set(log, size);
  }
  return size;
}

function textTail(text: string, byteLimit: number): string {
  let start = Math.max(0, text.length - Math.floor(byteLimit / 2));
  // Never expose half a surrogate pair at an omission boundary.
  if (start > 0 && /[\uDC00-\uDFFF]/.test(text[start])) start++;
  return text.slice(start);
}

const PROTECTED_KINDS: ReadonlySet<EngineEvent["kind"]> = new Set<EngineEvent["kind"]>([
  "session.meta", "session.closed", "status.changed", "config.changed", "access.changed",
  "fastmode.changed", "models.available", "plan.updated", "usage.updated", "context.usage",
  "thinking.signal", "subagent.started", "subagent.finished"]);

/** Safety and current state survive display-history eviction. An unresolved
 * approval is never clipped: its full input is needed to make a decision. These
 * control facts are a separate reserve, not disposable transcript bytes. */
function protectedEvents(log: readonly EngineEvent[]): Set<EngineEvent> {
  const kept = new Set<EngineEvent>();
  const latest = new Set<string>();
  let approvalsClosed = false;
  for (let i = log.length - 1; i >= 0; i--) {
    const event = log[i];
    if (event.kind === "approval.requested" && !approvalsClosed) kept.add(event);
    if (eventEndsTurn(event)) approvalsClosed = true;
    if (event.kind === "error" && !event.retryable || PROTECTED_KINDS.has(event.kind)) {
      const key = `${event.kind}:${"agentKey" in event ? event.agentKey ?? "" : ""}`;
      if (!latest.has(key)) { latest.add(key); kept.add(event); }
    }
  }
  return kept;
}

function compact(log: EngineEvent[], budget: number, count: number,
  protectedSet = protectedEvents(log)): EngineEvent[] {
  let bytes = retainedBytes(COMPACTED_NOTICE);
  let disposableCount = 1;
  const retained: EngineEvent[] = [];
  for (let i = log.length - 1; i >= 0; i--) {
    const event = log[i];
    if (event === COMPACTED_NOTICE) continue;
    const size = retainedBytes(event);
    if (protectedSet.has(event)) retained.push(event);
    else if (bytes + size <= budget && disposableCount < count) {
      retained.push(event); bytes += size; disposableCount++;
    }
  }
  retained.reverse();
  const result = [COMPACTED_NOTICE, ...retained];
  nextOrdinals.set(result, nextOrdinal(log));
  liveHistoryBytes(result);
  return result;
}

/**
 * Compaction's low-water mark, as a share of the caps. A log over its caps is
 * cut to 90% of them, so the next tenth of the caps (about 1,000 events or
 * 400 KiB of a session's) appends without compacting again. Cutting to the caps
 * themselves left every later frame one event over: each one recompacted the
 * whole log, and because compaction drops the log's head, the conversation
 * projection could never take its append path at the cap. The cost is that a
 * compacted pane shows up to a tenth fewer early live events.
 */
export const LIVE_COMPACTION_LOW_WATER = 0.9;

/** `compact`, or `log` itself when that would return the same sequence: it
 * already starts with the notice and every disposable event still fits, by
 * compact's own accounting (the notice takes a slot and its bytes, the reserve
 * neither). The reserve alone can hold a compacted log over the caps, and
 * every later frame then re-ran compaction to copy the log unchanged. A log
 * that does not fit is compacted to `lowWater` of `budget` and `count`. */
export function compactLiveLog(log: EngineEvent[], budget: number, count: number,
  lowWater = 1): EngineEvent[] {
  const reserve = protectedEvents(log);
  if (log[0] === COMPACTED_NOTICE) {
    let bytes = retainedBytes(COMPACTED_NOTICE);
    let disposableCount = 1;
    let fits = true;
    for (let i = log.length - 1; i > 0 && fits; i--) {
      if (reserve.has(log[i])) continue;
      bytes += retainedBytes(log[i]);
      disposableCount++;
      fits = log[i] !== COMPACTED_NOTICE && bytes <= budget && disposableCount <= count;
    }
    if (fits) return log;
  }
  return compact(log, Math.floor(budget * lowWater), Math.floor(count * lowWater), reserve);
}

/** Immutable frame batching; each touched log is copied once and each new
 * payload measured once. The continuous-text path never builds an unbounded
 * concatenation, even inside one unusually large push batch. */
export function appendLiveEvents(current: Logs,
  items: readonly { key: SessionKey; evt: EngineEvent }[], limits: Limits = LIVE_HISTORY_LIMITS): Logs {
  const working = new Map<SessionKey,
    { log: EngineEvent[]; bytes: number; omitted: boolean; next: number }>();
  for (const { key, evt } of items) {
    if (evt.kind === "commands.available" || evt.kind === "commands.invalidated") continue;
    let entry = working.get(key);
    const last = (entry?.log ?? current[key])?.at(-1);
    if (last?.kind === "thinking.signal" && evt.kind === "thinking.signal"
      && last.active === evt.active && last.agentKey === evt.agentKey) continue;
    if (!entry) {
      const old = current[key] ?? [];
      entry = { log: [...old], bytes: liveHistoryBytes(old), omitted: false, next: nextOrdinal(old) };
      working.set(key, entry);
    }
    let event = evt;
    let ordinal: number | undefined;
    if (evt.kind === "message.delta") {
      const merging = last?.kind === "message.delta" && last.agentKey === evt.agentKey;
      const incoming = textTail(evt.text, limits.textBytes);
      const prefix = merging ? textTail(last.text, Math.max(0, limits.textBytes - 2 * incoming.length)) : "";
      event = { ...(merging ? last : evt), text: prefix + incoming };
      entry.omitted ||= incoming.length !== evt.text.length || merging && prefix.length !== last.text.length;
      if (merging) {
        // The longer merge stands where the delta it replaces stood.
        ordinal = liveEventOrdinal(last, entry.log.length - 1);
        entry.log.pop(); entry.bytes -= retainedBytes(last);
      }
    } else {
      if (evt.kind === "diff.produced") event = slimDiffEvent(evt);
      // One object appended twice would share an ordinal, and so an item id.
      if (ordinals.has(event)) event = { ...event };
    }
    ordinals.set(event, ordinal ?? entry.next++);
    entry.log.push(event);
    entry.bytes += retainedBytes(event);
  }
  if (working.size === 0) return current;
  const next = { ...current };
  for (const [key, entry] of working) {
    logSizes.set(entry.log, entry.bytes);
    nextOrdinals.set(entry.log, entry.next);
    if (entry.omitted && entry.log[0] !== COMPACTED_NOTICE) {
      entry.log.unshift(COMPACTED_NOTICE);
      logSizes.set(entry.log, entry.bytes + retainedBytes(COMPACTED_NOTICE));
    }
    // The cheap length/bytes test first: the reserve walk runs only over the caps.
    next[key] = liveHistoryBytes(entry.log) > limits.sessionBytes || entry.log.length > limits.events
      ? compactLiveLog(entry.log, limits.sessionBytes, limits.events, LIVE_COMPACTION_LOW_WATER) : entry.log;
  }
  let total = Object.values(next).reduce((sum, log) => sum + liveHistoryBytes(log), 0);
  if (total > limits.totalBytes) {
    // Untouched sessions yield display history before the actively growing tail.
    const keys = [...Object.keys(next).filter(key => !working.has(key)), ...working.keys()];
    for (const key of keys) {
      if (total <= limits.totalBytes) break;
      const before = liveHistoryBytes(next[key]);
      next[key] = compact(next[key], Math.max(0, before - (total - limits.totalBytes)), limits.events);
      total += liveHistoryBytes(next[key]) - before;
    }
  }
  return next;
}
