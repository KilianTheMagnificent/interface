import { describe, expect, it, vi } from "vitest";
import type { CollaborationStatusDto } from "../shared/ipc";
import {
  CollaborationController, mergeCollaborationStatuses, type CollaborationApi,
} from "./collaboration-controller";
function status(revision: number): CollaborationStatusDto {
  return { sourceSessionKey: "claude#source", receipt: { version: 1,
    collaborationId: "one", revision, state: "awaiting_approval", mode: "consult",
    sourceEngine: "claude", targetEngine: "codex", approvalRequired: true,
    cancellationSafe: true, createdAt: "2026-09-24T00:00:00.000Z", updatedAt: "2026-09-24T00:00:00.000Z" } };
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function fixture() {
  let push!: Parameters<CollaborationApi["onCollaborationEvent"]>[0];
  const off = vi.fn();
  const api = { listCollaborations: vi.fn<CollaborationApi["listCollaborations"]>(),
    onCollaborationEvent: (listener: typeof push) => { push = listener; return off; },
    submitCollaboration: vi.fn(), approveCollaboration: vi.fn(), denyCollaboration: vi.fn(),
    acceptCollaborationPeer: vi.fn(), discardCollaborationPeer: vi.fn(), cancelCollaboration: vi.fn(),
  } satisfies CollaborationApi;
  const controller = new CollaborationController(api, String);
  const disconnect = controller.connect();
  return { api, controller, disconnect, off,
    push: (revision: number) => push({ version: 1, kind: "changed", status: status(revision) }) };
}
describe("collaboration receipt controller", () => {
  it("keeps a newer push when an older list completes, and ignores superseded list errors", async () => {
    const { api, controller, push, disconnect } = fixture();
    const first = deferred<CollaborationStatusDto[]>(), second = deferred<CollaborationStatusDto[]>();
    api.listCollaborations.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const one = controller.loadCollaborations(), two = controller.loadCollaborations();
    push(8);
    second.resolve([status(3)]); await two;
    first.reject(new Error("obsolete")); await one;
    expect(controller.snapshot()).toMatchObject({ collaborations: [status(8)],
      collaborationsLoading: false, collaborationsError: null });
    disconnect();
  });
  it("relists quietly on focus: an equal answer publishes nothing, a first one still settles", async () => {
    const { api, controller, disconnect } = fixture();
    let renders = 0;
    const stop = controller.subscribe(() => { renders++; });
    api.listCollaborations.mockResolvedValue([status(3)]);
    await controller.loadCollaborations({ quiet: true });
    expect(controller.snapshot()).toMatchObject({ collaborations: [status(3)], collaborationsLoading: false });
    const settled = renders;
    await controller.loadCollaborations({ quiet: true });
    expect(renders).toBe(settled);
    // An ordinary relist still shows its loading state.
    await controller.loadCollaborations();
    expect(renders).toBe(settled + 2);
    stop();
    disconnect();
  });
  it("invalidates a pending read on disposal, including a later reconnect", async () => {
    const { api, controller, disconnect, off } = fixture();
    const pending = deferred<CollaborationStatusDto[]>();
    api.listCollaborations.mockReturnValue(pending.promise);
    const read = controller.loadCollaborations();
    disconnect();
    const reconnect = controller.connect();
    const before = controller.snapshot();
    pending.resolve([status(10)]); await read;
    expect(controller.snapshot()).toBe(before);
    expect(off).toHaveBeenCalledOnce();
    reconnect();
  });
  it("applies mutation receipts by revision and propagates failure without retry", async () => {
    const { api, controller, push, disconnect } = fixture();
    push(8);
    api.cancelCollaboration.mockResolvedValueOnce(status(4)).mockRejectedValueOnce(new Error("unknown"));
    await controller.cancelCollaboration("one");
    expect(controller.snapshot().collaborations).toEqual([status(8)]);
    await expect(controller.cancelCollaboration("one")).rejects.toThrow("unknown");
    expect(api.cancelCollaboration).toHaveBeenCalledTimes(2);
    disconnect();
  });

  it("publishes only what changed, so an equal relist on focus renders nothing new", async () => {
    const { api, controller, push, disconnect } = fixture();
    api.listCollaborations.mockResolvedValue([status(8)]);
    await controller.loadCollaborations();
    let published = 0;
    const unsubscribe = controller.subscribe(() => { published++; });
    const held = controller.snapshot().collaborations;
    await controller.loadCollaborations();
    // Loading turns on and off (the panel says "Refreshing…"); the list stays.
    expect(published).toBe(2);
    expect(controller.snapshot().collaborations).toBe(held);
    expect(controller.snapshot().collaborationsLoading).toBe(false);
    // An equal push, or an older one, publishes nothing at all.
    const before = controller.snapshot();
    push(8);
    push(3);
    expect(controller.snapshot()).toBe(before);
    expect(published).toBe(2);
    push(9);
    expect(controller.snapshot().collaborations).toEqual([status(9)]);
    expect(published).toBe(3);
    unsubscribe();
    disconnect();
  });
});

describe("mergeCollaborationStatuses", () => {
  const other = (revision: number, updatedAt: string): CollaborationStatusDto => {
    const base = status(revision);
    return { ...base, receipt: { ...base.receipt, collaborationId: "two", updatedAt } };
  };

  it("returns the held list when nothing changed and new objects only for changes", () => {
    const held = mergeCollaborationStatuses([], [status(2), other(1, "2026-09-25T00:00:00.000Z")]);
    expect(held.map(entry => entry.receipt.collaborationId)).toEqual(["two", "one"]);
    expect(mergeCollaborationStatuses(held, [status(2), other(1, "2026-09-25T00:00:00.000Z")]))
      .toBe(held);
    expect(mergeCollaborationStatuses(held, [status(1)])).toBe(held);
    const changed = mergeCollaborationStatuses(held, [status(3)]);
    expect(changed).not.toBe(held);
    expect(changed[0]).toBe(held[0]);
    expect(changed[1]).toEqual(status(3));
    // An equal receipt at the same revision keeps the object already held.
    expect(mergeCollaborationStatuses(held, [{ ...status(2) }])[1]).toBe(held[1]);
  });
});
