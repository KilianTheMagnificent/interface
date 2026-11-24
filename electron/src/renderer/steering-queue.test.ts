import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_STEERING_QUEUE_LIMIT,
  MAX_STEERING_QUEUE_LIMIT,
  clearSteeringQueue,
  captureOutboundSendEvent,
  editSteeringPrompt,
  enqueueSteeringPrompt,
  moveSteeringPrompt,
  markSteeringPromptInflight,
  normalizeSteeringQueue,
  parseSteeringQueueMap,
  removeSteeringPrompt,
  pauseSteeringQueue, releaseSteeringPrompt,
  serializeSteeringQueueMap,
  outboundSettledDuringSend, steeringQueueCanAutoDeliver,
  steeringQueueItemName,
  steeringQueueEventStartsTurn,
  steeringQueueKeyAction,
  steeringQueueLimit,
  steeringQueueTerminalFailure,
  type SteeringQueueItem,
} from "./steering-queue";
import {
  STEERING_QUEUE_CSS,
  SteeringQueue,
} from "./components/SteeringQueue";
import { logicalListFocusIndex } from "./components/ModalBehavior";
import type { PromptContextItem } from "./prompt-context";

const queued = (id: string, text = `Prompt ${id}`): SteeringQueueItem => ({ id, text });
const fileContext: PromptContextItem = {
  id: "ctx-file", kind: "file", label: "App.tsx", value: "/repo/App.tsx",
};

describe("steering queue model", () => {
  it("clamps limits and normalizes untrusted restored rows", () => {
    expect(steeringQueueLimit(0)).toBe(1);
    expect(steeringQueueLimit(Number.POSITIVE_INFINITY)).toBe(DEFAULT_STEERING_QUEUE_LIMIT);
    expect(steeringQueueLimit(999)).toBe(MAX_STEERING_QUEUE_LIMIT);
    expect(normalizeSteeringQueue([
      queued("a", " First "),
      queued("a", "duplicate id"),
      queued("b", "First"), // repeated text is a valid deliberate second prompt
      queued("c", " "),
    ], 3)).toEqual([queued("a", "First"), queued("b", "First")]);
  });

  it("enqueues without crossing the cap and distinguishes refusal causes", () => {
    expect(enqueueSteeringPrompt([], queued("a"), 2)).toEqual({
      items: [queued("a")], outcome: "added",
    });
    expect(enqueueSteeringPrompt([queued("a")], queued("a"), 2).outcome)
      .toBe("duplicate-id");
    expect(enqueueSteeringPrompt([queued("a"), queued("b")], queued("c"), 2).outcome)
      .toBe("full");
    expect(enqueueSteeringPrompt([], queued("bad", " "), 2).outcome).toBe("invalid");
  });

  it("edits, removes, and clears without treating a blank edit as a deletion", () => {
    const items = [{ ...queued("a"), context: [fileContext] }, queued("b")];
    expect(editSteeringPrompt(items, "a", " Updated ")[0].text).toBe("Updated");
    expect(editSteeringPrompt(items, "a", " Updated ")[0].context).toEqual([fileContext]);
    expect(editSteeringPrompt(items, "a", " ")).toEqual(items);
    expect(editSteeringPrompt(items, "a", "/clear")).toEqual(items);
    expect(editSteeringPrompt(items, "a", " /compact focus ")).toEqual(items);
    // Double-slash comments remain ordinary prompt text.
    expect(editSteeringPrompt(items, "a", "// comment")[0].text).toBe("// comment");
    expect(removeSteeringPrompt(items, "a")).toEqual([queued("b")]);
    expect(clearSteeringQueue()).toEqual([]);
  });

  it("persists pause and in-flight delivery guards until an explicit decision", () => {
    const paused = pauseSteeringQueue([queued("a"), queued("b")]);
    expect(paused.map(item => item.deliveryState)).toEqual(["paused", "paused"]);
    const inflight = markSteeringPromptInflight(paused, "a");
    expect(inflight[0].deliveryState).toBe("inflight");
    // Pause is durable across a second Stop: an in-flight row keeps its marker.
    expect(pauseSteeringQueue(inflight).map(item => item.deliveryState))
      .toEqual(["inflight", "paused"]);
    expect(releaseSteeringPrompt(inflight, "a")[0].deliveryState).toBeUndefined();
    expect(parseSteeringQueueMap(serializeSteeringQueueMap({ key: inflight })))
      .toEqual({ key: inflight });
  });

  it("normalizes typed context without allowing corrupt restored payloads to throw", () => {
    const queue = normalizeSteeringQueue([{
      ...queued("a", " Use context "),
      context: [
        { ...fileContext, label: " App.tsx " },
        { ...fileContext, id: "duplicate", label: "duplicate" },
        { id: "bad", kind: "file", label: "Bad", value: "/bad", detail: 42 },
      ],
    } as unknown as SteeringQueueItem]);
    expect(queue).toEqual([{
      ...queued("a", "Use context"), context: [fileContext],
    }]);
  });

  it("moves by one or to either edge and refuses impossible moves", () => {
    const items = [queued("a"), queued("b"), queued("c")];
    expect(moveSteeringPrompt(items, "b", "up").map(item => item.id))
      .toEqual(["b", "a", "c"]);
    expect(moveSteeringPrompt(items, "b", "down").map(item => item.id))
      .toEqual(["a", "c", "b"]);
    expect(moveSteeringPrompt(items, "c", "first").map(item => item.id))
      .toEqual(["c", "a", "b"]);
    expect(moveSteeringPrompt(items, "a", "last").map(item => item.id))
      .toEqual(["b", "c", "a"]);
    expect(moveSteeringPrompt(items, "a", "up")).toEqual(items);
  });

  it("maps explicit Alt keyboard chords to bounded reorder actions", () => {
    expect(steeringQueueKeyAction(1, 3, "ArrowUp", true))
      .toEqual({ type: "move", move: "up" });
    expect(steeringQueueKeyAction(1, 3, "ArrowDown", true))
      .toEqual({ type: "move", move: "down" });
    expect(steeringQueueKeyAction(2, 3, "Home", true))
      .toEqual({ type: "move", move: "first" });
    expect(steeringQueueKeyAction(0, 3, "End", true))
      .toEqual({ type: "move", move: "last" });
    expect(steeringQueueKeyAction(0, 3, "ArrowUp", true)).toBeNull();
    expect(steeringQueueKeyAction(1, 3, "ArrowUp", false)).toBeNull();
  });

  it("builds a short single-line accessible item name", () => {
    expect(steeringQueueItemName(queued("a", "one\n two   three"))).toBe("one two three");
    expect(steeringQueueItemName(queued("a", "abcdefghij"), 6)).toBe("abcde…");
  });

  it("auto-delivers only one healthy idle queue and parks a rejected item", () => {
    const ready = {
      dispatching: false, waitingForTurn: false, hasError: false,
      live: true, status: "idle",
    };
    expect(steeringQueueCanAutoDeliver(1, ready)).toBe(true);
    expect(steeringQueueCanAutoDeliver(0, ready)).toBe(false);
    expect(steeringQueueCanAutoDeliver(1, { ...ready, hasError: true })).toBe(false);
    expect(steeringQueueCanAutoDeliver(1, { ...ready, waitingForTurn: true })).toBe(false);
    expect(steeringQueueCanAutoDeliver(1, { ...ready, status: "working" })).toBe(false);
    expect(steeringQueueCanAutoDeliver(1, { ...ready, live: false })).toBe(false);
    expect(steeringQueueCanAutoDeliver(1, {
      ...ready, firstDeliveryState: "inflight",
    })).toBe(false);
    expect(steeringQueueCanAutoDeliver(1, {
      ...ready, firstDeliveryState: "paused",
    })).toBe(false);
  });

  it("separates a bad turn end from a clean idle for the durable-pause decision", () => {
    // These three must arm the durable pause so a dead or failed session never
    // auto-delivers stale prompts on the next reattached idle.
    expect(steeringQueueTerminalFailure({ kind: "session.closed" })).toBe(true);
    expect(steeringQueueTerminalFailure({ kind: "status.changed", status: "failed" }))
      .toBe(true);
    expect(steeringQueueTerminalFailure({ kind: "error", retryable: false })).toBe(true);
    // A clean idle drains the queue; a retryable error is not terminal at all.
    expect(steeringQueueTerminalFailure({ kind: "status.changed", status: "idle" }))
      .toBe(false);
    expect(steeringQueueTerminalFailure({ kind: "status.changed", status: "working" }))
      .toBe(false);
    expect(steeringQueueTerminalFailure({ kind: "error", retryable: true })).toBe(false);
    expect(steeringQueueTerminalFailure({ kind: "message.delta" })).toBe(false);
  });

  it("recognizes provider response activity even without a working status edge", () => {
    expect(steeringQueueEventStartsTurn({ kind: "status.changed", status: "working" }))
      .toBe(true);
    expect(steeringQueueEventStartsTurn({ kind: "thinking.signal", active: true }))
      .toBe(true);
    expect(steeringQueueEventStartsTurn({ kind: "message.delta" })).toBe(true);
    expect(steeringQueueEventStartsTurn({ kind: "tool.call" })).toBe(true);
    expect(steeringQueueEventStartsTurn({ kind: "status.changed", status: "idle" }))
      .toBe(false);
    expect(steeringQueueEventStartsTurn({ kind: "thinking.signal", active: false }))
      .toBe(false);
  });

  it("replays a send-local working and terminal pair without accepting an old idle", () => {
    expect(outboundSettledDuringSend([
      { kind: "status.changed", status: "working" },
      { kind: "status.changed", status: "idle" },
    ])).toBe(true);
    expect(outboundSettledDuringSend([
      { kind: "status.changed", status: "idle" },
    ])).toBe(false);
    expect(outboundSettledDuringSend([
      { kind: "status.changed", status: "working" },
      { kind: "status.changed", status: "idle" },
      { kind: "status.changed", status: "working" },
    ])).toBe(false);
    expect(outboundSettledDuringSend([
      { kind: "status.changed", status: "working" },
      { kind: "status.changed", status: "idle" },
      { kind: "status.changed", status: "working" },
      { kind: "status.changed", status: "idle" },
    ])).toBe(true);
    expect(outboundSettledDuringSend([{ kind: "session.closed" }])).toBe(true);
  });

  it("does not capture an old turn settling while send validation is still running", () => {
    type LifecycleEvent = { kind: string; status?: string; retryable?: boolean };
    let buffer: { attempt: number; events: LifecycleEvent[] } | undefined;

    captureOutboundSendEvent(buffer, 7, { kind: "status.changed", status: "working" });
    captureOutboundSendEvent(buffer, 7, { kind: "status.changed", status: "idle" });
    buffer = { attempt: 7, events: [] };
    expect(buffer.events).toEqual([]);
    expect(outboundSettledDuringSend(buffer.events)).toBe(false);

    captureOutboundSendEvent(buffer, 6, { kind: "status.changed", status: "working" });
    expect(buffer.events).toEqual([]);
    captureOutboundSendEvent(buffer, 7, { kind: "status.changed", status: "working" });
    captureOutboundSendEvent(buffer, 7, { kind: "status.changed", status: "idle" });
    expect(outboundSettledDuringSend(buffer.events)).toBe(true);
  });

  it("parses only bounded object maps and fails closed on malformed storage", () => {
    expect(parseSteeringQueueMap(null)).toEqual({});
    expect(parseSteeringQueueMap("not json")).toEqual({});
    expect(parseSteeringQueueMap("[]")).toEqual({});
    expect(parseSteeringQueueMap("42")).toEqual({});

    const parsed = parseSteeringQueueMap(JSON.stringify({
      alpha: [
        { id: " a ", text: " First ", context: [fileContext] },
        { id: "b", text: "Second" },
        { id: 9, text: "corrupt" },
      ],
      beta: [queued("c")],
    }), 1, 1);
    expect(parsed).toEqual({
      alpha: [{ id: "a", text: "First", context: [fileContext] }],
    });
  });

  it("serializes the same normalized bounded map the parser accepts", () => {
    const serialized = serializeSteeringQueueMap({
      alpha: [queued("a", " One "), queued("b")],
      empty: [],
      beta: [queued("c")],
    }, 1, 1);
    expect(JSON.parse(serialized)).toEqual({ alpha: [queued("a", "One")] });
    expect(parseSteeringQueueMap(serialized)).toEqual({ alpha: [queued("a", "One")] });
  });
});

describe("SteeringQueue markup", () => {
  const callbacks = {
    onMove: () => undefined,
    onEdit: () => undefined,
    onRemove: () => undefined,
    onSendNow: () => undefined,
    onClear: () => undefined,
  };
  const markup = renderToStaticMarkup(createElement(SteeringQueue, {
    items: [
      { ...queued("a", "Run the focused checks"), context: [fileContext] },
      queued("b", "Then explain failures"),
    ],
    maxItems: 5,
    ...callbacks,
  }));

  it("ships an ordered, keyboard-reorderable region with named actions", () => {
    expect(markup).toMatch(/<section[^>]*aria-labelledby=/);
    expect(markup).toContain("Steering queue");
    expect(markup).toContain("2/5");
    expect(markup).toContain('aria-label="Prompts waiting to be sent"');
    expect(markup.match(/aria-keyshortcuts="Alt\+ArrowUp Alt\+ArrowDown Alt\+Home Alt\+End"/g))
      .toHaveLength(2);
    expect(markup).toContain('aria-label="Move queued prompt 1 later"');
    expect(markup).toContain('aria-label="Edit queued prompt 1"');
    expect(markup).toContain('aria-label="Send queued prompt 1 now"');
    expect(markup).toContain('aria-label="Remove queued prompt 1"');
    expect(markup).toContain('class="steering-queue-cli-act"');
    expect(markup).toContain('class="steering-queue-caret"');
    expect(markup).toContain("&gt;");
    expect(markup).toContain(">↑<");
    expect(markup).toContain(">↓<");
    expect(markup).toContain(">~<");
    expect(markup).toContain(">▶<");
    expect(markup).toContain(">×<");
    expect(STEERING_QUEUE_CSS).toContain(".steering-queue-cli-act,");
    expect(STEERING_QUEUE_CSS).toContain(".steering-queue-caret { display:none; }");
    expect(markup).toContain('aria-label="1 attached context item"');
    expect(markup).toContain(">1 context<");
    expect(markup).not.toContain("/repo/App.tsx");
    expect(markup).toContain('role="status" aria-live="polite"');
  });

  it("renders a useful empty state and no destructive clear action", () => {
    const empty = renderToStaticMarkup(createElement(SteeringQueue, {
      items: [], ...callbacks,
    }));
    expect(empty).toContain('class="steering-queue-empty" role="status"');
    expect(empty).toContain("No queued prompts.");
    expect(empty).not.toContain(">Clear<");
  });

  it("uses app tokens and no literal color values", () => {
    expect(STEERING_QUEUE_CSS).toContain("var(--surface)");
    expect(STEERING_QUEUE_CSS).toContain("var(--row-hover)");
    expect(STEERING_QUEUE_CSS).toContain("var(--focus)");
    expect(STEERING_QUEUE_CSS).not.toMatch(/#[0-9a-f]{3,8}/i);
  });

  it("restores the same action or nearest row after edit and remove mutations", () => {
    // Focus the next row after removal, or the previous row when the last is
    // removed — the rule SessionLibrary shares.
    expect(logicalListFocusIndex(1, 3)).toBe(1);
    expect(logicalListFocusIndex(2, 2)).toBe(1);
    expect(logicalListFocusIndex(0, 0)).toBe(-1);
    const source = SteeringQueue.toString();
    expect(source).toContain("pendingFocus");
    expect(source).toContain("finishEdit");
    expect(source).toContain("queueActionRefKey");
    expect(source).toContain("preferred.focus");
  });

  it("does not paint hover feedback on disabled Clear", () => {
    const disabled = renderToStaticMarkup(createElement(SteeringQueue, {
      items: [queued("a")], disabled: true, ...callbacks,
    }));
    expect(disabled).toMatch(/class="steering-queue-clear" disabled=""/);
    expect(STEERING_QUEUE_CSS).toContain(".steering-queue-clear:not(:disabled):hover");
    expect(STEERING_QUEUE_CSS).toContain(".steering-queue-clear:disabled");
  });
});
