import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ThreadTurn } from "../shared/threads";
import { Conversation, type ConversationProps } from "./components/Conversation";
import type { ConvItem } from "./conversation-reducer";
import type { ConversationThread } from "./conversation-types";
import { threadPromptDigest, threadTurnEnds } from "./thread-turns";
import { turnChangeSummaries } from "./tool-activity";

// Counts the two whole-transcript passes only the Chat layout draws from.
vi.mock("./tool-activity", async importOriginal => {
  const actual = await importOriginal<typeof import("./tool-activity")>();
  return { ...actual, turnChangeSummaries: vi.fn(actual.turnChangeSummaries) };
});
vi.mock("./thread-turns", async importOriginal => {
  const actual = await importOriginal<typeof import("./thread-turns")>();
  return { ...actual, threadTurnEnds: vi.fn(actual.threadTurnEnds) };
});

/* ==========================================================================
   A THREADS PANE SKIPS THE CHAT LAYOUT'S PASSES. The per-turn "+N −M" chip
   (turnChangeSummaries) and the Chat Fork's turn ends (threadTurnEnds) are
   read only by the Chat branch of the transcript; a Threads pane draws its
   own changes card and Fork from the turn foot. Both passes ran over the
   whole transcript on every streamed frame of a Threads pane regardless.
   ========================================================================== */

const turn = (ordinal: number, prompt: string): ThreadTurn => ({
  ordinal, sentAt: ordinal, promptDigest: threadPromptDigest(prompt), hasStart: true,
  hasEnd: true, state: "done", filesChanged: 1, added: 3, removed: 1,
});
const ITEMS: ConvItem[] = [
  { kind: "user", id: "user-0", text: "Fix the flaky login test" },
  { kind: "tool", id: "c1", tool: "Edit", summary: "/w/a.ts", input: { file_path: "/w/a.ts" },
    result: { ok: true, summary: "edited" } },
  { kind: "diff", id: "d1", path: "/w/a.ts", callId: "c1", added: 3, removed: 1 },
  { kind: "text", id: "text-1", text: "Fixed it.", streaming: false },
];
const THREAD: ConversationThread = { turns: [turn(0, "Fix the flaky login test")], project: true,
  worktree: false, revertFilesBlocked: null };

function paint(over: Partial<ConversationProps>): string {
  return renderToStaticMarkup(createElement(Conversation, {
    items: ITEMS, thinking: false, live: true, sessionKey: "codex#t",
    config: { effort: null, models: [], canSetModel: true, effortLevels: [], showEffort: false,
      canSetEffort: true, pendingModel: false, pendingEffort: false, error: null },
    approvals: {}, activeApprovalIds: new Set<string>(),
    onSend: () => undefined, onStop: () => undefined,
    onSetModel: () => undefined, onSetEffort: () => undefined,
    onApprove: () => undefined, onDeny: () => undefined,
    thread: THREAD, onThreadTurnDiff: () => undefined,
    onThreadEditFrom: () => Promise.resolve(), onThreadForkAt: () => undefined,
    pathRoot: "/w",
    ...over,
  }));
}

describe("a Threads pane and the Chat layout's per-turn passes", () => {
  beforeEach(() => {
    vi.mocked(turnChangeSummaries).mockClear();
    vi.mocked(threadTurnEnds).mockClear();
  });

  it("does not work out the Chat chip or the Chat Fork's turn ends", () => {
    const markup = paint({ threadLayout: true });
    expect(markup).toContain("is-thread-layout");
    expect(markup).not.toContain("conv-turn-changes");
    expect(turnChangeSummaries).not.toHaveBeenCalled();
    expect(threadTurnEnds).not.toHaveBeenCalled();
  });

  it("still works out both, and draws the chip, in the Chat layout", () => {
    const markup = paint({});
    expect(markup).toContain("conv-turn-changes");
    expect(turnChangeSummaries).toHaveBeenCalled();
    expect(threadTurnEnds).toHaveBeenCalled();
  });
});
