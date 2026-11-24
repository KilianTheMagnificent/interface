import type { PromptRecoveryInput } from "../shared/recovery";
import { describe, expect, it } from "vitest";
import type { EngineEvent, SessionKey } from "../shared/engine";
import {
  EFFECT_DEFINITELY_REJECTED_CODE, EFFECT_DEFINITELY_UNSENT_CODE, EFFECT_OUTCOME_UNKNOWN_CODE,
  encodeIpcErrorMessage,
} from "../shared/ipc-error";
import {
  OutboundLane, deliveryOutcomeUnknown, transmissionOutcomeUnknown, type OutboundLanePort,
} from "./outbound-lane";
import type { PromptContextItem } from "./prompt-context";
import {
  DEFAULT_STEERING_QUEUE_LIMIT, editSteeringPrompt, parseSteeringQueueMap, removeSteeringPrompt,
  type SteeringQueueItem,
} from "./steering-queue";

/* The provider send lane is the renderer's one outbound path for prompts and
   provider commands, and it is covered by the fail-closed rules in
   electron/AGENTS.md. These tests drive it through a fake port: every provider
   effect, storage write and App fact is observable and scriptable here. */

const A: SessionKey = "claude:a";
const B: SessionKey = "codex:b";

const fileContext: PromptContextItem = {
  id: "ctx-file", kind: "file", label: "notes.md", value: "/repo/notes.md",
};
const urlContext: PromptContextItem = {
  id: "ctx-url", kind: "url", label: "spec", value: "https://example.com/spec",
};

const working: EngineEvent = { kind: "status.changed", status: "working" };
const idle: EngineEvent = { kind: "status.changed", status: "idle" };
const failed: EngineEvent = { kind: "status.changed", status: "failed" };
const closed: EngineEvent = { kind: "session.closed" };

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

function deferred<T = void>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

/** Let every queued continuation run; the lane never waits on a timer. */
async function flush(): Promise<void> {
  for (let i = 0; i < 30; i++) await Promise.resolve();
}

function laneHarness(options: {
  initial?: Record<SessionKey, SteeringQueueItem[]>;
  live?: readonly SessionKey[];
} = {}) {
  const log: string[] = [];
  const stored: string[] = [];
  const sends: Array<{ key: SessionKey; prompt: string; recovery?: PromptRecoveryInput } & Deferred<unknown>> = [];
  /** The stored queue map at the instant of each transmission. */
  const storedAtSend: Array<Record<SessionKey, SteeringQueueItem[]>> = [];
  const live = new Set<SessionKey>(options.live ?? [A]);
  const lifecycles = new Map<SessionKey, number>();
  const pendingConfig = new Set<SessionKey>();
  const status = new Map<SessionKey, string>([[A, "idle"], [B, "idle"]]);
  const errors: string[] = [];
  const restored: Array<{ key: SessionKey; ids: string[] }> = [];
  const delivered: SessionKey[] = [];
  const interrupts: Array<{ key: SessionKey } & Deferred<unknown>> = [];
  let interruptMode: "resolve" | "manual" = "resolve";
  let failPersists = 0;
  let validation: ((items: readonly PromptContextItem[]) => Promise<PromptContextItem[]>)
    | undefined;
  let nextId = 0;
  const storedQueues = (): Record<SessionKey, SteeringQueueItem[]> =>
    parseSteeringQueueMap(stored.at(-1) ?? null);
  const port: OutboundLanePort = {
    send: (key, prompt, recovery) => {
      log.push(`send ${key} ${prompt}`);
      storedAtSend.push(storedQueues());
      const pending = deferred<unknown>();
      sends.push({ key, prompt, recovery, ...pending });
      return pending.promise;
    },
    interrupt: key => {
      log.push(`interrupt ${key}`);
      const pending = deferred<unknown>();
      interrupts.push({ key, ...pending });
      if (interruptMode === "resolve") pending.resolve(undefined);
      return pending.promise;
    },
    validateContext: async (key, items) => {
      log.push(`validate ${key}`);
      if (items.length === 0) return [];
      return validation === undefined ? [...items] : validation(items);
    },
    restoreAccessMode: () => undefined,
    isLive: key => live.has(key),
    lifecycle: key => lifecycles.get(key) ?? 0,
    configPending: key => pendingConfig.has(key),
    persist: serialized => {
      if (failPersists > 0) {
        failPersists -= 1;
        log.push("persist failed");
        return false;
      }
      stored.push(serialized);
      log.push("persist");
      return true;
    },
    publishDraft: key => { log.push(`draft ${key}`); },
    checkpoint: key => { log.push(`checkpoint ${key}`); },
    reportError: message => { errors.push(message); },
    markDelivered: key => { delivered.push(key); },
    restoreContext: (key, items) => { restored.push({ key, ids: items.map(item => item.id) }); },
    queueItemId: () => `queued:${(++nextId).toString()}`,
    now: () => 1_000,
  };
  const lane = new OutboundLane(port, options.initial ?? {});
  /** What App's two post-render effects do, in their declaration order. */
  const render = (): void => {
    lane.retryUnsavedQueues();
    const snapshot = lane.snapshot();
    lane.autoDeliver(snapshot.queues, snapshot.errors, key => status.get(key));
  };
  return {
    lane, port, log, stored, storedQueues, storedAtSend, sends, interrupts, live, lifecycles,
    pendingConfig, status, errors, restored, delivered, render,
    failNextPersists(count = 1) { failPersists = count; },
    holdInterrupts() { interruptMode = "manual"; },
    holdValidation() {
      const gate = deferred<PromptContextItem[]>();
      validation = () => gate.promise;
      return gate;
    },
    sendCount: () => sends.length,
    queue: (key: SessionKey = A) => lane.snapshot().queues[key] ?? [],
  };
}

const queued = (id: string, text: string,
  deliveryState?: SteeringQueueItem["deliveryState"]): SteeringQueueItem => ({
  id, text, ...(deliveryState === undefined ? {} : { deliveryState }),
});

/**
 * A Rust host error as `window.iface` rejects with it: the words, then the v1
 * error shape (shared/ipc-error.ts) the bridge appends and the preload keeps.
 */
const hostError = (message: string, code: string, replaySafe: boolean, retryable = false): Error =>
  new Error(encodeIpcErrorMessage(message, { code, retryable, replaySafe }));
/** The host's proof that nothing reached the provider. */
const unsent = (message: string): Error => hostError(message, EFFECT_DEFINITELY_UNSENT_CODE, true);
/** The host's proof that the provider refused it. */
const rejected = (message: string): Error =>
  hostError(message, EFFECT_DEFINITELY_REJECTED_CODE, false);
/** The adapter could not say whether the provider took it. */
const ambiguous = (message: string): Error => hostError(message, EFFECT_OUTCOME_UNKNOWN_CODE, false);

describe("transmissionOutcomeUnknown", () => {
  it("treats every host error without a proof of non-delivery as unknown", () => {
    // Rust's OutcomeUnknown sends, whose words carry no uncertainty phrase.
    for (const message of [
      "Claude's input transport closed during delivery",
      "Claude input write failed: Broken pipe (os error 32)",
      "Codex writer ended after accepting the frame",
      "Codex writer did not confirm the accepted frame",
      "The session changed, but its registry update could not reach the desktop.",
    ]) expect(transmissionOutcomeUnknown(ambiguous(message)), message).toBe(true);
    // The host dying or being stopped under a request, and a broken protocol.
    for (const [code, retryable] of [
      ["host-exited", true], ["host-stopped", true], ["host-protocol", false],
      ["desktop-worker-failed", false], ["response-too-large", false],
      ["runtime-response-mismatch", false], ["COLLABORATION_STATE_CORRUPT", false],
    ] as const) {
      expect(transmissionOutcomeUnknown(hostError("gone", code, false, retryable)), code).toBe(true);
    }
  });

  it("keeps a proven refusal definite", () => {
    expect(transmissionOutcomeUnknown(unsent("Claude is not running"))).toBe(false);
    expect(transmissionOutcomeUnknown(rejected("Codex rejected the turn"))).toBe(false);
    for (const [code, retryable] of [
      ["invalid-payload", false], ["host-not-ready", true], ["host-backpressure", true],
      ["request-too-large", false], ["host-write", true], ["host-spawn", true],
    ] as const) {
      expect(transmissionOutcomeUnknown(hostError("refused", code, true, retryable)), code).toBe(false);
    }
  });

  it("classifies a rejection without the typed shape as unknown", () => {
    // The pre-change shape: the words alone, as a build before the shape (or
    // main's own refusal) delivers them. Words cannot prove nothing was sent.
    expect(transmissionOutcomeUnknown(new Error(
      "Error invoking remote method 'iface:send': Error: provider refused"))).toBe(true);
    expect(transmissionOutcomeUnknown(new Error("Claude is not running"))).toBe(true);
    expect(transmissionOutcomeUnknown("not an error")).toBe(true);
    expect(transmissionOutcomeUnknown(new Error("Maybe. [iface-error/2 code=effect-definitely-unsent]")))
      .toBe(true);
  });

  it("still honours an adapter's uncertainty phrase or the unknown code over replay-safe", () => {
    expect(transmissionOutcomeUnknown(unsent("Codex did not confirm whether the turn started")))
      .toBe(true);
    expect(transmissionOutcomeUnknown(hostError("Inconsistent.", EFFECT_OUTCOME_UNKNOWN_CODE, true)))
      .toBe(true);
  });
});

describe("deliveryOutcomeUnknown", () => {
  it("keeps a failure before transmission definite unless its words say otherwise", () => {
    // Settings pending, cancellation, validation and the in-flight mark never
    // let the prompt leave; they reach here as-is, not as a transmission.
    expect(deliveryOutcomeUnknown(new Error("Claude is not running"))).toBe(false);
    expect(deliveryOutcomeUnknown(hostError("The Rust host stopped unexpectedly (exit 1).",
      "host-exited", false, true))).toBe(false);
    expect(deliveryOutcomeUnknown(new Error(
      "Error invoking remote method 'iface:send': Error: Codex did not confirm whether the turn started",
    ))).toBe(true);
    expect(deliveryOutcomeUnknown(new Error("Delivery Outcome Unknown for that frame"))).toBe(true);
    expect(deliveryOutcomeUnknown("provider did not confirm whether it ran")).toBe(true);
  });
});

describe("OutboundLane composer sends", () => {
  it("validates, transmits, reasserts the draft and publishes delivery", async () => {
    const h = laneHarness();
    const sent = h.lane.submitPrompt(A, "hello", []);
    expect(h.lane.isSending(A)).toBe(true);
    await flush();
    expect(h.log).toEqual([`validate ${A}`, `send ${A} hello`]);
    h.sends[0].resolve(undefined);
    await sent;
    expect(h.log).toEqual([`validate ${A}`, `send ${A} hello`, `draft ${A}`]);
    expect(h.delivered).toEqual([A]);
    expect(h.lane.isSending(A)).toBe(false);
    // Accepted but the provider's turn has not ended: still an outbound pending.
    expect(h.lane.outboundPending(A)).toBe(true);
  });

  it("refuses a repeated submit while the first is validating, with no second transmit", async () => {
    const h = laneHarness();
    const gate = h.holdValidation();
    const first = h.lane.submitPrompt(A, "one", [fileContext]);
    await expect(h.lane.submitPrompt(A, "two", [urlContext])).rejects.toThrow(
      "Another prompt is already being delivered. Queue this follow-up instead.");
    // The refused submit's own captured context goes back; the first keeps its.
    expect(h.restored).toEqual([{ key: A, ids: ["ctx-url"] }]);
    gate.resolve([fileContext]);
    await flush();
    expect(h.sendCount()).toBe(1);
    expect(h.sends[0].prompt).toContain("Task:\none");
    h.sends[0].resolve(undefined);
    await first;
  });

  it("holds the lane from acceptance until the provider's turn ends", async () => {
    const h = laneHarness();
    const first = h.lane.submitPrompt(A, "one", []);
    await flush();
    h.sends[0].resolve(undefined);
    await first;
    await expect(h.lane.submitPrompt(A, "two", [])).rejects.toThrow(
      "Another prompt is already being delivered.");
    expect(h.sendCount()).toBe(1);
    // Acceptance counts as the turn starting, so a bare terminal edge (a
    // Claude result with no deltas) releases the latch.
    h.lane.observe(A, idle);
    expect(h.lane.outboundPending(A)).toBe(false);
    const third = h.lane.submitPrompt(A, "three", []);
    await flush();
    expect(h.sends.map(send => send.prompt)).toEqual(["one", "three"]);
    h.sends[1].resolve(undefined);
    await third;
  });

  it("keeps sessions independent", async () => {
    const h = laneHarness({ live: [A, B] });
    const a = h.lane.submitPrompt(A, "for a", []);
    const b = h.lane.submitPrompt(B, "for b", []);
    await flush();
    expect(h.sends.map(send => `${send.key} ${send.prompt}`)).toEqual([`${A} for a`, `${B} for b`]);
    h.sends[0].resolve(undefined);
    h.sends[1].resolve(undefined);
    await Promise.all([a, b]);
  });

  it("does not arm a stale latch when the provider settles inside send()", async () => {
    const h = laneHarness();
    const sent = h.lane.submitPrompt(A, "fast", []);
    await flush();
    h.lane.observe(A, working);
    h.lane.observe(A, idle);
    h.sends[0].resolve(undefined);
    await sent;
    expect(h.lane.outboundPending(A)).toBe(false);
  });

  it("ignores lifecycle edges seen before transmission (an old turn's terminal)", async () => {
    const h = laneHarness();
    const gate = h.holdValidation();
    const sent = h.lane.submitPrompt(A, "later", [fileContext]);
    h.lane.observe(A, working);
    h.lane.observe(A, idle);
    gate.resolve([fileContext]);
    await flush();
    h.sends[0].resolve(undefined);
    await sent;
    expect(h.lane.outboundPending(A)).toBe(true);
  });

  it("refuses to send under a pending settings change, before and after validation", async () => {
    const h = laneHarness();
    h.pendingConfig.add(A);
    await expect(h.lane.submitPrompt(A, "early", [fileContext])).rejects.toThrow(
      "Wait for the session settings change to finish before sending.");
    expect(h.log).toEqual([]);
    expect(h.restored).toEqual([{ key: A, ids: ["ctx-file"] }]);
    h.pendingConfig.delete(A);
    const gate = h.holdValidation();
    const late = h.lane.submitPrompt(A, "late", [fileContext]);
    h.pendingConfig.add(A);
    gate.resolve([fileContext]);
    await expect(late).rejects.toThrow(
      "Wait for the session settings change to finish before sending.");
    expect(h.sendCount()).toBe(0);
    // A refusal that reached validation still reasserts the draft.
    expect(h.log).toEqual([`validate ${A}`, `draft ${A}`]);
  });

  it("cancels an attempt stopped during validation before any bytes leave", async () => {
    const h = laneHarness();
    const gate = h.holdValidation();
    const sent = h.lane.submitPrompt(A, "stop me", [fileContext]);
    h.lane.pause(A, "Queue paused after Stop. Use Send now to resume queued follow-ups.");
    gate.resolve([fileContext]);
    await expect(sent).rejects.toThrow("Prompt delivery was cancelled before it started.");
    expect(h.sendCount()).toBe(0);
    expect(h.restored).toEqual([{ key: A, ids: ["ctx-file"] }]);
  });

  it("never sends into a replacement handle after the session closed during validation", async () => {
    const h = laneHarness();
    const gate = h.holdValidation();
    const sent = h.lane.submitPrompt(A, "stale", [fileContext]);
    h.lifecycles.set(A, 1);
    gate.resolve([fileContext]);
    await expect(sent).rejects.toThrow(
      "That session closed while prompt context was being checked. "
      + "Nothing was sent; review the prompt and send it again after Resume.");
    expect(h.sendCount()).toBe(0);
  });

  it("interrupts again when Stop races an accepted send, and never resends", async () => {
    const h = laneHarness();
    const sent = h.lane.submitPrompt(A, "raced", []);
    await flush();
    h.lane.pause(A, "Queue paused after Stop. Use Send now to resume queued follow-ups.");
    h.sends[0].resolve(undefined);
    await sent;
    expect(h.interrupts.map(call => call.key)).toEqual([A]);
    expect(h.errors).toEqual([
      "The provider accepted the prompt as Stop was pressed, so Interface sent a second "
      + "interrupt after the turn became available. The prompt will not be sent again."]);
    expect(h.sendCount()).toBe(1);
    expect(h.lane.outboundPending(A)).toBe(false);
  });

  it("reports a failed follow-up interrupt without resending", async () => {
    const h = laneHarness();
    h.holdInterrupts();
    const sent = h.lane.submitPrompt(A, "raced", []);
    await flush();
    h.lane.pause(A, "Queue paused after Stop.");
    h.sends[0].resolve(undefined);
    await flush();
    h.interrupts[0].reject(new Error("Error invoking remote method 'iface:interrupt': Error: gone"));
    await sent;
    expect(h.errors).toEqual([
      "The provider accepted the prompt as Stop was pressed, but the follow-up interrupt "
      + "failed: gone. The prompt will not be sent again."]);
    expect(h.sendCount()).toBe(1);
  });

  it("resolves an unknown outcome without restoring context or marking the echo undelivered", async () => {
    const h = laneHarness();
    const sent = h.lane.submitPrompt(A, "maybe", [fileContext]);
    await flush();
    h.sends[0].reject(new Error("Codex did not confirm whether the turn started"));
    await expect(sent).resolves.toBeUndefined();
    expect(h.restored).toEqual([]);
    expect(h.delivered).toEqual([]);
    expect(h.errors).toEqual(["Codex did not confirm whether the turn started Delivery outcome "
      + "unknown; Interface will not resend this prompt automatically."]);
    expect(h.lane.outboundPending(A)).toBe(false);
  });

  it("restores the captured context and rejects with the provider's words on a definite refusal", async () => {
    const h = laneHarness();
    const sent = h.lane.submitPrompt(A, "no", [fileContext]);
    await flush();
    h.sends[0].reject(new Error(
      `Error invoking remote method 'iface:send': Error: ${unsent("provider refused").message}`));
    await expect(sent).rejects.toThrow(/^provider refused$/u);
    expect(h.restored).toEqual([{ key: A, ids: ["ctx-file"] }]);
    expect(h.delivered).toEqual([]);
    expect(h.errors).toEqual([]);
    expect(h.lane.outboundPending(A)).toBe(false);
  });

  it("treats a host outcome-unknown send as unknown whatever its words", async () => {
    const h = laneHarness();
    const sent = h.lane.submitPrompt(A, "maybe", [fileContext]);
    await flush();
    h.sends[0].reject(ambiguous("Claude's input transport closed during delivery"));
    await expect(sent).resolves.toBeUndefined();
    expect(h.restored).toEqual([]);
    expect(h.delivered).toEqual([]);
    expect(h.errors).toEqual(["Claude's input transport closed during delivery Delivery outcome "
      + "unknown; Interface will not resend this prompt automatically."]);
    expect(h.sendCount()).toBe(1);
  });

  it("treats a send the host died under, or a rejection without the shape, as unknown", async () => {
    for (const failure of [
      hostError("The Rust host stopped unexpectedly (exit 1).", "host-exited", false, true),
      hostError("The Rust host was stopped by the application.", "host-stopped", false, true),
      new Error("Error invoking remote method 'iface:send': Error: provider refused"),
    ]) {
      const h = laneHarness();
      const sent = h.lane.submitPrompt(A, "maybe", [fileContext]);
      await flush();
      h.sends[0].reject(failure);
      await expect(sent, failure.message).resolves.toBeUndefined();
      expect(h.restored, failure.message).toEqual([]);
    }
  });

  it("keeps a host failure before transmission definite: nothing left", async () => {
    const h = laneHarness();
    const gate = h.holdValidation();
    const sent = h.lane.submitPrompt(A, "never sent", [fileContext]);
    await flush();
    gate.reject(hostError("The Rust host stopped unexpectedly (exit 1).", "host-exited", false, true));
    await expect(sent).rejects.toThrow(/^The Rust host stopped unexpectedly \(exit 1\)\.$/u);
    expect(h.sendCount()).toBe(0);
    expect(h.restored).toEqual([{ key: A, ids: ["ctx-file"] }]);
  });

  it("classifies provider commands the same way, without context to restore", async () => {
    const h = laneHarness();
    const unknown = h.lane.runCommand(A, "/compact");
    await flush();
    h.sends[0].reject(new Error("delivery outcome unknown"));
    await expect(unknown).resolves.toBeUndefined();
    expect(h.errors).toEqual(["delivery outcome unknown Command outcome unknown; "
      + "Interface will not run it again automatically."]);
    h.lane.observe(A, idle);
    const refused = h.lane.runCommand(A, "/compact");
    await flush();
    h.sends[1].reject(rejected("Error: no such command"));
    await expect(refused).rejects.toThrow(/^no such command$/u);
    expect(h.restored).toEqual([]);
    const accepted = h.lane.runCommand(A, "/compact");
    await flush();
    h.sends[2].resolve(undefined);
    await accepted;
    expect(h.delivered).toEqual([A]);
  });

  it("waits for the access-mode restore before the first transmission", async () => {
    const h = laneHarness();
    const restore = deferred();
    const order: string[] = [];
    const lane = new OutboundLane({
      ...h.port,
      restoreAccessMode: key => {
        order.push(`restore ${key}`);
        return restore.promise.then(() => { order.push("restored"); });
      },
      send: async (key, prompt) => { order.push(`send ${key} ${prompt}`); },
    }, {});
    const sent = lane.submitPrompt(A, "first", []);
    await flush();
    expect(order).toEqual([`restore ${A}`]);
    restore.resolve(undefined);
    await sent;
    expect(order).toEqual([`restore ${A}`, "restored", `send ${A} first`]);
  });
});

describe("OutboundLane steering queue", () => {
  it("persists the in-flight marker before the queued prompt is transmitted", async () => {
    const h = laneHarness();
    h.status.set(A, "working");
    expect(h.lane.enqueue(A, "queued one", [])).toBe(true);
    h.render();
    expect(h.sendCount()).toBe(0);
    h.status.set(A, "idle");
    h.render();
    const id = h.queue()[0].id;
    expect(h.lane.snapshot().sending[A]).toBe(id);
    await flush();
    expect(h.log.slice(h.log.indexOf(`checkpoint ${A}`))).toEqual([
      `checkpoint ${A}`, `validate ${A}`, "persist", `send ${A} queued one`]);
    expect(h.storedAtSend[0][A]).toEqual([{ id, text: "queued one", createdAt: 1_000,
      deliveryState: "inflight" }]);
    h.sends[0].resolve(undefined);
    await flush();
    expect(h.queue()).toEqual([]);
    expect(h.storedQueues()[A]).toBeUndefined();
    expect(h.lane.snapshot().sending[A]).toBeUndefined();
    expect(h.lane.snapshot().errors[A]).toBeUndefined();
  });

  it("delivers a session's queue in order, one prompt per provider turn", async () => {
    const h = laneHarness();
    h.status.set(A, "working");
    for (const text of ["first", "second", "third"]) h.lane.enqueue(A, text, []);
    h.render();
    h.status.set(A, "idle");
    for (let turn = 0; turn < 3; turn++) {
      h.render();
      await flush();
      h.render();
      expect(h.sendCount()).toBe(turn + 1);
      h.sends[turn].resolve(undefined);
      await flush();
      // Accepted: the next row waits for this turn's terminal edge.
      h.render();
      await flush();
      expect(h.sendCount()).toBe(turn + 1);
      h.lane.observe(A, working);
      h.lane.observe(A, idle);
    }
    expect(h.sends.map(send => send.prompt)).toEqual(["first", "second", "third"]);
    expect(h.queue()).toEqual([]);
  });

  it("drains two sessions' queues independently", async () => {
    const h = laneHarness({ live: [A, B] });
    h.status.set(A, "working");
    h.status.set(B, "working");
    h.lane.enqueue(A, "a1", []);
    h.lane.enqueue(B, "b1", []);
    h.render();
    h.status.set(A, "idle");
    h.status.set(B, "idle");
    h.render();
    await flush();
    expect(h.sends.map(send => `${send.key} ${send.prompt}`).sort()).toEqual([`${A} a1`, `${B} b1`]);
  });

  it("does not auto-deliver unless the session is live, idle, settled and unblocked", async () => {
    const cases: Array<[string, (h: ReturnType<typeof laneHarness>) => void]> = [
      ["not live", h => { h.live.delete(A); }],
      ["working", h => { h.status.set(A, "working"); }],
      ["unknown status", h => { h.status.delete(A); }],
      ["settings pending", h => { h.pendingConfig.add(A); }],
      ["blocking error", h => {
        // A queue edit that could not be stored blocks without touching the rows.
        h.failNextPersists(1);
        expect(h.lane.applyQueueUpdate(A, items => [...items])).toBe(false);
      }],
    ];
    for (const [name, block] of cases) {
      const h = laneHarness();
      h.status.set(A, "working");
      h.lane.enqueue(A, "held", []);
      h.render();
      h.status.set(A, "idle");
      block(h);
      h.render();
      await flush();
      expect(h.sendCount(), name).toBe(0);
    }
    for (const state of ["inflight", "paused"] as const) {
      const h = laneHarness({ initial: { [A]: [queued("r1", "restored", state)] } });
      h.render();
      await flush();
      expect(h.sendCount(), state).toBe(0);
    }
  });

  it("parks a definitely refused queued prompt and blocks automatic retry", async () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "refused")] } });
    h.render();
    await flush();
    h.sends[0].reject(unsent("Error: provider refused"));
    await flush();
    expect(h.queue()).toEqual([{ id: "q1", text: "refused", deliveryState: "paused" }]);
    expect(h.storedQueues()[A]).toEqual([{ id: "q1", text: "refused", deliveryState: "paused" }]);
    expect(h.lane.snapshot().errors[A]).toBe(
      "provider refused The prompt is paused; use Send now to retry it.");
    h.lane.observe(A, idle);
    h.render();
    await flush();
    expect(h.sendCount()).toBe(1);
  });

  it("keeps an unknown-outcome queued prompt in flight and never resends it automatically", async () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "maybe")] } });
    h.render();
    await flush();
    h.sends[0].reject(new Error("Codex did not confirm whether the turn started"));
    await flush();
    expect(h.queue()).toEqual([{ id: "q1", text: "maybe", deliveryState: "inflight" }]);
    expect(h.storedQueues()[A]).toEqual([{ id: "q1", text: "maybe", deliveryState: "inflight" }]);
    expect(h.lane.snapshot().errors[A]).toBe("Codex did not confirm whether the turn started "
      + "Delivery outcome unknown; Interface will not retry this prompt automatically.");
    h.lane.observe(A, idle);
    h.render();
    await flush();
    expect(h.sendCount()).toBe(1);
  });

  it("keeps a queued row in flight when the host dies under its send", async () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "maybe")] } });
    h.render();
    await flush();
    h.sends[0].reject(hostError("The Rust host stopped unexpectedly (exit 1).", "host-exited",
      false, true));
    await flush();
    expect(h.queue()).toEqual([{ id: "q1", text: "maybe", deliveryState: "inflight" }]);
    expect(h.storedQueues()[A]).toEqual([{ id: "q1", text: "maybe", deliveryState: "inflight" }]);
    expect(h.lane.snapshot().errors[A]).toBe("The Rust host stopped unexpectedly (exit 1). "
      + "Delivery outcome unknown; Interface will not retry this prompt automatically.");
    h.lane.observe(A, idle);
    h.render();
    await flush();
    expect(h.sendCount()).toBe(1);
  });

  it("does not send a queued row removed while its context was being validated", async () => {
    const h = laneHarness();
    const gate = h.holdValidation();
    h.status.set(A, "working");
    h.lane.enqueue(A, "remove me", [fileContext]);
    h.render();
    h.status.set(A, "idle");
    h.render();
    const id = h.queue()[0].id;
    expect(h.lane.applyQueueUpdate(A, items => removeSteeringPrompt(items, id))).toBe(true);
    gate.resolve([fileContext]);
    await flush();
    expect(h.sendCount()).toBe(0);
    expect(h.lane.snapshot().errors[A]).toBeUndefined();
    expect(h.lane.snapshot().sending[A]).toBeUndefined();
  });

  it("sends nothing when the in-flight marker cannot be stored, and parks the row", async () => {
    const h = laneHarness();
    h.status.set(A, "working");
    h.lane.enqueue(A, "fragile", []);
    h.render();
    h.status.set(A, "idle");
    h.failNextPersists(1);
    h.render();
    await flush();
    expect(h.sendCount()).toBe(0);
    expect(h.queue()[0].deliveryState).toBe("paused");
    expect(h.storedQueues()[A]?.[0].deliveryState).toBe("paused");
    expect(h.lane.snapshot().errors[A]).toBe("The queued prompt could not be marked durable "
      + "before sending. Nothing was sent. The prompt is paused; use Send now to retry it.");
  });

  it("drops an accepted row from view when its removal cannot be stored, then retries the write", async () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "accepted")] } });
    h.render();
    await flush();
    h.failNextPersists(1);
    h.sends[0].resolve(undefined);
    await flush();
    expect(h.queue()).toEqual([]);
    expect(h.lane.snapshot().errors[A]).toBe("The provider accepted this prompt, but durable "
      + "queue storage could not be updated. It will not be sent again; review any Delivery "
      + "outcome unknown row after restart.");
    // Storage still holds the in-flight marker: never a row that could resend.
    expect(h.storedQueues()[A]).toEqual([{ id: "q1", text: "accepted", deliveryState: "inflight" }]);
    h.render();
    expect(h.storedQueues()[A]).toBeUndefined();
  });

  it("parks a refused row in memory when its retry state cannot be stored, then retries the write", async () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "refused")] } });
    h.render();
    await flush();
    h.failNextPersists(1);
    h.sends[0].reject(unsent("provider refused"));
    await flush();
    expect(h.queue()).toEqual([{ id: "q1", text: "refused", deliveryState: "paused" }]);
    expect(h.lane.snapshot().errors[A]).toBe("provider refused The retry state could not be "
      + "saved; review this queue before restarting.");
    expect(h.storedQueues()[A]).toEqual([{ id: "q1", text: "refused", deliveryState: "inflight" }]);
    h.render();
    expect(h.storedQueues()[A]).toEqual([{ id: "q1", text: "refused", deliveryState: "paused" }]);
  });

  it("refuses Send now on a paused row while the session is not live, keeping it paused", async () => {
    const h = laneHarness({ initial: { [A]: [queued("p1", "later", "paused")] }, live: [] });
    h.lane.sendQueuedNow(A, "p1");
    await flush();
    expect(h.sendCount()).toBe(0);
    expect(h.queue()).toEqual([{ id: "p1", text: "later", deliveryState: "paused" }]);
    expect(h.stored).toEqual([]);
    expect(h.lane.snapshot().errors[A]).toBe(
      "Resume this session before sending its queued prompts. They stay queued until then.");
    h.live.add(A);
    h.lane.sendQueuedNow(A, "p1");
    await flush();
    expect(h.sends.map(send => send.prompt)).toEqual(["later"]);
    expect(h.storedAtSend[0][A]).toEqual([{ id: "p1", text: "later", deliveryState: "inflight" }]);
    expect(h.lane.snapshot().errors[A]).toBeUndefined();
  });

  it("refuses a queued row that looks like a provider command", async () => {
    const h = laneHarness({ initial: { [A]: [queued("c1", "/compact")] } });
    h.lane.sendQueuedNow(A, "c1");
    await flush();
    expect(h.sendCount()).toBe(0);
    expect(h.lane.snapshot().errors[A]).toBe("This queued row looks like a provider command. "
      + "It was not sent; remove it and run the command directly.");
  });

  it("pauses armed rows on Stop and leaves an in-flight row alone", () => {
    const h = laneHarness({ initial: { [A]: [queued("i1", "sent?", "inflight"), queued("a1", "armed")] } });
    h.lane.pause(A, "Queue paused after Stop. Use Send now to resume queued follow-ups.");
    const expected = [queued("i1", "sent?", "inflight"), queued("a1", "armed", "paused")];
    expect(h.queue()).toEqual(expected);
    expect(h.storedQueues()[A]).toEqual(expected);
    expect(h.lane.snapshot().errors[A]).toBe(
      "Queue paused after Stop. Use Send now to resume queued follow-ups.");
  });

  it("says so when a pause could not be stored", () => {
    const h = laneHarness({ initial: { [A]: [queued("a1", "armed")] } });
    h.failNextPersists(1);
    h.lane.pause(A, "Queue paused because the live provider session was closed.");
    expect(h.queue()[0].deliveryState).toBe("paused");
    expect(h.lane.snapshot().errors[A]).toBe("Queue paused because the live provider session "
      + "was closed. The pause could not be saved; review it before restarting Interface.");
  });

  it("pauses armed rows when the provider ends badly, never on a clean idle", () => {
    const h = laneHarness({ initial: { [A]: [queued("a1", "armed")], [B]: [queued("b1", "armed")] } });
    h.lane.observe(A, idle);
    expect(h.queue()[0].deliveryState).toBeUndefined();
    h.lane.observe(A, failed);
    expect(h.queue()[0].deliveryState).toBe("paused");
    expect(h.lane.snapshot().errors[A]).toBe("Queue paused because the turn failed. Resolve the "
      + "error, then use Send now on each prompt to resume it.");
    h.lane.observe(B, closed);
    expect(h.queue(B)[0].deliveryState).toBe("paused");
    expect(h.lane.snapshot().errors[B]).toBe("Queue paused because the provider session ended. "
      + "Use Send now on each prompt to resume it.");
    h.lane.observe(B, { kind: "error", message: "boom", retryable: false });
    // Nothing armed is left, so the deliberate message stays.
    expect(h.lane.snapshot().errors[B]).toBe("Queue paused because the provider session ended. "
      + "Use Send now on each prompt to resume it.");
  });

  it("keeps rejected adds advisory, so accepted rows still drain", async () => {
    const h = laneHarness();
    h.status.set(A, "working");
    expect(h.lane.enqueue(A, "/compact", [])).toBe(false);
    expect(h.lane.snapshot().notices[A]).toBe("Provider commands cannot enter the steering queue. "
      + "Wait for the active turn, then run the command directly.");
    for (let i = 0; i < DEFAULT_STEERING_QUEUE_LIMIT; i++) {
      expect(h.lane.enqueue(A, `prompt ${i.toString()}`, [])).toBe(true);
    }
    expect(h.lane.enqueue(A, "one too many", [])).toBe(false);
    expect(h.lane.snapshot().notices[A]).toBe(
      "The steering queue is full. Send or remove a queued prompt before adding another.");
    expect(h.lane.snapshot().errors[A]).toBeUndefined();
    h.status.set(A, "idle");
    h.render();
    await flush();
    expect(h.sendCount()).toBe(1);
    // A row leaving clears the stale "queue full" advisory.
    expect(h.lane.snapshot().notices[A]).toBeUndefined();
  });

  it("keeps the draft when an add cannot be stored, and inherits a paused queue's state", () => {
    const h = laneHarness({ initial: { [A]: [queued("p1", "paused", "paused")] } });
    h.failNextPersists(1);
    expect(h.lane.enqueue(A, "unsaved", [])).toBe(false);
    expect(h.lane.snapshot().notices[A]).toBe(
      "This prompt could not be saved to the durable queue. Your draft was kept.");
    expect(h.queue()).toHaveLength(1);
    expect(h.lane.enqueue(A, "saved", [fileContext])).toBe(true);
    expect(h.queue()[1]).toEqual({ id: "queued:2", text: "saved", context: [fileContext],
      createdAt: 1_000, deliveryState: "paused" });
    expect(h.lane.snapshot().notices[A]).toBeUndefined();
  });

  it("reports a queue edit that could not be stored and changes nothing", () => {
    const h = laneHarness({ initial: { [A]: [queued("a1", "keep")] } });
    h.failNextPersists(1);
    expect(h.lane.applyQueueUpdate(A, () => [])).toBe(false);
    expect(h.queue()).toEqual([queued("a1", "keep")]);
    expect(h.lane.snapshot().errors[A]).toBe(
      "This queue change could not be saved. No queued prompts were changed.");
    expect(h.lane.applyQueueUpdate(A, () => [])).toBe(true);
    expect(h.lane.snapshot().queues[A]).toBeUndefined();
    expect(h.lane.snapshot().errors[A]).toBeUndefined();
  });

  it("forgets a deleted session and cancels its in-flight validation", async () => {
    const h = laneHarness({ initial: { [A]: [queued("a1", "gone")], [B]: [queued("b1", "stays")] } });
    const gate = h.holdValidation();
    const sent = h.lane.submitPrompt(A, "never", [fileContext]);
    h.lane.drop(A);
    expect(h.lane.snapshot().queues[A]).toBeUndefined();
    expect(h.storedQueues()).toEqual({ [B]: [queued("b1", "stays")] });
    expect(h.lane.isSending(A)).toBe(false);
    gate.resolve([fileContext]);
    await expect(sent).rejects.toThrow("Prompt delivery was cancelled before it started.");
    expect(h.sendCount()).toBe(0);
  });

  it("publishes a new snapshot whenever the lane's gates move", async () => {
    const h = laneHarness();
    const initial = h.lane.snapshot();
    const sent = h.lane.submitPrompt(A, "tick", []);
    const started = h.lane.snapshot();
    expect(started).not.toBe(initial);
    await flush();
    h.sends[0].resolve(undefined);
    await sent;
    const accepted = h.lane.snapshot();
    expect(accepted).not.toBe(started);
    h.lane.observe(A, idle);
    expect(h.lane.snapshot()).not.toBe(accepted);
  });
});

describe("OutboundLane queued prompt under edit", () => {
  /** A follow-up queued behind a running turn, open in the pane's editor. */
  const editingWhileWorking = () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "fix the tests")] } });
    h.status.set(A, "working");
    h.render();
    h.lane.setQueueEditing(A, "q1", true);
    // The agent's turn ends while the owner is still typing.
    h.status.set(A, "idle");
    h.lane.observe(A, idle);
    h.render();
    return h;
  };

  it("does not deliver a queued prompt open for editing when the turn ends", async () => {
    const h = editingWhileWorking();
    await flush();
    h.render();
    await flush();
    expect(h.sendCount()).toBe(0);
    expect(h.queue()).toEqual([queued("q1", "fix the tests")]);
  });

  it("delivers the saved text once the edit is saved, never the old words", async () => {
    const h = editingWhileWorking();
    await flush();
    // Save: the pane writes the new words, then its editor closes.
    expect(h.lane.applyQueueUpdate(A,
      items => editSteeringPrompt(items, "q1", "fix the tests and the lint"))).toBe(true);
    h.render();
    await flush();
    expect(h.sendCount()).toBe(0);
    h.lane.setQueueEditing(A, "q1", false);
    h.render();
    await flush();
    expect(h.sends.map(send => send.prompt)).toEqual(["fix the tests and the lint"]);
  });

  it("restores normal delivery when the edit is cancelled", async () => {
    const h = editingWhileWorking();
    await flush();
    h.lane.setQueueEditing(A, "q1", false);
    h.render();
    await flush();
    expect(h.sends.map(send => send.prompt)).toEqual(["fix the tests"]);
  });

  it("delivers the rows ahead of an edited row, then holds at it", async () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "first"), queued("q2", "second")] } });
    h.lane.setQueueEditing(A, "q2", true);
    h.render();
    await flush();
    expect(h.sends.map(send => send.prompt)).toEqual(["first"]);
    h.sends[0].resolve(undefined);
    await flush();
    h.lane.observe(A, working);
    h.lane.observe(A, idle);
    h.render();
    await flush();
    expect(h.sendCount()).toBe(1);
    expect(h.queue()).toEqual([queued("q2", "second")]);
  });

  it("clears queueEditing when a session is dropped so a recreated key is never wedged", async () => {
    const h = editingWhileWorking();
    h.lane.drop(A);
    expect(h.lane.enqueue(A, "fresh follow-up", [])).toBe(true);
    h.render();
    await flush();
    expect(h.sends.map(send => send.prompt)).toEqual(["fresh follow-up"]);
  });

  it("refuses editing a queued prompt into a slash command", () => {
    const h = laneHarness({ initial: { [A]: [queued("q1", "run checks")] } });
    expect(h.lane.applyQueueUpdate(A, items => editSteeringPrompt(items, "q1", "/clear")))
      .toBe(true);
    expect(h.queue()).toEqual([queued("q1", "run checks")]);
    expect(h.lane.applyQueueUpdate(A, items => editSteeringPrompt(items, "q1", " /compact now ")))
      .toBe(true);
    expect(h.queue()).toEqual([queued("q1", "run checks")]);
  });
});

describe("OutboundLane storage writes (renderer-app-15)", () => {
  it("writes each accepted queue change once and never writes back what it read", () => {
    const h = laneHarness({ initial: { [A]: [queued("a1", "restored")] } });
    h.status.set(A, "working");
    h.render();
    expect(h.stored).toEqual([]);
    h.lane.enqueue(A, "next", []);
    h.render();
    h.render();
    expect(h.stored).toHaveLength(1);
    h.lane.drop(A);
    h.render();
    expect(h.stored).toHaveLength(2);
    expect(h.storedQueues()).toEqual({});
  });
});

describe("durable recovery envelope", () => {
  it("passes original prompt and validated attachments separately from serialized provider text", async () => {
    const h = laneHarness();
    const sent = h.lane.sendWithContext(A, "Make the report", [fileContext], undefined, undefined, { id: "receipt", mode: "continue" });
    await flush();
    expect(h.sends[0].recovery).toEqual({ text: "Make the report", context: [fileContext], source: { id: "receipt", mode: "continue" } });
    expect(h.sends[0].prompt).not.toBe("Make the report");
    h.sends[0].resolve(undefined); await sent;
  });
  it("never records a provider command as a replayable prompt", async () => {
    const h = laneHarness(); const sent = h.lane.runCommand(A, "/compact"); await flush();
    expect(h.sends[0].recovery).toBeUndefined(); h.sends[0].resolve(undefined); await sent;
  });
});


describe("in-flight attachment retention", () => {
  it("pins captured images before validation through access-mode restore and host settlement", async () => {
    const h = laneHarness();
    const validation = h.holdValidation();
    const access = deferred<void>();
    h.port.restoreAccessMode = () => access.promise;
    const sent = h.lane.submitPrompt(A, "use this image", [fileContext]);
    expect(h.lane.snapshot().retainedContext).toContainEqual(fileContext);
    validation.resolve([fileContext]); await flush();
    expect(h.sendCount()).toBe(0);
    expect(h.lane.snapshot().retainedContext).toContainEqual(fileContext);
    access.resolve(); await flush();
    expect(h.sendCount()).toBe(1);
    expect(h.lane.snapshot().retainedContext).toContainEqual(fileContext);
    h.sends[0].resolve(undefined); await sent;
    expect(h.lane.snapshot().retainedContext).toEqual([]);
  });

  it("keeps context pinned through failed-send restoration and does not release another capture", async () => {
    const h = laneHarness();
    let pinnedAtRestore = false;
    h.port.restoreContext = () => { pinnedAtRestore = h.lane.snapshot().retainedContext.some(item => item.id === fileContext.id); };
    const releaseLaunch = h.lane.captureContext([urlContext]);
    const sent = h.lane.submitPrompt(A, "original", [fileContext]);
    await flush();
    h.sends[0].reject(unsent("No bytes sent"));
    await expect(sent).rejects.toThrow("No bytes sent");
    expect(pinnedAtRestore).toBe(true);
    expect(h.lane.snapshot().retainedContext).toEqual([urlContext]);
    releaseLaunch(); releaseLaunch();
    expect(h.lane.snapshot().retainedContext).toEqual([]);
  });

  it("retains a cancelled attempt until its asynchronous work has actually settled", async () => {
    const h = laneHarness();
    const validation = h.holdValidation();
    const sent = h.lane.sendWithContext(A, "recover", [fileContext], undefined, undefined, { id: "saved", mode: "retry" });
    h.lane.drop(A);
    expect(h.lane.snapshot().retainedContext).toContainEqual(fileContext);
    validation.resolve([fileContext]);
    await expect(sent).rejects.toThrow("cancelled");
    expect(h.sendCount()).toBe(0);
    expect(h.lane.snapshot().retainedContext).toEqual([]);
  });
});
