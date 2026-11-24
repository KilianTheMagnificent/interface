import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import type { ConvItem } from "./conversation-reducer";
import { subagentParent, subagentPrompt } from "./subagent-detail";

const card = (id: string, children: ConvItem[] = []): Extract<ConvItem, { kind: "subagent" }> => ({
  kind: "subagent", id, subagentType: `type-${id}`, description: id, status: "running",
  children, toolCount: 0, subagentCount: 0,
});

describe("subagent detail", () => {
  it("reads the spawn's full prompt back off the log by callId", () => {
    const events: EngineEvent[] = [
      { kind: "tool.call", callId: "other", tool: "Task", summary: "", input: { prompt: "not this" } },
      { kind: "tool.call", callId: "spawn", tool: "Task", summary: "", input: { prompt: "  " } },
      { kind: "tool.call", callId: "spawn", tool: "Task", summary: "", input: { prompt: "Review the parser" } },
    ];
    expect(subagentPrompt(events, "spawn")).toBe("Review the parser");
    expect(subagentPrompt(events, "missing")).toBeUndefined();
  });

  it("finds the agent that spawned one at any depth", () => {
    const inner = card("c");
    const middle = card("b", [inner]);
    const items: ConvItem[] = [card("a", [middle])];
    expect(subagentParent(items, "c")).toBe(middle);
    expect(subagentParent(items, "b")?.id).toBe("a");
    expect(subagentParent(items, "a")).toBeUndefined();
  });
});
