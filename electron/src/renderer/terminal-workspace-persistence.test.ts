import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TerminalWorkspaceRecord } from "../shared/ipc";
import { TerminalWorkspacePersistenceQueue } from "./terminal-workspace-persistence";

function deferred(): {
  readonly promise: Promise<void>;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
} {
  let resolve = (): void => undefined;
  let reject = (_error: unknown): void => undefined;
  const promise = new Promise<void>((accept, refuse) => {
    resolve = accept;
    reject = refuse;
  });
  return { promise, resolve, reject };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function roster(count: number): TerminalWorkspaceRecord[] {
  return [{
    id: "workspace-1",
    name: "Seven agents",
    createdAt: "2026-08-30T12:00:00.000Z",
    updatedAt: `2026-08-30T12:00:0${count.toString()}.000Z`,
    terminals: Array.from({ length: count }, (_, index) => {
      const number = index + 1;
      return {
        id: `terminal-${number.toString()}`,
        title: `Agent ${number.toString()}`,
        cwd: "/work/interface",
        launch: { version: 1 as const, profileId: number % 2 === 0 ? "claude" : "codex" },
        launchSession: {
          version: 1 as const,
          id: `managed-pane-${number.toString()}`,
          resume: false,
        },
      };
    }),
  }];
}

afterEach(() => { vi.useRealTimers(); });

describe("TerminalWorkspacePersistenceQueue", () => {
  it("debounces unsent snapshots and writes only the latest roster", async () => {
    vi.useFakeTimers();
    const writes: TerminalWorkspaceRecord[][] = [];
    const queue = new TerminalWorkspacePersistenceQueue(
      400,
      async workspaces => { writes.push(workspaces); },
      error => { throw error; },
    );

    queue.schedule(roster(1), "soon");
    queue.schedule(roster(4), "soon");
    queue.schedule(roster(7), "soon");
    await vi.advanceTimersByTimeAsync(399);
    expect(writes).toEqual([]);

    await vi.advanceTimersByTimeAsync(1);
    expect(writes).toHaveLength(1);
    expect(writes[0][0].terminals).toHaveLength(7);
  });

  it("withholds newer IPC until a delayed older write settles, then durably writes all bindings",
    async () => {
      const first = deferred();
      const writes: TerminalWorkspaceRecord[][] = [];
      let durable: TerminalWorkspaceRecord[] = [];
      const queue = new TerminalWorkspacePersistenceQueue(
        400,
        async workspaces => {
          writes.push(workspaces);
          if (writes.length === 1) await first.promise;
          durable = workspaces;
        },
        error => { throw error; },
      );

      queue.schedule(roster(1), "now");
      for (let count = 2; count <= 7; count += 1) queue.schedule(roster(count), "now");

      // The old implementation started all seven full-snapshot writes here,
      // allowing the one-terminal request to complete last. This queue never
      // exposes the host to that out-of-order completion race.
      expect(writes.map(write => write[0].terminals.length)).toEqual([1]);
      first.resolve();
      await settle();

      expect(writes.map(write => write[0].terminals.length)).toEqual([1, 7]);
      expect(durable[0].terminals.map(terminal => terminal.id)).toEqual([
        "terminal-1", "terminal-2", "terminal-3", "terminal-4",
        "terminal-5", "terminal-6", "terminal-7",
      ]);
      expect(durable[0].terminals.map(terminal => terminal.launchSession?.id)).toEqual([
        "managed-pane-1", "managed-pane-2", "managed-pane-3", "managed-pane-4",
        "managed-pane-5", "managed-pane-6", "managed-pane-7",
      ]);
    });

  it("reports a failed write and still flushes the newest snapshot behind it", async () => {
    const first = deferred();
    const errors: unknown[] = [];
    const writes: TerminalWorkspaceRecord[][] = [];
    let durable: TerminalWorkspaceRecord[] = [];
    const queue = new TerminalWorkspacePersistenceQueue(
      400,
      async workspaces => {
        writes.push(workspaces);
        if (writes.length === 1) await first.promise;
        durable = workspaces;
      },
      error => { errors.push(error); },
    );

    queue.schedule(roster(1), "now");
    queue.schedule(roster(7), "now");
    const failure = new Error("host write failed");
    first.reject(failure);
    await settle();

    expect(errors).toEqual([failure]);
    expect(writes.map(write => write[0].terminals.length)).toEqual([1, 7]);
    expect(durable[0].terminals).toHaveLength(7);
  });
});

describe("TerminalWorkspacePersistenceQueue folder moves", () => {
  const move = { from: "/Users/me/Desktop/Projects/app", to: "/Users/me/Developer/app" };

  it("keeps a confirmed move on the newer snapshot that supersedes it and settles its caller",
    async () => {
      const first = deferred();
      const writes: { count: number; moves: readonly unknown[] }[] = [];
      const queue = new TerminalWorkspacePersistenceQueue(
        400,
        async (workspaces, movedFolders) => {
          writes.push({ count: workspaces[0].terminals.length, moves: movedFolders });
          if (writes.length === 1) await first.promise;
        },
        error => { throw error; },
      );

      queue.schedule(roster(1), "now");
      let settled = false;
      const moved = queue.saveMove(roster(2), [move]).then(() => { settled = true; });
      // A later ordinary save replaces the waiting snapshot; the move must ride
      // along, or the host would narrow away the moved panes' restart proofs.
      queue.schedule(roster(3), "soon");
      await settle();
      expect(settled).toBe(false);

      first.resolve();
      await moved;
      expect(writes).toEqual([
        { count: 1, moves: [] },
        { count: 3, moves: [move] },
      ]);
    });

  it("rejects the move's caller, not the general error sink, when its save fails", async () => {
    const errors: unknown[] = [];
    const failure = new Error("That folder no longer leads there.");
    const queue = new TerminalWorkspacePersistenceQueue(
      400,
      async (_workspaces, movedFolders) => {
        if (movedFolders.length > 0) throw failure;
      },
      error => { errors.push(error); },
    );

    await expect(queue.saveMove(roster(1), [move])).rejects.toBe(failure);
    expect(errors).toEqual([]);
  });
});

describe("IntegratedTerminalCenter workspace persistence wiring", () => {
  it("routes every host roster write through the single persistence queue", () => {
    const source = readFileSync(
      new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
    expect(source.match(/api\.setTerminalWorkspaces/g)).toHaveLength(1);
    expect(source).toContain("workspacePersistence.schedule(");
  });

  it("lets workspace deletion pass through the updater before persistence", () => {
    const source = readFileSync(
      new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
    const start = source.indexOf("const confirmDeleteWorkspace");
    const end = source.indexOf("const switcher", start);
    const deletion = source.slice(start, end);

    expect(deletion).toContain("const remaining = forgetWorkspace(current, workspaceId);");
    expect(deletion).toContain('updateWorkspaces(() => remaining, "now");');
    expect(deletion).not.toContain("workspacesRef.current = remaining;");
  });

  it("returns before lifecycle workspace filing when the revision gate rejects an event", () => {
    const source = readFileSync(
      new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
    const start = source.indexOf("const acceptLifecycle");
    const end = source.indexOf("const acceptData", start);
    const lifecycle = source.slice(start, end);
    const rejectedRevision = lifecycle.indexOf(
      "if (nextRegistry === currentRegistry) return;",
    );
    const rejectedPayload = lifecycle.indexOf("if (!application.payloadApplied) return;");
    const authoritativeFile = lifecycle.indexOf(
      'file(event.terminal, binding, workspaceId, "authoritative")',
    );

    expect(rejectedRevision).toBeGreaterThanOrEqual(0);
    expect(rejectedPayload).toBeGreaterThan(rejectedRevision);
    expect(authoritativeFile).toBeGreaterThan(rejectedPayload);
  });

  it("reconciles provider bindings from an accepted post-load registry snapshot", () => {
    const source = readFileSync(
      new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
    const start = source.indexOf("const acceptRegistrySnapshot");
    const end = source.indexOf("const requestRestart", start);
    const snapshot = source.slice(start, end);

    expect(snapshot).toContain("if (workspacesLoadedRef.current)");
    expect(snapshot).toContain("reconcileLiveTerminalWorkspaceBindings(");
    expect(snapshot).toContain('), "now");');
  });
});
