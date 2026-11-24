import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ThreadAway } from "../shared/away";
import type { ThreadTurn } from "../shared/threads";
import fixture from "../fixtures/threads-v45.json";
import { Conversation, type ConversationProps } from "./components/Conversation";
import type { ConversationThread } from "./conversation-types";
import type { ConvItem } from "./conversation-reducer";
import { awayReport, awayReviewerOf, awayReviewView, awayRoundLine, awayTurnCards } from "./away-thread";
import { conversationThread } from "./thread-pane";
import { threadPromptDigest } from "./thread-turns";
import type { ThreadsView } from "../shared/threads";

/* ==========================================================================
   AN AWAY LOOP'S THREAD (review B3, protocol 45): each work round is a turn
   of the thread, its prompt drawn as the round with the instructions folded;
   the fresh review of that round sits under the turn's changes with Rust's
   verdict; the loop's report (its status and Stop while it runs) ends the
   thread. Every verdict is Rust's.
   ========================================================================== */

const view = fixture.view as unknown as ThreadsView;
const awayThread = view.threads[0]!;
const away = awayThread.away as ThreadAway;

describe("an Away loop's thread", () => {
  it("joins each work round to the thread turn it ran as, with that round's review", () => {
    const cards = awayTurnCards(away);
    expect([...cards.keys()]).toEqual([0, 1]);
    expect(cards.get(0)?.review?.verdict).toBe("revise");
    expect(cards.get(1)?.review?.verdict).toBe("complete");
    expect(awayRoundLine(cards.get(1)!)).toBe("Work round 2 of 6");
    expect(awayTurnCards(undefined).size).toBe(0);
  });

  it("gives the round still working the thread's running turn, before Rust names it", () => {
    const working: ThreadAway = { ...away, state: "running", rounds: [...away.rounds.slice(0, 4),
      { iteration: 3, role: "work", state: "started" }] };
    const turns = [{ ordinal: 0, state: "done" as const }, { ordinal: 1, state: "done" as const },
      { ordinal: 2, state: "running" as const }];
    expect(awayTurnCards(working, turns).get(2)?.iteration).toBe(3);
    // No running turn yet: the round waits for its turn.
    expect([...awayTurnCards(working, turns.slice(0, 2)).keys()]).toEqual([0, 1]);
  });

  it("says each review's verdict in words, and where a review stands without one", () => {
    const cards = awayTurnCards(away);
    const revise = awayReviewView(cards.get(0)!.review!, awayReviewerOf(away));
    expect(revise).toMatchObject({ title: "Review · Codex", verdict: "Changes requested", tone: "revise",
      summary: "The retry hides the race; await the session instead.", sessionKey: "codex#0199f2a4-review" });
    expect(awayReviewView({ iteration: 3, role: "review", state: "started" }, "claude"))
      .toMatchObject({ title: "Review · Claude Code", verdict: "Reviewing…", tone: "pending" });
    expect(awayReviewView({ iteration: 3, role: "review", state: "blocked" }, "codex"))
      .toMatchObject({ verdict: "No verdict", tone: "blocked" });
  });

  it("reports a finished loop with its last verdict, and a running one with its deadline", () => {
    const report = awayReport(away)!;
    expect(report).toMatchObject({ live: false, title: "Away · Finished",
      detail: "No justified work remains." });
    expect(report.verdict).toEqual({ label: "Approved, done", summary: "No justified work remains.",
      checks: ["npm test: 212 passed"] });
    expect(report.meta).toMatch(/^2 rounds reviewed · Codex · gpt-5\.6-terra · started /u);
    const running = awayReport({ ...away, state: "running" })!;
    expect(running.live).toBe(true);
    expect(running.title).toBe("Away is working");
    expect(running.meta).toMatch(/ · ends /u);
    expect(awayReport(undefined)).toBeNull();
  });

  it("names the reviewer each review ran on: the other vendor when the loop chose one (protocol 47)", () => {
    const crossed: ThreadAway = { ...away, reviewer: "claude" };
    expect(awayReviewerOf(crossed)).toBe("claude");
    expect(awayReviewerOf({ provider: "codex" })).toBe("codex");
    expect(awayReviewView(awayTurnCards(crossed).get(0)!.review!, awayReviewerOf(crossed)).title)
      .toBe("Review · Claude Code");
    expect(awayReport(crossed)!.meta).toMatch(/· Codex · gpt-5\.6-terra, reviewed by Claude Code · /u);
    expect(awayReport(away)!.meta).not.toContain("reviewed by");
    const markup = paint({ turns: [turn(0, PROMPT_1), turn(1, PROMPT_2)], project: true, worktree: true,
      revertFilesBlocked: null, away: crossed });
    expect(markup).toContain('aria-label="Review · Claude Code: Changes requested"');
  });

  it("hands the pane the loop with the thread", () => {
    expect(conversationThread(awayThread).away).toBe(away);
  });
});

const PROMPT_1 = "You are executing a bounded Interface Away loop in /w. Iteration 1/6.";
const PROMPT_2 = "You are executing a bounded Interface Away loop in /w. Iteration 2/6.";
const ITEMS: ConvItem[] = [
  { kind: "user", id: "user-0", text: PROMPT_1 },
  { kind: "text", id: "text-1", text: "Fixed the race.", streaming: false },
  { kind: "user", id: "user-2", text: PROMPT_2 },
  { kind: "text", id: "text-3", text: "Awaited the session.", streaming: false },
];
const turn = (ordinal: number, prompt: string): ThreadTurn => ({
  ordinal, sentAt: 1_000 + ordinal, promptDigest: threadPromptDigest(prompt), hasStart: true,
  hasEnd: true, state: "done", filesChanged: 1, added: 3, removed: 1,
});

function paint(thread: ConversationThread, over: Partial<ConversationProps> = {}): string {
  return renderToStaticMarkup(createElement(Conversation, {
    items: ITEMS, thinking: false, live: true, sessionKey: "codex#away", threadLayout: true, pathRoot: "/w",
    config: { effort: null, models: [], canSetModel: true, effortLevels: [], showEffort: false,
      canSetEffort: true, pendingModel: false, pendingEffort: false, error: null },
    approvals: {}, activeApprovalIds: new Set<string>(),
    onSend: () => undefined, onStop: () => undefined,
    onSetModel: () => undefined, onSetEffort: () => undefined,
    onApprove: () => undefined, onDeny: () => undefined,
    thread, onThreadTurnDiff: () => undefined, onThreadForkAt: () => undefined,
    onThreadAway: () => undefined,
    ...over,
  })).replace(/<style[^>]*>[\s\S]*?<\/style>/gu, "");
}

const count = (markup: string, needle: string): number => markup.split(needle).length - 1;

describe("an Away thread's transcript", () => {
  const thread: ConversationThread = { turns: [turn(0, PROMPT_1), turn(1, PROMPT_2)], project: true,
    worktree: true, revertFilesBlocked: null, away };

  it("draws each work round's prompt as the round, its instructions folded", () => {
    const markup = paint(thread);
    expect(markup).toContain("Work round 1 of 6");
    expect(markup).toContain("Work round 2 of 6");
    expect(count(markup, '<details class="taway-instructions">')).toBe(2);
    expect(count(markup, 'class="conv-msg-user')).toBe(0);
  });

  it("puts each round's review under its turn, and the report at the end", () => {
    const markup = paint(thread);
    expect(count(markup, "taway-card taway-review")).toBe(2);
    expect(markup).toContain("Changes requested");
    expect(markup).toContain('aria-label="Review · Codex: Approved, done"');
    expect(count(markup, ">Open review</button>")).toBe(2);
    const report = markup.indexOf("taway-card taway-report");
    expect(report).toBeGreaterThan(markup.lastIndexOf("taway-card taway-review"));
    expect(markup).toContain("Away · Finished");
    expect(markup).toContain(">Review changes</button>");
    expect(markup).not.toContain(">Stop loop</button>");
  });

  it("offers Stop while the loop runs", () => {
    const markup = paint({ ...thread, away: { ...away, state: "running" } });
    expect(markup).toContain("Away is working");
    expect(markup).toContain(">Stop loop</button>");
    expect(markup).not.toContain(">Review changes</button>");
  });

  it("leaves every other thread as it was", () => {
    const markup = paint({ ...thread, away: undefined }, { onThreadAway: undefined });
    expect(markup).not.toMatch(/taway-/u);
    expect(markup).toContain("Iteration 1/6");
  });

  it("locks the composer and turn actions while the Away loop is running", () => {
    const runningMarkup = paint({ ...thread, away: { ...away, state: "running" } });
    expect(runningMarkup).toContain("Away loop is running — stop Away before sending.");
    expect(runningMarkup).toMatch(/class="thread-act thread-fork"[^>]*disabled=""/u);
  });
});
