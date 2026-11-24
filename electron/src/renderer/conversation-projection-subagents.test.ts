import { expect, it, vi } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { conversationItems } from "./conversation-reducer";
import { createConversationProjection } from "./conversation-projection";

// Counts reference-reducer runs so the test can tell the incremental path
// from the full one; the wrapped reducer itself is unchanged.
const reference = vi.hoisted(() => ({ calls: 0 }));
vi.mock("./conversation-reducer", async importOriginal => {
  const actual = await importOriginal<typeof import("./conversation-reducer")>();
  return {
    ...actual,
    conversationItems: (events: EngineEvent[]) => {
      reference.calls++;
      return actual.conversationItems(events);
    },
  };
});

it("keeps main-thread and running-subagent streaming incremental after a subagent has spawned", () => {
  const project = createConversationProjection();
  let events: EngineEvent[] = [
    { kind: "user.message", text: "question" },
    { kind: "message.delta", text: "Let me delegate." },
    { kind: "tool.call", callId: "spawn", tool: "Task", summary: "child", input: {} },
    { kind: "subagent.started", agentKey: "spawn", subagentType: "reviewer", description: "review" },
    { kind: "message.delta", agentKey: "spawn", text: "child working" },
  ];
  expect(project(events)).toEqual(conversationItems(events));
  const step = (next: EngineEvent[], incremental: boolean): void => {
    const calls = reference.calls;
    const projected = project(next);
    expect(reference.calls - calls).toBe(incremental ? 0 : 1);
    expect(projected).toEqual(conversationItems(next));
    events = next;
  };
  step([...events, { kind: "message.delta", text: "Main" }], true);
  // live-events merges a delta into the log's last delta of the same agentKey.
  step([...events.slice(0, -1), { kind: "message.delta", text: "Main thread" }], true);
  step([...events, { kind: "commands.available", commands: [] },
    { kind: "message.delta", text: " continues" }], true);
  // A running card's own text streams into the card in place.
  step([...events, { kind: "message.delta", agentKey: "spawn", text: " more" }], true);
  // A card's delta did not close MAIN's open run; the next main delta extends it.
  step([...events, { kind: "message.delta", text: " still main" }], true);
  step([...events, { kind: "message.delta", agentKey: "spawn", text: " tail" }], true);
  step([...events.slice(0, -1), { kind: "message.delta", agentKey: "spawn", text: " tail grows" }], true);
  step([...events, { kind: "subagent.finished", agentKey: "spawn", ok: true, summary: "done" }], false);
  // A finished card's late text is the reducer's to place.
  step([...events, { kind: "message.delta", agentKey: "spawn", text: " late" }], false);
  step([...events, { kind: "message.delta", text: "Summary" }], true);
});

it("streams parallel and nested subagents in place, and leaves every structural change to the reducer", () => {
  const project = createConversationProjection();
  const spawn = (callId: string, agentKey?: string): EngineEvent[] => [
    { kind: "tool.call", callId, tool: "Task", summary: `spawn ${callId}`, input: {}, agentKey },
    { kind: "subagent.started", agentKey: callId, subagentType: "worker", description: callId },
  ];
  let events: EngineEvent[] = [
    { kind: "user.message", text: "fan out" },
    ...spawn("a"), ...spawn("b"), ...spawn("c", "a"),
    { kind: "message.delta", agentKey: "a", text: "   a starts  " },
  ];
  expect(project(events)).toEqual(conversationItems(events));
  const step = (added: EngineEvent[], incremental: boolean, replaceLast = false): void => {
    const next = [...(replaceLast ? events.slice(0, -1) : events), ...added];
    const calls = reference.calls;
    const old = project(events);
    const projected = project(next);
    expect(reference.calls - calls).toBe(incremental ? 0 : 1);
    expect(projected).toEqual(conversationItems(next));
    // The previous frame's items are never mutated.
    expect(old).toEqual(conversationItems(events));
    events = next;
  };
  // Two workers interleave; live-events cannot merge across them.
  step([{ kind: "message.delta", agentKey: "b", text: "b says" }], true);
  step([{ kind: "message.delta", agentKey: "a", text: "and a" }], true);
  step([{ kind: "message.delta", agentKey: "a", text: "and a again" }], true, true);
  // A worker nested in a worker streams inside both cards.
  step([{ kind: "message.delta", agentKey: "c", text: "nested ".repeat(30) }], true);
  step([{ kind: "message.delta", agentKey: "c", text: "nested, longer ".repeat(30) }], true, true);
  // A tool event reshapes a card: the reducer, then text streams again after it.
  step([{ kind: "tool.call", callId: "t1", tool: "Bash", summary: "npm test", input: {}, agentKey: "b" }], false);
  step([{ kind: "message.delta", agentKey: "b", text: "tests pass" }], true);
  // An agentKey nobody announced, and a card that finished, take the reducer.
  step([{ kind: "message.delta", agentKey: "stranger", text: "who?" }], false);
  step([{ kind: "subagent.finished", agentKey: "c", ok: false, summary: "gave up" }], false);
  step([{ kind: "message.delta", agentKey: "c", text: "after the end" }], false);
  step([{ kind: "message.delta", agentKey: "a", text: "a still going" }], true);
  step([{ kind: "message.delta", text: "main again" }, { kind: "message.delta", agentKey: "b", text: " more" }], true);
});
