import type { MovedTerminalFolder, TerminalWorkspaceRecord } from "../shared/ipc";

export type TerminalWorkspacePersistenceMode = "now" | "soon";

type TerminalWorkspaceWriter = (
  workspaces: TerminalWorkspaceRecord[],
  movedFolders: readonly MovedTerminalFolder[],
) => Promise<unknown>;

interface WriteWaiter {
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
}

interface PendingWorkspaceWrite {
  readonly revision: number;
  readonly workspaces: TerminalWorkspaceRecord[];
  /** Confirmed folder moves; a newer snapshot that replaces this one inherits them. */
  movedFolders: readonly MovedTerminalFolder[];
  waiters: readonly WriteWaiter[];
}

/**
 * Serializes full-roster workspace writes.
 *
 * A roster is a snapshot, not a patch: letting two host writes overlap allows
 * an older request to finish last and erase terminals recorded by the newer
 * one. While a write is in flight, this queue retains only the latest requested
 * snapshot and sends it as soon as the current request settles.
 */
export class TerminalWorkspacePersistenceQueue {
  private timer: ReturnType<typeof globalThis.setTimeout> | null = null;
  private pending: PendingWorkspaceWrite | null = null;
  private inFlight = false;
  private nextRevision = 0;

  constructor(
    private readonly delayMs: number,
    private readonly write: TerminalWorkspaceWriter,
    private readonly onError: (error: unknown) => void,
  ) {}

  schedule(
    workspaces: readonly TerminalWorkspaceRecord[],
    mode: TerminalWorkspacePersistenceMode,
  ): void {
    this.submit(workspaces, mode, [], []);
  }

  /**
   * Saves a roster that follows owner-confirmed folder moves, in order with
   * every other save. The host moves the panes' restart proofs in the same
   * write, so the moves must reach it with the first snapshot naming the new
   * folders; a newer snapshot queued before that write inherits them. Settles
   * when that write does, and its failure goes to the caller alone.
   */
  saveMove(
    workspaces: readonly TerminalWorkspaceRecord[],
    movedFolders: readonly MovedTerminalFolder[],
  ): Promise<void> {
    return new Promise((resolve, reject) => {
      this.submit(workspaces, "now", movedFolders, [{ resolve, reject }]);
    });
  }

  private submit(
    workspaces: readonly TerminalWorkspaceRecord[],
    mode: TerminalWorkspacePersistenceMode,
    movedFolders: readonly MovedTerminalFolder[],
    waiters: readonly WriteWaiter[],
  ): void {
    const request: PendingWorkspaceWrite = {
      revision: this.nextRevision + 1,
      workspaces: [...workspaces],
      movedFolders,
      waiters,
    };
    this.nextRevision = request.revision;

    if (this.timer !== null) {
      globalThis.clearTimeout(this.timer);
      this.timer = null;
    }

    if (mode === "now") {
      this.enqueue(request);
      return;
    }

    // An immediate write already waiting behind the active request keeps its
    // urgency, but the newer snapshot replaces its stale contents.
    if (this.pending !== null) {
      this.pending = inherit(request, this.pending);
      return;
    }

    this.timer = globalThis.setTimeout(() => {
      this.timer = null;
      this.enqueue(request);
    }, this.delayMs);
  }

  private enqueue(request: PendingWorkspaceWrite): void {
    if (this.pending === null) this.pending = request;
    else if (request.revision > this.pending.revision) this.pending = inherit(request, this.pending);
    else this.pending = inherit(this.pending, request);
    this.flush();
  }

  private flush(): void {
    if (this.inFlight || this.pending === null) return;
    const request = this.pending;
    this.pending = null;
    this.inFlight = true;

    let write: Promise<unknown>;
    try {
      write = this.write(request.workspaces, request.movedFolders);
    } catch (error) {
      write = Promise.reject(error);
    }
    void write.then(
      () => { for (const waiter of request.waiters) waiter.resolve(); },
      (error: unknown) => {
        if (request.waiters.length === 0) this.onError(error);
        for (const waiter of request.waiters) waiter.reject(error);
      },
    ).finally(() => {
      this.inFlight = false;
      this.flush();
    });
  }
}

/** `newer` replaces `older` as the snapshot to write, keeping its moves and callers. */
function inherit(newer: PendingWorkspaceWrite, older: PendingWorkspaceWrite): PendingWorkspaceWrite {
  if (older.movedFolders.length === 0 && older.waiters.length === 0) return newer;
  newer.movedFolders = [...older.movedFolders, ...newer.movedFolders];
  newer.waiters = [...older.waiters, ...newer.waiters];
  return newer;
}
