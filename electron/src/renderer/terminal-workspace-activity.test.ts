import { afterEach, expect, it, vi } from "vitest";
import type { TerminalSummary, TerminalWorkspaceActivityRequest } from "../shared/ipc";
import { TerminalWorkspaceActivityQueue } from "./terminal-workspace-activity";
const terminal = (id = "one", generation = 1): TerminalSummary => ({ id, generation, state: "running" } as TerminalSummary);
const tick = async (): Promise<void> => { await Promise.resolve(); await Promise.resolve(); };
afterEach(() => vi.useRealTimers());

it("coalesces rapid workspace switches and orders wake after an in-flight sleep", async () => {
  const sent: TerminalWorkspaceActivityRequest[] = [];
  let release!: () => void;
  const queue = new TerminalWorkspaceActivityQueue(async request => {
    sent.push(request);
    if (sent.length === 1) await new Promise<void>(resolve => { release = resolve; });
  }, error => { throw error; });
  queue.reconcile([terminal()], new Set(), () => ({ outputCursor: "42" }));
  queue.reconcile([terminal()], new Set(["one"]), () => ({}));
  queue.reconcile([terminal()], new Set(), () => ({}));
  queue.reconcile([terminal()], new Set(["one"]), () => ({}));
  expect(sent).toHaveLength(1);
  release(); await tick();
  expect(sent.map(request => request.active)).toEqual([false, true]);
  queue.reconcile([terminal()], new Set(["one"]), () => ({}));
  expect(sent).toHaveLength(2);
  queue.dispose();
});

it("sends a fresh generation and only active workspace membership", async () => {
  const send = vi.fn(async () => undefined);
  const queue = new TerminalWorkspaceActivityQueue(send, () => undefined);
  queue.reconcile([terminal(), terminal("two")], new Set(["two"]), () => ({}));
  await tick();
  queue.reconcile([terminal("one", 2), terminal("two")], new Set(["one"]), () => ({}));
  await tick();
  expect(send.mock.calls).toHaveLength(4);
  expect(send).toHaveBeenCalledWith(expect.objectContaining({ id: "one", generation: 2, active: true }));
  expect(send).toHaveBeenCalledWith(expect.objectContaining({ id: "two", active: false }));
  queue.dispose();
});

it("recovers a failed wake without replaying input and cancels retired retries", async () => {
  vi.useFakeTimers();
  const send = vi.fn().mockRejectedValueOnce(new Error("temporary wake failure")).mockResolvedValue({});
  const report = vi.fn();
  const queue = new TerminalWorkspaceActivityQueue(send, report);
  queue.reconcile([terminal()], new Set(["one"]), () => ({}));
  await tick();
  expect(report).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(5_000);
  expect(send).toHaveBeenCalledTimes(2);
  queue.reconcile([], new Set(), () => ({}));
  queue.dispose();
  await vi.runAllTimersAsync();
  expect(send).toHaveBeenCalledTimes(2);
});

it("shares the Rust v24 generation-fenced workspace activity fixture", async () => {
  const fixture = (await import("../fixtures/terminal-workspace-activity-v24.json")).default;
  const request: TerminalWorkspaceActivityRequest = { ...fixture, version: 1 };
  expect(request).toEqual(fixture);
  expect(request.active).toBe(false);
  expect(request.screen?.bracketedPaste).toBe(true);
});
