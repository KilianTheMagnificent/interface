import { describe, expect, it, vi } from "vitest";
import type { OrchestrationEventDto, OrchestrationSnapshotDto } from "../shared/ipc";
import { OrchestrationController, type OrchestrationApi } from "./orchestration-controller";

function snapshot(revision: number, over: Partial<OrchestrationSnapshotDto> = {}):
    OrchestrationSnapshotDto {
  return {
    version: 1, hostId: "host-1", runtimeEpoch: "epoch-1", revision,
    createdAt: "2026-09-27T00:00:00.000Z", updatedAt: "2026-09-27T00:00:00.000Z",
    agents: [], messages: [], roleClaims: [], worktreeLeases: [], orchestratorPolicies: [],
    ...over,
  } as OrchestrationSnapshotDto;
}

function harness() {
  const reads: Array<OrchestrationSnapshotDto | Error> = [];
  const calls: string[] = [];
  let push: ((event: OrchestrationEventDto) => void) | undefined;
  const next = async (): Promise<OrchestrationSnapshotDto> => {
    calls.push("read");
    const value = reads.shift() ?? snapshot(1);
    if (value instanceof Error) throw value;
    return value;
  };
  const intent = <T,>(name: string, result: T) => vi.fn(async () => {
    calls.push(name);
    return result;
  });
  const api = {
    getOrchestrationSnapshot: vi.fn(next),
    onOrchestrationEvent: vi.fn((cb: (event: OrchestrationEventDto) => void) => {
      push = cb;
      return () => { push = undefined; };
    }),
    createOrchestrationAgent: intent("createOrchestrationAgent", {}),
    stopOrchestrationAgent: intent("stopOrchestrationAgent", {}),
    setOrchestratorPolicy: intent("setOrchestratorPolicy", {}),
    clearOrchestratorPolicy: intent("clearOrchestratorPolicy", {}),
    bindOrchestrationParent: intent("bindOrchestrationParent", {}),
    registerOrchestrationSession: intent("registerOrchestrationSession", {}),
    submitCoordinatedWorker: intent("submitCoordinatedWorker", {}),
    integrateCoordinatedWorker: intent("integrateCoordinatedWorker",
      { taskId: "task-7", accepted: true }),
    abandonCoordinatedTask: intent("abandonCoordinatedTask", {}),
    ackCoordinatedObligation: intent("ackCoordinatedObligation", {}),
  } as unknown as OrchestrationApi;
  const controller = new OrchestrationController(api,
    error => error instanceof Error ? error.message : String(error));
  let published = 0;
  controller.subscribe(() => { published += 1; });
  return { api, controller, reads, calls, push: (event: OrchestrationEventDto) => push?.(event),
    subscribed: () => push !== undefined, published: () => published };
}

describe("OrchestrationController", () => {
  it("re-reads the snapshot after every intent the host answers", async () => {
    const h = harness();
    const input = {} as never;
    const intents: Array<[string, () => Promise<unknown>]> = [
      ["createOrchestrationAgent", () => h.controller.createAgent(input)],
      ["stopOrchestrationAgent", () => h.controller.stopAgent(input)],
      ["setOrchestratorPolicy", () => h.controller.setPolicy(input)],
      ["clearOrchestratorPolicy", () => h.controller.clearPolicy(input)],
      ["bindOrchestrationParent", () => h.controller.bindParent(input)],
      ["registerOrchestrationSession", () => h.controller.registerSession(input)],
      ["submitCoordinatedWorker", () => h.controller.submitWorker(input)],
      ["integrateCoordinatedWorker", () => h.controller.integrateWorker(input)],
      ["abandonCoordinatedTask", () => h.controller.abandonTask(input)],
      ["ackCoordinatedObligation", () => h.controller.ackObligation(input)],
    ];
    for (const [name, run] of intents) {
      h.calls.length = 0;
      await run();
      expect(h.calls, name).toEqual([name, "read"]);
    }
  });

  it("answers an integrate with its claim while the verify runs on", async () => {
    const h = harness();
    await expect(h.controller.integrateWorker({} as never))
      .resolves.toEqual({ taskId: "task-7", state: "verifying" });
  });

  it("does not re-read when the host refuses an intent", async () => {
    const h = harness();
    vi.mocked(h.api.stopOrchestrationAgent).mockRejectedValueOnce(new Error("no such agent"));
    await expect(h.controller.stopAgent({} as never)).rejects.toThrow("no such agent");
    expect(h.calls).toEqual([]);
  });

  it("accepts newer or equal revisions of the same run and keeps an equal one's identity", async () => {
    const h = harness();
    const first = snapshot(2);
    h.reads.push(first, snapshot(2), snapshot(1), snapshot(3));
    await h.controller.load();
    expect(h.controller.snapshot()).toMatchObject({ snapshot: first, reads: 1, error: null });
    await h.controller.load();
    // Equal: the held object stays, but the read still counts.
    expect(h.controller.snapshot().snapshot).toBe(first);
    expect(h.controller.snapshot().reads).toBe(2);
    await h.controller.load();
    // Older revision of the same host run: ignored, not counted.
    expect(h.controller.snapshot().snapshot).toBe(first);
    expect(h.controller.snapshot().reads).toBe(2);
    await h.controller.load();
    expect(h.controller.snapshot().snapshot?.revision).toBe(3);
    expect(h.controller.snapshot().reads).toBe(3);
  });

  it("accepts any revision from a new host run", async () => {
    const h = harness();
    h.reads.push(snapshot(9), snapshot(1, { runtimeEpoch: "epoch-2" }),
      snapshot(0, { hostId: "host-2", runtimeEpoch: "epoch-2" }));
    await h.controller.load();
    await h.controller.load();
    expect(h.controller.snapshot().snapshot?.runtimeEpoch).toBe("epoch-2");
    await h.controller.load();
    expect(h.controller.snapshot().snapshot?.hostId).toBe("host-2");
  });

  it("drops the held snapshot and shows the error when a read fails, clearing it on the next", async () => {
    const h = harness();
    h.reads.push(snapshot(1), new Error("orchestration owner unavailable"), snapshot(1));
    await h.controller.load();
    await h.controller.load();
    expect(h.controller.snapshot()).toEqual({
      snapshot: null, reads: 2, error: "orchestration owner unavailable" });
    await h.controller.load();
    expect(h.controller.snapshot().error).toBeNull();
    expect(h.controller.snapshot().snapshot?.revision).toBe(1);
  });

  it("lets only the newest of overlapping reads land", async () => {
    const h = harness();
    let releaseFirst!: (value: OrchestrationSnapshotDto) => void;
    vi.mocked(h.api.getOrchestrationSnapshot)
      .mockImplementationOnce(() => new Promise(resolve => { releaseFirst = resolve; }))
      .mockImplementationOnce(async () => snapshot(4));
    const first = h.controller.load();
    await h.controller.load();
    releaseFirst(snapshot(5));
    await first;
    expect(h.controller.snapshot().snapshot?.revision).toBe(4);
  });

  it("follows the host's change pushes once connected", async () => {
    const h = harness();
    const disconnect = h.controller.connect();
    expect(h.subscribed()).toBe(true);
    h.push({ version: 1, kind: "changed", hostId: "host-1", runtimeEpoch: "epoch-1", revision: 1 });
    await Promise.resolve();
    expect(h.calls).toEqual(["read"]);
    disconnect();
    expect(h.subscribed()).toBe(false);
  });

  it("publishes a reported error and nothing for a change that changes nothing", async () => {
    const h = harness();
    h.controller.reportError("The team could not start.");
    expect(h.controller.snapshot().error).toBe("The team could not start.");
    expect(h.published()).toBe(1);
    h.reads.push(snapshot(1), snapshot(0));
    await h.controller.load();
    const afterAccept = h.published();
    await h.controller.load();
    // An older read with the error already clear: no new state at all.
    expect(h.published()).toBe(afterAccept);
  });
});
