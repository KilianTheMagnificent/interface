import type {
  ContextCompactTrigger, ContextUsage, EngineEvent, EngineId, PlanEntry,
} from "../shared/engine";
import {
  contextAutoCompactRemaining, contextUsageChange, contextWindowRemaining, formatContextTokens,
} from "../shared/engine";
import { parseAgentQuestions } from "../shared/questions";
import type { AgentQuestion } from "../shared/questions";
import { eventLineDelta } from "./diff-stats";
import { liveEventOrdinal } from "./live-events";
import { eventFailsTurn } from "./turn-events";
import type { PromptTurnImage } from "../shared/prompt-images";
import type { PromptTurnAttachment } from "./prompt-context";

/**
 * The conversation model: `ConvItem` and the reducer that turns an engine's
 * event log into it (`conversationItems`), with subagent grouping, the live
 * activity clip, the plan and the context-usage reports.
 */

// "ended" is not a result. It means the event stream stopped — an interrupt, a
// dead CLI, a session that ended mid-spawn — while this subagent was still
// running and no `subagent.finished` ever arrived. Whether it succeeded is
// unknown, so it borrows neither "done" nor "failed": the card says only what we
// can actually support.
export type SubagentStatus = "running" | "done" | "failed" | "ended";

export type ConvItem =
  | { kind: "text"; id: string; text: string; streaming: boolean }
  // What the USER said, not the agent. A replayed transcript is the only place
  // the engines emit this (engine.ts:147 — a live session never does, because
  // the app already knows what it just sent), and 14 of the 87 sessions on the
  // author's machine consist ENTIRELY of user turns: without this member they
  // reduce to an empty list and the pane says "No messages yet." over a real
  // conversation. The live half of the same thing is a locally echoed
  // PendingEcho below, rendered as this exact item kind so a message looks the
  // same the moment it is sent as it does after a reload.
  // `images` are the attachments drawn beside the message, and they now reach
  // this item BOTH ways: from the local echo the instant it is sent, and from a
  // replayed `user.message` whose engine store kept the bytes. What still has no
  // picture is an attachment the engine stored as a PATH — Codex's `localImage`
  // — which replays as `[image: <path>]` inside `text`. See PendingEcho.images.
  | { kind: "user"; id: string; text: string; images?: readonly PromptTurnImage[];
      attachments?: readonly PromptTurnAttachment[] }
  | { kind: "tool"; id: string; tool: string; summary: string;
      result?: { ok: boolean; summary: string }; input: unknown }
  | { kind: "approval"; id: string; tool: string; summary: string;
      input: unknown; questions?: AgentQuestion[]; resolved?: "allow" | "deny" }
  // `callId` is the tool call that produced this change. It is what lets the
  // renderer draw a call and its diffs as ONE row without guessing from
  // adjacency — see mergeFileChangeRows.
  | { kind: "diff"; id: string; path: string; callId: string;
      added?: number; removed?: number }
  | { kind: "notice"; id: string; text: string; tone?: "info" | "warning" | "error";
      /** The provider refused because its CLI is not signed in: offer Sign in. */
      signIn?: true }
  // What a peer agent answered when this session consulted one. `untrusted` is
  // a SAFETY property and not decoration: the collaboration controller records
  // the reply precisely so a later handoff cannot pass another provider's words
  // off as trusted user instruction, and a surface that drops the flag hands
  // the reader prose it has no way to tell apart from the agent's own. The CLI
  // cell carries it (cli/store/reducer.ts) — so does this one, and the row that
  // draws it has to say so on screen.
  | { kind: "peer"; id: string; operationId: string; sourceEngine: EngineId;
      objective: string; text: string; untrusted: true }
  // A page the agent showed (T3 section 8). `visualId` is Rust's id for it,
  // the same live and replayed, so `id` is stable across a reload. The page
  // itself is fetched by the card and drawn only in the sandboxed frame.
  | { kind: "visual"; id: string; visualId: string; title: string; bytes: number }
  | { kind: "compact"; id: string; trigger: ContextCompactTrigger;
      preTokens?: number; postTokens?: number; ok?: boolean }
  // A spawned subagent and its whole nested transcript. `id` is the agentKey,
  // which is the tool_use id of the Agent/Task call that opened it — so the card
  // stands exactly where that call was, and `children` are the items reduced
  // from every event the engine attributed to it.
  // `result` is the subagent's own closing summary and nothing else, kept apart
  // from `lastActivity` (which falls back to whatever it was mid-stream doing
  // when it never reported one). A detail surface can therefore say "this is
  // what the agent came back with" without claiming a half-finished tool
  // summary was the answer.
  | { kind: "subagent"; id: string; subagentType: string; description: string;
      model?: string; status: SubagentStatus; children: ConvItem[];
      toolCount: number; subagentCount: number; lastActivity?: string;
      result?: string };

type TextItem = Extract<ConvItem, { kind: "text" }>;
export type ToolItem = Extract<ConvItem, { kind: "tool" }>;
export type SubagentItem = Extract<ConvItem, { kind: "subagent" }>;

/** Every still-running subagent in this tree, depth-first, parent then child. */
export function collectRunningSubagents(
  items: readonly ConvItem[],
): Array<Extract<ConvItem, { kind: "subagent" }>> {
  const out: Array<Extract<ConvItem, { kind: "subagent" }>> = [];
  for (const item of items) {
    if (item.kind !== "subagent") continue;
    if (item.status === "running") out.push(item);
    if (item.children.length > 0) out.push(...collectRunningSubagents(item.children));
  }
  return out;
}

/**
 * A detached session has no live transport that could still be running a
 * child. Stored transcripts sometimes omit the child/session terminal edge,
 * so preserve the unknown outcome as `ended` instead of animating "Running"
 * forever. Explicit done/failed results remain authoritative.
 */
export function settleDetachedSubagents(items: readonly ConvItem[]): ConvItem[] {
  return items.map(item => {
    if (item.kind !== "subagent") return item;
    return {
      ...item,
      status: item.status === "running" ? "ended" : item.status,
      children: settleDetachedSubagents(item.children),
    };
  });
}

// The main thread's bucket. A symbol, not a string, so it can never collide
// with an agentKey no matter what the engine mints.
const MAIN = Symbol("main");
type ThreadKey = string | symbol;

// One position in one thread's stream: either an event to reduce, or an
// already-built subagent card standing in for the spawn that opened it. `at` is
// the event's ordinal in the ORIGINAL event array (see conversationItems), so
// generated ids stay unique across every thread rather than restarting at 0
// inside each subagent.
type Slot =
  | { at: number; kind: "evt"; evt: EngineEvent }
  | { at: number; kind: "card"; card: SubagentItem };

const ACTIVITY_MAX = 80;

// The ordinal of the event that created each item (a text run's first delta, a
// tool's call, a card's spawn), recorded by the reducer and carried by the
// projection's copies. The transcript places an echo of a sent message by it:
// a live list's positions shift when compaction drops its head, its ordinals
// do not (live-events.ts). Weak keys: an item no list holds is forgotten.
const ITEM_ORDINALS = new WeakMap<ConvItem, number>();

/** The item's creation ordinal in its live log; `index` for one built by hand. */
export function liveItemOrdinal(item: ConvItem, index: number): number {
  return ITEM_ORDINALS.get(item) ?? index;
}

/** `next` replaces `previous` in a live list: it keeps `previous`'s ordinal. */
export function keepItemOrdinal<T extends ConvItem>(next: T, previous: ConvItem): T {
  const ordinal = ITEM_ORDINALS.get(previous);
  if (ordinal !== undefined && next !== previous) ITEM_ORDINALS.set(next, ordinal);
  return next;
}

/** A new item the projection appends for the event at `ordinal`. */
export function itemAtOrdinal<T extends ConvItem>(item: T, ordinal: number): T {
  ITEM_ORDINALS.set(item, ordinal);
  return item;
}

/**
 * Pure reducer: an event log in, a render list out. `resolved` is never set here —
 * the approval card owns its own allow/deny state after `iface.respond` resolves.
 * Item ids double as handles: an approval item's id is its approvalId and a tool
 * item's id is its callId (the adapter mints approvalIds with randomUUID, so the
 * two id spaces do not collide).
 *
 * Subagents (Phase 1.6) are GROUPED rather than flattened. Events carrying an
 * `agentKey` are reduced into that subagent's own `children` list by the very
 * same reducer the main thread uses, and the card is drawn at the position of
 * the spawn that opened it. The spawn's own tool.call/tool.result pair is
 * dropped from the parent thread once a card exists for it, because the card IS
 * that call — showing both would spawn twice on screen.
 *
 * Attribution we cannot resolve is never dropped: an `agentKey` no
 * `subagent.started` ever announced falls back to the main thread, which is
 * exactly the flat stream this app rendered before subagents existed.
 *
 * Generated ids (`text-<n>`, `user-<n>`, `notice-<n>`…) are minted from
 * `ordinalOf`: by default the event's live-log ordinal, which compaction does
 * not shift (live-events.ts), and its index for an event that never entered a
 * live log. Every reduction of one log must use the same rule, because callers
 * hold row keys built from these ids across frames.
 */
export function conversationItems(events: EngineEvent[],
    ordinalOf: (event: EngineEvent, index: number) => number = liveEventOrdinal): ConvItem[] {
  // ---- pass 1: every announced subagent gets its card up front, so a spawn's
  // tool.call is recognisable as a spawn regardless of which of the two the
  // scan reaches first.
  const cards = new Map<string, SubagentItem>();
  const announced = new Map<string, number>();     // agentKey -> subagent.started index
  const reported = new Map<string, string>();      // agentKey -> its closing summary

  for (let i = 0; i < events.length; i++) {
    const evt = events[i];
    if (evt.kind !== "subagent.started") continue;
    if (cards.has(evt.agentKey)) continue;         // re-announced: the first wins
    const card: SubagentItem = {
      kind: "subagent", id: evt.agentKey, subagentType: evt.subagentType,
      description: evt.description, status: "running", children: [], toolCount: 0,
      subagentCount: 0,
    };
    // Absent, not undefined: the engine omits the model when the subagent
    // inherits the parent's, and that is not something to render as "undefined".
    if (evt.model !== undefined) card.model = evt.model;
    cards.set(evt.agentKey, card);
    announced.set(evt.agentKey, i);
  }

  for (const evt of events) {
    if (evt.kind !== "subagent.finished") continue;
    const card = cards.get(evt.agentKey);
    if (card === undefined) continue;              // a close with nothing to close
    card.status = evt.ok ? "done" : "failed";
    if (evt.summary.length > 0) reported.set(evt.agentKey, evt.summary);
  }

  // `subagent.finished` is not guaranteed to arrive. When the stream reaches a
  // terminal condition with a card still "running", that card would otherwise
  // pulse forever for something that stopped — so it becomes "ended", which
  // asserts nothing about the outcome. A card that already reported done/failed
  // is untouched: a real result outranks "no result".
  //
  // The LAST terminal event, not the first: this event log accumulates across a
  // resume, so a close belonging to an earlier turn must not end a subagent
  // spawned after it.
  let lastTerminal = -1;
  for (let i = 0; i < events.length; i++) {
    const evt = events[i];
    if (eventFailsTurn(evt)) lastTerminal = i;
  }
  for (const [key, at] of announced) {
    const card = cards.get(key);
    if (card === undefined || card.status !== "running") continue;
    if (at < lastTerminal) card.status = "ended";
  }

  // An agentKey nobody announced is not a thread we can nest — see the doc
  // comment: those events stay on the main thread rather than vanishing.
  const bucketOf = (agentKey: string | undefined): ThreadKey =>
    agentKey !== undefined && cards.has(agentKey) ? agentKey : MAIN;

  // ---- pass 2: where each card is drawn. At its spawn's tool.call, in whatever
  // thread made that call — so a subagent that spawns a subagent nests one level
  // deeper. The spawn's callId IS the agentKey (both are the same tool_use id),
  // which is what makes the pairing possible at all.
  const drawnAt = new Map<string, number>();       // agentKey -> event index
  const thread = new Map<string, ThreadKey>();     // agentKey -> thread holding the card
  for (let i = 0; i < events.length; i++) {
    const evt = events[i];
    if (evt.kind !== "tool.call") continue;
    if (!cards.has(evt.callId) || drawnAt.has(evt.callId)) continue;
    drawnAt.set(evt.callId, i);
    thread.set(evt.callId, bucketOf(evt.agentKey));
  }
  // A card whose spawn call we never saw still has to appear somewhere: draw it
  // where it was announced, on the main thread.
  for (const [key, at] of announced) {
    if (drawnAt.has(key)) continue;
    drawnAt.set(key, at);
    thread.set(key, MAIN);
  }
  const placement = new Map<number, SubagentItem[]>();
  for (const [key, card] of cards) {
    const at = drawnAt.get(key);
    if (at === undefined) continue;                // unreachable: every card is placed
    const here = placement.get(at);
    if (here === undefined) placement.set(at, [card]);
    else here.push(card);
  }

  // ---- pass 3: split the log into one slot list per thread, in event order.
  const buckets = new Map<ThreadKey, Slot[]>();
  const push = (key: ThreadKey, slot: Slot): void => {
    const list = buckets.get(key);
    if (list === undefined) buckets.set(key, [slot]);
    else list.push(slot);
  };
  for (let i = 0; i < events.length; i++) {
    const evt = events[i];
    const at = ordinalOf(evt, i);
    const here = placement.get(i);
    if (here !== undefined) {
      for (const card of here) push(thread.get(card.id) ?? MAIN, { at, kind: "card", card });
    }
    // subagent.started is not transcript — the card carries everything it said —
    // and dropping it here rather than in the reducer's default arm keeps it
    // from splitting a streaming text run a second time: the card slot at the
    // spawn already split it, and another split would leave an empty text block
    // sitting between the prose and the card.
    if (evt.kind === "subagent.started") continue;
    // subagent.finished is kept, and routes into that subagent's OWN bucket —
    // an announced key never buckets to MAIN, so no main-thread run is split.
    // There it hits the reducer's default arm, which pushes no item but does
    // close the subagent's open text run. Without it the last text child of a
    // finished card stays streaming forever and renders a live blinking caret
    // under a header that says "done". An unannounced key has no bucket to
    // close, so it stays dropped rather than landing on the main thread.
    if (evt.kind === "subagent.finished" && !cards.has(evt.agentKey)) continue;
    // The card replaces the spawn's call AND its result. Both are keyed by the
    // agentKey, and an ordinary call can never collide with one: the agentKey is
    // the spawn's own tool_use id.
    if ((evt.kind === "tool.call" || evt.kind === "tool.result")
        && cards.has(evt.callId)) continue;
    push(bucketOf("agentKey" in evt ? evt.agentKey : undefined),
      { at, kind: "evt", evt });
  }

  // ---- pass 4: fill each card from its own bucket, with the same reducer the
  // main thread gets. Cards are already embedded in their parent's slot list by
  // reference, so filling them in any order is safe.
  for (const [key, card] of cards) {
    const slots = buckets.get(key) ?? [];
    card.children = reduceItems(slots);
    // A card that is no longer running must not keep a blinking caret under it.
    // done/failed get that from their own `subagent.finished` landing in this
    // bucket and closing the run; "ended" has no such event by definition — the
    // stream simply stopped — so the last text child is closed here instead.
    if (card.status !== "running") {
      const last = card.children[card.children.length - 1];
      if (last !== undefined && last.kind === "text") last.streaming = false;
    }
    // What the collapsed card claims: only the calls this subagent made itself.
    // A nested spawn is its own card, not one of its parent's tools.
    card.toolCount = card.children.reduce((n, c) => (c.kind === "tool" ? n + 1 : n), 0);
    // …and it is counted here instead, because the alternative is a card that
    // delegated all of its work saying "no activity". An orchestrator subagent
    // that calls nothing itself and spawns three children had a toolCount of 0,
    // no lastActivity of its own, and therefore a metadata line reading
    // "starting…" over three finished agents — a false statement about the one
    // thing the collapsed card exists to report.
    card.subagentCount = card.children.reduce(
      (n, c) => (c.kind === "subagent" ? n + 1 : n), 0);
    // The closing summary is the newest thing the subagent said, so it outranks
    // whatever it was doing a moment before finishing.
    const result = reported.get(key);
    const activity = result ?? streamActivity(slots);
    if (activity !== undefined) card.lastActivity = activity;
    // …and the same string is recorded as the RESULT, which is a different
    // claim: lastActivity may be a mid-stream tool summary that nobody reported,
    // and a surface that prints "the agent reported" over one of those is
    // lying. Only a `subagent.finished` summary lands here.
    if (result !== undefined) card.result = result;
  }

  return reduceItems(buckets.get(MAIN) ?? []);
}

/** ACP plan updates replace the whole plan; the newest one is authoritative. */
export function latestPlanEntries(events: readonly EngineEvent[]): readonly PlanEntry[] | undefined {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    const event = events[index];
    if (event.kind === "plan.updated") return event.entries;
  }
  return undefined;
}

/**
 * The two newest complete provider reports. A compacting-only ping merges onto
 * current so the meter stays visible, while previous remains a stable baseline.
 */
export interface ContextUsageReports {
  current?: ContextUsage;
  previous?: ContextUsage;
}

function isCompleteContextUsage(usage: ContextUsage): boolean {
  return usage.used !== undefined && usage.size !== undefined
    && Number.isFinite(usage.used) && Number.isFinite(usage.size)
    && usage.used >= 0 && usage.size > 0;
}

export function contextUsageReports(events: readonly EngineEvent[]): ContextUsageReports {
  let current: ContextUsage | undefined;
  let previous: ContextUsage | undefined;
  for (const event of events) {
    if (event.kind === "context.usage") {
      const next = event.usage;
      if (next.compacting === true && current !== undefined
          && !isCompleteContextUsage(next)) {
        current = { ...current, compacting: true };
      } else {
        if (isCompleteContextUsage(next) && current !== undefined
            && isCompleteContextUsage(current)) {
          previous = { ...current, compacting: false };
        }
        current = next;
      }
      continue;
    }
    if (event.kind === "context.compact" && current?.compacting === true) {
      current = { ...current, compacting: false };
    }
  }
  return {
    ...(current === undefined ? {} : { current }),
    ...(previous === undefined ? {} : { previous }),
  };
}

/** `41%`, or `<1%` for a context that holds tokens but rounds to none; "" when unreported. */
export function contextPercentText(usage: ContextUsage): string {
  const percent = usage.percent;
  if (percent === null || percent === undefined) return "";
  return percent === 0 && (usage.used ?? 0) > 0 ? "<1%" : `${percent}%`;
}

export function contextMeterLabel(usage: ContextUsage): string {
  return usage.compacting === true ? "Compacting" : contextPercentText(usage);
}

export interface ContextMeterDetails {
  windowRemaining?: string;
  reportChange?: string;
  autoCompact?: "on" | "off";
  autoCompactThreshold?: string;
}

export function contextMeterDetails(usage: ContextUsage,
    previous?: ContextUsage): ContextMeterDetails {
  const remaining = contextWindowRemaining(usage);
  const change = contextUsageChange(previous, usage);
  const compactRemaining = contextAutoCompactRemaining(usage);
  const threshold = usage.autoCompactThreshold;
  const reportChange = change === undefined ? undefined
    : `${change > 0 ? "+" : change < 0 ? "-" : ""}${
      formatContextTokens(Math.abs(change))}`;
  return {
    ...(remaining === undefined ? {} : { windowRemaining: formatContextTokens(remaining) }),
    ...(reportChange === undefined ? {} : { reportChange }),
    ...(usage.autoCompact === undefined ? {} : {
      autoCompact: usage.autoCompact ? "on" as const : "off" as const,
    }),
    ...(threshold === undefined ? {} : {
      autoCompactThreshold: `${formatContextTokens(threshold)}${
        compactRemaining === undefined ? ""
          : compactRemaining === 0 ? " · threshold reached"
          : ` · ${formatContextTokens(compactRemaining)} remaining`}`,
    }),
  };
}

export function compactSeamLabel(item: Extract<ConvItem, { kind: "compact" }>): string {
  if (item.ok === false) return "Context compaction failed";
  const how = item.trigger === "auto" ? " automatically"
    : item.trigger === "manual" ? " on request" : "";
  const from = item.preTokens === undefined ? "" : formatContextTokens(item.preTokens);
  const to = item.postTokens === undefined ? "" : formatContextTokens(item.postTokens);
  const span = from !== "" && to !== "" ? ` · ${from} → ${to}` : "";
  return `Context compacted${how}${span}`;
}

// The single item-building reducer, shared by the main thread and by every
// subagent — nesting must not mean a second copy of these rules.
function reduceItems(slots: Slot[]): ConvItem[] {
  const items: ConvItem[] = [];
  const tools = new Map<string, ToolItem>();
  let open: TextItem | null = null;
  // Every item is pushed while the slot that creates it is being reduced, so
  // stamping the new tail at the next slot (and once after the loop) gives
  // each its creation ordinal without touching any push below.
  let stamped = 0;
  let stampAt = 0;
  const stamp = (): void => {
    for (; stamped < items.length; stamped++) ITEM_ORDINALS.set(items[stamped], stampAt);
  };

  for (const slot of slots) {
    stamp();
    stampAt = slot.at;
    if (slot.kind === "evt" && slot.evt.kind === "message.delta") {
      if (!open) {
        open = { kind: "text", id: `text-${slot.at}`, text: "", streaming: true };
        items.push(open);
      }
      open.text += slot.evt.text;
      continue;
    }
    // Command catalogues are ephemeral control snapshots. They normally live
    // outside the transcript store, but keeping them transparent here makes a
    // replay/import or future caller unable to split one streamed paragraph.
    if (slot.kind === "evt" && slot.evt.kind === "commands.available") continue;
    // Anything that is not a delta closes the run: the text stops streaming, and
    // later deltas start a fresh block.
    if (open) {
      open.streaming = false;
      open = null;
    }
    if (slot.kind === "card") {
      items.push(slot.card);
      continue;
    }

    const evt = slot.evt;
    switch (evt.kind) {
      case "tool.call": {
        const existing = tools.get(evt.callId);
        if (existing !== undefined) {
          existing.tool = evt.tool;
          existing.summary = evt.summary;
          existing.input = evt.input;
          break;
        }
        const item: ToolItem = { kind: "tool", id: evt.callId, tool: evt.tool,
          summary: evt.summary, input: evt.input };
        tools.set(evt.callId, item);
        items.push(item);
        break;
      }
      case "tool.result": {
        // Completed in place by callId — results may arrive out of order, and an
        // orphan result (no matching call) renders nothing.
        const item = tools.get(evt.callId);
        if (item) item.result = { ok: evt.ok, summary: evt.summary };
        break;
      }
      case "user.message":
        // The other half of the transcript. `at` is the event's ordinal in
        // the ORIGINAL event array (see Slot), so this id is unique across
        // every thread and stable for as long as the event stays in its log.
        //
        // `images` ride through unchanged and unvalidated HERE because the
        // mapper that minted the event already validated every `src` against
        // isPromptImagePreviewUrl — one boundary, at the edge where untrusted
        // store data enters, rather than a second opinion in the reducer that
        // would drift out of step with the first.
        items.push({ kind: "user", id: `user-${slot.at}`, text: evt.text,
          ...(evt.images === undefined || evt.images.length === 0
            ? {} : { images: evt.images }) });
        break;
      case "peer.result":
        // Transcript, not chrome: this is the answer to a question the session
        // asked, and a pane that drops it shows the reader a turn with no reply
        // in it. The untrusted flag rides along rather than being re-derived at
        // the row, so no future caller can build one of these without it.
        items.push({
          kind: "peer", id: `peer-${slot.at}`, operationId: evt.operationId,
          sourceEngine: evt.sourceEngine, objective: evt.objective, text: evt.text,
          untrusted: true,
        });
        break;
      case "visual.shown":
        items.push({ kind: "visual", id: `visual-${evt.id}`, visualId: evt.id,
          title: evt.title, bytes: evt.bytes });
        break;
      case "approval.requested": {
        const questions = parseAgentQuestions(evt.tool, evt.input);
        items.push({
          kind: "approval", id: evt.approvalId, tool: evt.tool,
          summary: evt.summary, input: evt.input,
          ...(questions === undefined ? {} : { questions }),
        });
        break;
      }
      case "diff.produced": {
        const delta = eventLineDelta(evt);
        items.push({
          kind: "diff", id: `diff-${slot.at}`, path: evt.path, callId: evt.callId,
          added: delta.added, removed: delta.removed,
        });
        break;
      }
      case "error":
        items.push({ kind: "notice", id: `notice-${slot.at}`, text: evt.message,
          tone: "error", ...(evt.cause === "signedOut" ? { signIn: true as const } : {}) });
        break;
      case "notice":
        items.push({ kind: "notice", id: `notice-${slot.at}`, text: evt.message,
          tone: evt.tone });
        break;
      case "turn.retry":
        // The attempt counter is printed, not left to the provider's sentence.
        // `message` is whatever the vendor CLI happened to say and need not
        // carry the numbers at all, so a bare echo of it told the reader an
        // unbounded loop and a last attempt apart only by luck — while the CLI
        // beside it has always drawn "retry 1/10". Both surfaces now state the
        // same fact.
        items.push({ kind: "notice", id: `notice-${slot.at}`,
          text: `Retry ${evt.attempt}/${evt.maxAttempts} — ${evt.message}`,
          tone: "info" });
        break;
      case "context.compact":
        items.push({
          kind: "compact", id: `compact-${slot.at}`, trigger: evt.trigger,
          ...(evt.preTokens === undefined ? {} : { preTokens: evt.preTokens }),
          ...(evt.postTokens === undefined ? {} : { postTokens: evt.postTokens }),
          ...(evt.ok === undefined ? {} : { ok: evt.ok }),
        });
        break;
      // NAMED, not swept up by a default arm. session.meta, thinking.signal,
      // status.changed, usage.updated, throughput.updated and context.usage
      // drive the header, meter and status chips; models.available,
      // commands.available, commands.invalidated, config.changed,
      // access.changed and fastmode.changed drive the model/effort/access
      // pills; plan.updated draws the plan panel; the subagent edges and
      // session.closed are consumed by conversationItems' own passes. None of
      // them belongs in the conversation list — a picker click is not something
      // the agent said. They do still close an open text run, like any
      // non-delta event.
      //
      // message.delta and commands.available never reach this switch at all
      // (both are taken above), but they are still labelled: the check below is
      // over the whole EngineEvent union, and a kind that is missing because it
      // is handled elsewhere is indistinguishable from one that was forgotten.
      case "message.delta":
      case "session.meta":
      case "thinking.signal":
      case "status.changed":
      case "usage.updated":
      case "throughput.updated":
      case "context.usage":
      case "models.available":
      case "commands.available":
      case "commands.invalidated":
      case "config.changed":
      case "access.changed":
      case "fastmode.changed":
      case "plan.updated":
      case "subagent.started":
      case "subagent.finished":
      case "session.closed":
        break;
      default:
        // A kind added to EngineEvent is a COMPILE ERROR here now, exactly as it
        // already was in the CLI reducer. The bare `default: break` this
        // replaced is how peer.result reached this surface and rendered
        // NOTHING: the two front ends silently disagreed about what the user is
        // allowed to see, and no build ever said so. Declared locally rather
        // than imported from cli/store/reducer.ts — that boundary is deliberate
        // and the renderer does not reach across it.
        assertNever(evt);
    }
  }
  stamp();
  return items;
}

export function assertNever(value: never): never {
  throw new Error(`Unhandled engine event: ${String(value)}`);
}

// The one line a collapsed card shows. Tool traffic is the useful signal —
// "Bash — npm test" says more about where a subagent is than the prose around
// it — so the newest tool summary wins, and prose is the fallback for an agent
// that has not called anything yet.
function streamActivity(slots: Slot[]): string | undefined {
  let last: string | undefined;
  let prose = "";
  for (const slot of slots) {
    if (slot.kind === "card") continue;
    const evt = slot.evt;
    if ((evt.kind === "tool.call" || evt.kind === "tool.result")
        && evt.summary.length > 0) last = evt.summary;
    else if (evt.kind === "message.delta") prose += evt.text;
  }
  if (last !== undefined) return clip(last);
  const said = clip(prose);
  return said.length > 0 ? said : undefined;
}

/**
 * What the conversation projection needs to stream a running subagent's text
 * without the full reducer: every running card, and whether its activity line
 * is a tool summary (which a delta cannot change) rather than its prose.
 */
export interface SubagentStream {
  readonly toolActivity: boolean;
}

export function runningSubagentStreams(items: readonly ConvItem[],
    events: readonly EngineEvent[]): ReadonlyMap<string, SubagentStream> {
  const announced = new Set<string>();
  for (const event of events) if (event.kind === "subagent.started") announced.add(event.agentKey);
  const toolActivity = new Set<string>();
  for (const event of events) {
    // streamActivity's rule over that card's own bucket: a spawn's call and
    // result are the card itself, not one of its tools.
    if ((event.kind === "tool.call" || event.kind === "tool.result") && event.agentKey !== undefined
        && announced.has(event.agentKey) && !announced.has(event.callId)
        && event.summary.length > 0) toolActivity.add(event.agentKey);
  }
  const streams = new Map<string, SubagentStream>();
  for (const card of collectRunningSubagents(items)) {
    if (announced.has(card.id)) streams.set(card.id, { toolActivity: toolActivity.has(card.id) });
  }
  return streams;
}

/**
 * `card` with one more delta streamed into its own thread, as conversationItems
 * builds it: the open text run grows, or a new one starts with `id`. When the
 * newest delta merely replaced the previous one (live-events merges a run's
 * deltas), `replaced` is that previous delta's text, taken back off first.
 * Null when the card does not end in the open run that `replaced` needs.
 */
export function streamSubagentDelta(card: SubagentItem, text: string, id: string,
    stream: SubagentStream, replaced?: string): SubagentItem | null {
  const children = [...card.children];
  let tail = children[children.length - 1];
  if (replaced !== undefined) {
    if (tail?.kind !== "text" || !tail.streaming || !tail.text.endsWith(replaced)) return null;
    tail = { ...tail, text: tail.text.slice(0, tail.text.length - replaced.length) };
    children[children.length - 1] = tail;
  }
  if (tail?.kind === "text" && tail.streaming) {
    children[children.length - 1] = { ...tail, text: tail.text + text };
  } else {
    children.push({ kind: "text", id, text, streaming: true });
  }
  const { lastActivity: _previous, ...rest } = card;
  const next: SubagentItem = { ...rest, children };
  if (stream.toolActivity) {
    if (card.lastActivity !== undefined) next.lastActivity = card.lastActivity;
    return next;
  }
  const said = clipText(children);
  if (said.length > 0) next.lastActivity = said;
  return next;
}

/** `clip` of every text child's text joined, reading only as far as the clip needs. */
function clipText(children: readonly ConvItem[]): string {
  let flat = "";
  let space = false;
  for (const child of children) {
    if (child.kind !== "text") continue;
    for (const character of child.text) {
      if (/\s/u.test(character)) {
        if (flat.length > 0) space = true;
        continue;
      }
      if (space) { flat += " "; space = false; }
      flat += character;
      if (flat.length > ACTIVITY_MAX) return `${flat.slice(0, ACTIVITY_MAX - 1)}…`;
    }
  }
  return flat;
}

// One line, bounded. Newlines collapse because the card renders this on a single
// ellipsised row and a raw tool summary is often several lines. Exported because
// the sidebar clips its own activity line to the same width — see
// session-activity.ts's sessionActivity, which differs in the SCAN, not the clipper.
export function clip(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length <= ACTIVITY_MAX ? flat : `${flat.slice(0, ACTIVITY_MAX - 1)}…`;
}
