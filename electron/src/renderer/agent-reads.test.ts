import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyAgentControlResponse, type AgentRequest } from "../shared/agents";
import type { AgentWorkView } from "../shared/agent-work";
import { AGENT_WORK_POLL_MS, agentRequestIsRead, keepAgentWork, startAgentWorkReads } from "./agent-reads";
import type { AgentRequestResult } from "./components/AgentWorkspacePanel";

class Visibility extends EventTarget {
  hidden = false;
  change(hidden: boolean): void {
    this.hidden = hidden;
    this.dispatchEvent(new Event("visibilitychange"));
  }
}
const flush = async (): Promise<void> => { await vi.advanceTimersByTimeAsync(0); };
const work = (revision: number): AgentWorkView => ({ revision } as unknown as AgentWorkView);
const answer = (revision: number): AgentRequestResult =>
  ({ ok: true, response: { ...emptyAgentControlResponse(), work: work(revision) } });

describe("agent requests that only read", () => {
  it("never mark the agent page busy, while every change and anything unlisted does", () => {
    const reads: AgentRequest[] = [
      { kind: "work", agentId: "a", command: { action: "read" } },
      { kind: "calendar", agentId: "a", command: { action: "read", routineId: "r" } },
      { kind: "calendar", agentId: "a", command: { action: "preview", afterMs: 0,
        schedule: { timezone: "UTC", weekdays: [1], hour: 9, minute: 0, missed: "skip", pauseOnFailure: false } } },
      { kind: "markRead", agentId: "a" },
      { kind: "readAgentFiles", agentId: "a" },
      { kind: "readAgentDecisions", agentId: "a", limit: 50 },
    ];
    const changes: AgentRequest[] = [
      { kind: "work", agentId: "a", command: { action: "stop", taskId: "t" } },
      { kind: "work", agentId: "a", command: { action: "restoreMemory", sha256: "s", expectedSha256: "e" } },
      { kind: "calendar", agentId: "a", command: { action: "save", expectedRevision: 1, routineId: "r", schedule: null } },
      { kind: "startAgentThread", agentId: "a", text: "hi", requestId: "q" },
      { kind: "saveAgentMemory", agentId: "a", expectedSha256: "e", text: "" },
      { kind: "overwriteAgentInstructionsFile", agentId: "a", expectedSha256: "e" },
      { kind: "recreateAgentHome", agentId: "a" },
      { kind: "revealAgentHome", agentId: "a" },
      { kind: "savePermissionMode", agentId: "a", mode: "auto" },
      { kind: "futureKind", agentId: "a" } as unknown as AgentRequest,
    ];
    expect(reads.filter(command => !agentRequestIsRead(command))).toEqual([]);
    expect(changes.filter(agentRequestIsRead)).toEqual([]);
  });
});

describe("agent work view identity", () => {
  it("keeps the view on screen while its revision is unchanged", () => {
    const shown = work(4);
    expect(keepAgentWork(shown, work(4))).toBe(shown);
    const newer = work(5);
    expect(keepAgentWork(shown, newer)).toBe(newer);
    expect(keepAgentWork(null, shown)).toBe(shown);
  });
});

describe("agent work reads", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("reads nothing while the agent page is not on screen", async () => {
    const onRequest = vi.fn(async () => answer(1));
    const stop = startAgentWorkReads({ visible: false, agentId: "a", onRequest, onWork: vi.fn(), onError: vi.fn() },
      new Visibility());
    await vi.advanceTimersByTimeAsync(10 * AGENT_WORK_POLL_MS);
    expect(onRequest).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
    stop();
  });

  it("reads at once, then after each answer, and again when the window returns", async () => {
    const visibility = new Visibility();
    const onRequest = vi.fn(async (_command: AgentRequest) => answer(1));
    const onWork = vi.fn();
    const stop = startAgentWorkReads({ visible: true, agentId: "a", onRequest, onWork, onError: vi.fn() }, visibility);
    await flush();
    expect(onRequest).toHaveBeenCalledExactlyOnceWith({ kind: "work", agentId: "a", command: { action: "read" } });
    expect(onWork).toHaveBeenCalledWith(work(1));
    await vi.advanceTimersByTimeAsync(AGENT_WORK_POLL_MS);
    expect(onRequest).toHaveBeenCalledTimes(2);
    visibility.change(true);
    await vi.advanceTimersByTimeAsync(10 * AGENT_WORK_POLL_MS);
    expect(onRequest).toHaveBeenCalledTimes(2);
    visibility.change(false); await flush();
    expect(onRequest).toHaveBeenCalledTimes(3);
    stop();
    await vi.advanceTimersByTimeAsync(10 * AGENT_WORK_POLL_MS);
    expect(onRequest).toHaveBeenCalledTimes(3);
  });

  it("shows a refusal and drops an answer that lands after the page left", async () => {
    let settle!: (result: AgentRequestResult) => void;
    const onRequest = vi.fn()
      .mockResolvedValueOnce({ ok: false, error: "Agent work storage is uncertain." })
      .mockReturnValueOnce(new Promise<AgentRequestResult>(done => { settle = done; }));
    const onWork = vi.fn();
    const onError = vi.fn();
    const stop = startAgentWorkReads({ visible: true, agentId: "a", onRequest, onWork, onError }, new Visibility());
    await flush();
    expect(onError).toHaveBeenCalledWith("Agent work storage is uncertain.");
    await vi.advanceTimersByTimeAsync(AGENT_WORK_POLL_MS);
    expect(onRequest).toHaveBeenCalledTimes(2);
    stop();
    settle(answer(2)); await flush();
    expect(onWork).not.toHaveBeenCalled();
  });
});
