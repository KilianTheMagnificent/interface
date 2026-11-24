import { describe, expect, it } from "vitest";
import type { SessionSummary } from "../shared/engine";
import type { ThreadsView, ThreadView } from "../shared/threads";
import {
  threadReferenceCapability, threadReferenceChoices, threadReferenceItem,
} from "./thread-reference";

function thread(key: string, root: string | null, at: number, over: Partial<ThreadView> = {}): ThreadView {
  return {
    key, section: "active", createdAt: at, woke: false, unread: false, autoSettle: true, turns: [],
    ...(root === null ? {} : { workspace: { mode: "local", projectRoot: root, owned: false } }),
    ...over,
  };
}

function session(key: string, title: string, kind: SessionSummary["kind"] = "code"): SessionSummary {
  const [engine, sessionId] = key.split("#") as [string, string];
  return {
    engine: engine as SessionSummary["engine"], sessionId, title, cwd: "/repo", status: "idle",
    cloud: false, lastActivity: "2026-11-15T00:00:00Z", firstSeen: "2026-11-15T00:00:00Z",
    archived: false, kind,
  };
}

const view: ThreadsView = {
  version: 1, revision: 1, now: 1000,
  settings: { autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true },
  threads: [
    thread("claude#sender", "/website", 100),
    thread("codex#older", "/website", 200),
    thread("claude#newer", "/website", 300, { lastRun: { startedAt: 900, endedAt: 950 } }),
    thread("claude#api", "/api", 400),
    thread("claude#chat", null, 500),
    thread("claude#worker", "/website", 600),
    thread("claude#agent", "/website", 700),
  ],
  hiddenKeys: ["claude#worker"],
};
const sessions = [
  session("claude#sender", "Lead"), session("codex#older", "Fix the header"),
  session("claude#newer", "  Add login  "), session("claude#api", "API"),
  session("claude#chat", "Chat"), session("claude#worker", "Worker"),
  session("claude#agent", "Agent", "agent"),
];

describe("thread references in the picker", () => {
  it("lists the project's other listed threads, newest activity first", () => {
    expect(threadReferenceChoices(view, sessions, "claude#sender")).toEqual([
      { key: "claude#newer", title: "Add login", provider: "Claude Code", lastActivityAt: 950 },
      { key: "codex#older", title: "Fix the header", provider: "Codex", lastActivityAt: 200 },
    ]);
    // Nothing for a pane that is not a project thread, or before Threads loads.
    expect(threadReferenceChoices(view, sessions, "claude#chat")).toEqual([]);
    expect(threadReferenceChoices(view, sessions, "claude#unknown")).toEqual([]);
    expect(threadReferenceChoices(null, sessions, "claude#sender")).toEqual([]);
  });

  it("is offered only in a project thread's composer, saying why otherwise", () => {
    expect(threadReferenceCapability(view, "claude#sender")).toEqual({ available: true });
    expect(threadReferenceCapability(view, "claude#chat").available).toBe(false);
    expect(threadReferenceCapability(view, "claude#chat").unavailableReason).toMatch(/project/u);
    expect(threadReferenceCapability(null, "claude#sender").available).toBe(false);
  });

  it("becomes a chat chip whose value is the thread's key", () => {
    const chip = threadReferenceItem({ key: "codex#older", title: `${"t".repeat(100)}`,
      provider: "Codex", lastActivityAt: 0 }, "chat-1");
    expect(chip).toMatchObject({ id: "chat-1", kind: "chat", value: "codex#older",
      detail: "Codex thread · codex#older" });
    expect([...chip.label]).toHaveLength(80);
    expect(chip.label.endsWith("…")).toBe(true);
    expect(chip.safeContent).toBeUndefined();
  });
});
