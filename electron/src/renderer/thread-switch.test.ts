import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ThreadCommand, ThreadControlResponse, ThreadView } from "../shared/threads";
import { ThreadHandoffPrompt } from "./components/ThreadAwayParts";
import { ThreadPaneHead } from "./components/ThreadPaneHead";
import { carryKeyed, conversationThread, switchSeed, switchThreadTo } from "./thread-pane";
import { ALL_THREAD_PROJECTS, buildThreadInbox } from "./thread-inbox";
import type { EngineEvent, SessionSummary } from "../shared/engine";
import { headStretchPromptRows, matchThreadTurns, threadPromptDigest } from "./thread-turns";
import type { TranscriptRow } from "./transcript-merge";
import { replaceWorkspaceKey } from "./workspace";

// Switch provider inside a thread (T3 section 5, protocol 59): what the
// renderer decides around Rust's `switchProvider`.

const VIEW = { version: 1, revision: 1, now: 0, settings: {
  autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true,
}, threads: [] } as const;

function answered(response: Partial<ThreadControlResponse>) {
  const sent: ThreadCommand[] = [];
  const moved: string[] = [];
  const port = {
    request: (command: ThreadCommand) => {
      sent.push(command);
      return Promise.resolve({ view: VIEW, ...response } as ThreadControlResponse);
    },
    now: () => 5_000,
    carry: (from: string, to: string, launched: string | undefined, switchedAt: number) => {
      expect(switchedAt).toBe(5_000);
      moved.push(`carry ${from} -> ${to}${launched === undefined ? "" : ` with ${launched}`}`);
    },
    select: (key: string) => { moved.push(`select ${key}`); },
  };
  return { port, sent, moved };
}

function row(key: string, item: TranscriptRow extends infer R ? R extends { kind: "item"; item: infer I } ? I : never : never):
    TranscriptRow {
  return { kind: "item", key, origin: "history", item };
}

describe("Switch to (protocol 59)", () => {
  it("moves the pane, draft and attachments to the answered head, then shows it", async () => {
    const { port, sent, moved } = answered({ key: "codex#b", handoff: "sent" });
    const result = await switchThreadTo(port, "claude#a", "codex");
    expect(sent).toEqual([{ kind: "switchProvider", key: "claude#a", engine: "codex" }]);
    expect(moved).toEqual(["carry claude#a -> codex#b", "select codex#b"]);
    expect(result).toEqual({ key: "codex#b", notice: null });
    // A new ACP session started with its handoff: the pane shows it as launched.
    const acp = answered({ key: "acp:grok#c", handoff: "sent", prompt: "You are taking over…" });
    await switchThreadTo(acp.port, "claude#a", "acp:grok");
    expect(acp.moved[0]).toBe("carry claude#a -> acp:grok#c with You are taking over…");
  });

  it("says when the handoff may not have arrived, and never sends it again", async () => {
    const { port, sent } = answered({ key: "codex#b", handoff: "unknown" });
    const result = await switchThreadTo(port, "claude#a", "codex");
    expect(sent).toHaveLength(1);
    expect(result.notice).toBe(
      "The thread is on Codex now, but its handoff may not have arrived. Type to carry on.");
  });

  it("moves nothing when Rust names no new head", async () => {
    const { port, moved } = answered({});
    await expect(switchThreadTo(port, "claude#a", "codex")).rejects.toThrow("Nothing moved.");
    expect(moved).toEqual([]);
  });

  it("keeps the pane in its place, focused, and carries keyed state", () => {
    const state = { openKeys: ["x#1", "claude#a", "y#2"], focusedKey: "x#1", maximizedKey: null };
    expect(replaceWorkspaceKey(state, "claude#a", "codex#b"))
      .toEqual({ openKeys: ["x#1", "codex#b", "y#2"], focusedKey: "codex#b", maximizedKey: null });
    expect(replaceWorkspaceKey({ ...state, maximizedKey: "claude#a", focusedKey: "claude#a" },
      "claude#a", "codex#b").maximizedKey).toBe("codex#b");
    // Not open here: the head is shown as any selection is.
    expect(replaceWorkspaceKey(state, "z#9", "codex#b").openKeys).toEqual(["codex#b", "claude#a", "y#2"]);
    expect(carryKeyed({ "claude#a": [1], other: [2] }, "claude#a", "codex#b"))
      .toEqual({ "codex#b": [1], other: [2] });
    const same = { other: [2] };
    expect(carryKeyed(same, "claude#a", "codex#b")).toBe(same);
  });

  it("gives controls only to the current session's own turns", () => {
    const turn = (ordinal: number, digest: string) => ({
      ordinal, sentAt: ordinal, promptDigest: digest, hasStart: true, hasEnd: true,
      state: "done" as const,
    });
    const rows: TranscriptRow[] = [
      row("u0", { kind: "user", id: "u0", text: "run the tests" }),
      row("n1", { kind: "notice", id: "n1", text: "Switched to Codex" }),
      row("u1", { kind: "user", id: "u1", text: "You are taking over a thread from Claude in Interface; the owner switched it to Codex.\n" }),
      row("u2", { kind: "user", id: "u2", text: "run the tests" }),
    ];
    expect(headStretchPromptRows(rows).map(prompt => prompt.key)).toEqual(["u1", "u2"]);
    const thread = {
      key: "codex#b", section: "active", createdAt: 0, woke: false, unread: false, autoSettle: true,
      workspace: { mode: "local", projectRoot: "/p", owned: false },
      turns: [turn(0, "x")],
      chain: { segments: [] },
    } as unknown as ThreadView;
    expect(conversationThread(thread).switched).toBe(true);
    // The head's own "run the tests" is its turn 1; Claude's earlier one is not.
    const matched = matchThreadTurns(headStretchPromptRows(rows),
      [turn(1, threadPromptDigest("run the tests"))], false);
    expect([...matched.keys()]).toEqual(["u2"]);
  });

  it("draws the handoff folded and says the switch in the pane head", () => {
    const folded = renderToStaticMarkup(createElement(ThreadHandoffPrompt, {
      line: "Handed the thread to Codex", text: "You are taking over a thread…",
    }));
    expect(folded).toContain("Handed the thread to Codex");
    expect(folded).toContain("<details class=\"taway-instructions\"><summary>What was sent</summary>");
    const head = renderToStaticMarkup(createElement(ThreadPaneHead, { switchedFrom: "Claude Code" }));
    expect(head).toContain("Switched from <span class=\"thread-head-parent\">Claude Code</span>");
    expect(renderToStaticMarkup(createElement(ThreadPaneHead, {}))).toBe("");
  });

  it("lists a switched thread once, under its head, while its earlier session stays in the registry", () => {
    const summary = (key: string): SessionSummary => {
      const [engine, sessionId] = key.split("#") as [SessionSummary["engine"], string];
      return { engine, sessionId, title: "Fix the login", cwd: "/p", status: "idle", cloud: false,
        origin: "app", firstSeen: new Date(1_000).toISOString(), lastActivity: new Date(1_000).toISOString(),
        archived: false, kind: "code" };
    };
    const head = {
      key: "acp:grok#b", section: "active", createdAt: 0, woke: false, unread: false, autoSettle: true,
      turns: [], workspace: { mode: "local", projectRoot: "/p", owned: false },
      chain: { segments: [
        { key: "claude#a", engine: "claude", order: 0, firstAt: 0, lastAt: 1 },
        { key: "acp:grok#b", engine: "acp:grok", order: 1, firstAt: 2, lastAt: 3 },
      ] },
    } as unknown as ThreadView;
    const model = buildThreadInbox({
      sessions: [summary("claude#a"), summary("acp:grok#b")],
      view: { ...VIEW, threads: [head] }, scope: ALL_THREAD_PROJECTS, query: "", selectedKey: null,
      // Inside the grace a session Rust has not recorded yet would be listed.
      now: 1_500,
    });
    expect(model.sections.flatMap(section => section.rows.map(row => row.key))).toEqual(["acp:grok#b"]);
  });

  it("draws the thread from before the switch, then the switch, before the new head's live stream", () => {
    const at = (ms: number) => new Date(ms).toISOString();
    const events = [
      { at: at(1_000), kind: "user.message", text: "fix the login" },
      { at: at(2_000), kind: "message.delta", text: "Fixed." },
      { at: at(6_000), kind: "user.message", text: "You are taking over a thread from Claude in Interface; the owner switched it to Grok.\n" },
      { at: at(7_000), kind: "message.delta", text: "Where it stands." },
    ] as EngineEvent[];
    expect(switchSeed(events, 5_000, "acp:grok")).toEqual([
      events[0], events[1],
      { at: at(5_000), kind: "notice", message: "Switched to Grok", tone: "info" },
    ]);
    expect(switchSeed(events, 500, "claude")).toEqual([]);
    expect(switchSeed(events.slice(0, 2), 5_000, "claude").at(-1))
      .toMatchObject({ message: "Switched to Claude" });
  });
});
