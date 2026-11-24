import type { TerminalSummary, TerminalWorkspaceActivityRequest } from "../shared/ipc";

type Evidence = Pick<TerminalWorkspaceActivityRequest, "screen" | "outputCursor">;
interface Lane {
  desired: TerminalWorkspaceActivityRequest;
  running: boolean;
  applied?: TerminalWorkspaceActivityRequest;
  retry?: ReturnType<typeof setTimeout>;
}

/** Ordered, coalesced window facts. The host owns idleness and process effects. */
export class TerminalWorkspaceActivityQueue {
  private readonly lanes = new Map<string, Lane>();
  private disposed = false;
  constructor(
    private readonly send: (request: TerminalWorkspaceActivityRequest) => Promise<unknown>,
    private readonly report: (error: unknown) => void,
  ) {}

  reconcile(terminals: readonly TerminalSummary[], active: ReadonlySet<string>, evidence: (key: string) => Evidence): void {
    const live = new Set(terminals.filter(t => t.state === "running").map(t => t.id));
    for (const [id, lane] of this.lanes) {
      if (!live.has(id)) {
        clearTimeout(lane.retry);
        this.lanes.delete(id);
      }
    }
    for (const terminal of terminals) {
      if (!live.has(terminal.id)) continue;
      const isActive = active.has(terminal.id);
      const previous = this.lanes.get(terminal.id);
      if (previous?.desired.generation === terminal.generation && previous.desired.active === isActive) continue;
      const desired: TerminalWorkspaceActivityRequest = {
        version: 1, id: terminal.id, generation: terminal.generation, active: isActive,
        ...(!isActive ? evidence(`${terminal.id}:${terminal.generation}`) : {}),
      };
      const lane = previous ?? { desired, running: false };
      lane.desired = desired;
      clearTimeout(lane.retry);
      this.lanes.set(terminal.id, lane);
      void this.flush(lane);
    }
  }

  dispose(): void {
    this.disposed = true;
    for (const lane of this.lanes.values()) clearTimeout(lane.retry);
    this.lanes.clear();
  }

  private async flush(lane: Lane): Promise<void> {
    if (this.disposed || lane.running) return;
    lane.running = true;
    try {
      while (!this.disposed && this.lanes.get(lane.desired.id) === lane && lane.applied !== lane.desired) {
        const submitted = lane.desired;
        try {
          await this.send(submitted);
          lane.applied = submitted;
        } catch (error) {
          if (lane.desired !== submitted) continue;
          this.report(error);
          // Idempotent state assignment only; never retry terminal input.
          lane.retry = setTimeout(() => { void this.flush(lane); }, 5_000);
          break;
        }
      }
    } finally { lane.running = false; }
  }
}
