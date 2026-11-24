import type { EngineUsage } from "../shared/engine";

/** A finished turn's read waits at least this long, so several tiles idling in
 * the same second share one read. */
const USAGE_TURN_SETTLE_MS = 800;

/**
 * The floor under the reads finished turns ask for: one starts no sooner than
 * this long after the previous read began, whatever began that one. Several
 * chats, agents or voice sessions finishing turns would otherwise spawn the
 * usage CLIs once per turn to redraw a chip. The turn is still counted — its
 * read waits for the floor rather than being dropped — so the chips can lag a
 * finished turn by up to this long. Refresh is never floored.
 */
export const USAGE_TURN_FLOOR_MS = 60_000;

/** What a usage read reports back to its owner. */
export interface PlanUsageReadsEnvironment {
  readonly read: () => Promise<EngineUsage[]>;
  /** The window is hidden: nobody can see the chips a turn's read would redraw. */
  readonly hidden?: () => boolean;
  /** A read has begun. */
  readonly loading: () => void;
  /** A read answered; `readAt` is the instant it landed. */
  readonly landed: (rows: EngineUsage[], readAt: number) => void;
  readonly failed: (error: unknown) => void;
}

/**
 * When plan usage is read. One read at a time: a request while one is in
 * flight is queued behind it, never spawned on top. `refresh` reads at once;
 * `turnDone` owes a read that waits for USAGE_TURN_FLOOR_MS, and any read that
 * begins after the turn pays that debt.
 */
export class PlanUsageReads {
  private inFlight = false;
  private disposed = false;
  private generation = 0;
  private wanted = false;
  private lastStartedAt: number | null = null;
  private turnOwed = false;
  private turnTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly environment: PlanUsageReadsEnvironment) {}

  /** Read now: Refresh, opening an empty panel, the stale catch-up. */
  refresh(): void {
    if (this.disposed) return;
    if (this.inFlight) {
      this.wanted = true;
      return;
    }
    this.inFlight = true;
    this.wanted = false;
    this.lastStartedAt = Date.now();
    // Every turn that ended before this read began is in its answer.
    this.turnOwed = false;
    this.clearTurnTimer();
    const generation = this.generation;
    this.environment.loading();
    let pending: Promise<EngineUsage[]>;
    try { pending = this.environment.read(); }
    catch (error: unknown) { pending = Promise.reject(error); }
    pending
      // Stamped at the instant the answer landed.
      .then(rows => {
        if (generation === this.generation && !this.disposed) this.environment.landed(rows, Date.now());
      })
      .catch((err: unknown) => {
        if (generation === this.generation && !this.disposed) this.environment.failed(err);
      })
      .finally(() => {
        this.inFlight = false;
        if (this.wanted) this.refresh();
      });
  }

  /** A turn just finished and spent quota. A burst of these shares one read. */
  turnDone(): void {
    if (this.disposed) return;
    this.turnOwed = true;
    this.clearTurnTimer();
    const floor = this.lastStartedAt === null ? 0
      : this.lastStartedAt + USAGE_TURN_FLOOR_MS - Date.now();
    this.turnTimer = setTimeout(() => {
      this.turnTimer = null;
      // Hidden, the turn stays owed and `shown` reads it on the way back.
      if (this.turnOwed && this.environment.hidden?.() !== true) this.refresh();
    }, Math.max(USAGE_TURN_SETTLE_MS, floor));
  }

  /** The window is visible again: read a turn that ended while it was hidden. */
  shown(): void {
    if (this.turnOwed && this.turnTimer === null) this.refresh();
  }

  /** React may set up the same controller again after an effect cleanup. */
  activate(): void {
    this.disposed = false;
  }

  dispose(): void {
    this.disposed = true;
    this.generation += 1;
    this.wanted = false;
    this.turnOwed = false;
    this.clearTurnTimer();
  }

  private clearTurnTimer(): void {
    if (this.turnTimer !== null) clearTimeout(this.turnTimer);
    this.turnTimer = null;
  }
}

