import { advanceTerminalReplayBoundary, type TerminalReplayBoundary } from "./terminal-output-state";

export interface TerminalRedrawPulseSize {
  readonly columns: number;
  readonly rows: number;
}

type TerminalRedrawPulseSender = (size: TerminalRedrawPulseSize) => Promise<void>;

interface TerminalRedrawPulse {
  targetSize: TerminalRedrawPulseSize;
  readonly send: TerminalRedrawPulseSender;
  readonly acknowledgement: "output" | "opentui-resize";
  resizeParameters: string | null;
  minimumDelayElapsed: boolean;
  outputObserved: boolean;
  fallbackElapsed: boolean;
  kickResolved: boolean;
  restoreInFlight: boolean;
  minimumTimer: ReturnType<typeof globalThis.setTimeout> | null;
  fallbackTimer: ReturnType<typeof globalThis.setTimeout> | null;
}

function sameSize(left: TerminalRedrawPulseSize, right: TerminalRedrawPulseSize): boolean {
  return left.columns === right.columns && left.rows === right.rows;
}

/**
 * Sends a temporary PTY geometry, then restores the latest fitted geometry
 * only after the TUI has had time to react to the first SIGWINCH. Output can
 * be an unrelated animation, so the minimum delay must also cover the TUI's
 * resize debounce. OpenTUI recovery waits for its CSI 14 t resize query;
 * animation frames do not prove its event loop has handled SIGWINCH.
 * A bounded fallback prevents a quiet or failed-to-redraw
 * process from leaving the PTY at the temporary size forever.
 */
export class TerminalRedrawPulseQueue {
  private readonly entries = new Map<string, TerminalRedrawPulse>();

  constructor(
    private readonly minimumDelayMs: number,
    private readonly outputFallbackMs: number,
    private readonly onError: (error: unknown) => void,
  ) {}

  start(
    key: string,
    kickSize: TerminalRedrawPulseSize,
    targetSize: TerminalRedrawPulseSize,
    send: TerminalRedrawPulseSender,
    acknowledgement: "output" | "opentui-resize" = "output",
  ): void {
    this.cancel(key);
    const entry: TerminalRedrawPulse = {
      targetSize,
      send,
      acknowledgement,
      resizeParameters: null,
      minimumDelayElapsed: false,
      outputObserved: false,
      fallbackElapsed: false,
      kickResolved: false,
      restoreInFlight: false,
      minimumTimer: null,
      fallbackTimer: null,
    };
    this.entries.set(key, entry);

    let kickRequest: Promise<void>;
    try {
      kickRequest = send(kickSize);
    } catch (error) {
      this.failKick(key, entry, error);
      return;
    }
    void kickRequest.then(() => {
      if (this.entries.get(key) !== entry) return;
      entry.kickResolved = true;
      this.armRestoreGate(key, entry);
    }, error => {
      this.failKick(key, entry, error);
    });
  }

  updateTarget(key: string, size: TerminalRedrawPulseSize): boolean {
    const entry = this.entries.get(key);
    if (entry === undefined) return false;
    entry.targetSize = size;
    return true;
  }

  noteOutput(key: string, data?: Uint8Array, boundary: TerminalReplayBoundary = "ground"): void {
    const entry = this.entries.get(key);
    if (entry === undefined) return;
    if (entry.acknowledgement === "output") entry.outputObserved = true;
    else if (data !== undefined && !entry.outputObserved) {
      // Use the full stream's parser boundary: a title/control-string payload
      // containing CSI-looking text must not acknowledge a resize. Parameters
      // survive split transport chunks and remain bounded to the exact query.
      advanceTerminalReplayBoundary(boundary, data, (state, byte) => {
        if (state === "escape") entry.resizeParameters = byte === 0x5b ? "" : null;
        else if (state === "csi-parameter" || state === "csi-intermediate") {
          if (byte === 0x74 && entry.resizeParameters === "14") entry.outputObserved = true;
          entry.resizeParameters = byte >= 0x30 && byte <= 0x39
            && entry.resizeParameters !== null && entry.resizeParameters.length < 2
            ? entry.resizeParameters + String.fromCharCode(byte) : null;
        }
      });
    }
    this.maybeRestore(key, entry);
  }

  cancel(key: string): void {
    const entry = this.entries.get(key);
    if (entry === undefined) return;
    this.clearTimers(entry);
    this.entries.delete(key);
  }

  dispose(): void {
    for (const entry of this.entries.values()) this.clearTimers(entry);
    this.entries.clear();
  }

  private maybeRestore(key: string, entry: TerminalRedrawPulse): void {
    if (this.entries.get(key) !== entry || entry.restoreInFlight) return;
    if (!entry.kickResolved || !entry.minimumDelayElapsed) return;
    if (!entry.outputObserved && !entry.fallbackElapsed) return;

    this.clearTimers(entry);
    entry.restoreInFlight = true;
    const sentSize = entry.targetSize;
    let restoreRequest: Promise<void>;
    try {
      restoreRequest = entry.send(sentSize);
    } catch (error) {
      this.onError(error);
      this.finishRestore(key, entry, sentSize);
      return;
    }
    void restoreRequest.then(() => {
      this.finishRestore(key, entry, sentSize);
    }, error => {
      this.onError(error);
      this.finishRestore(key, entry, sentSize);
    });
  }

  /** The spacing guarantee begins only after the kick has reached the host. */
  private armRestoreGate(key: string, entry: TerminalRedrawPulse): void {
    entry.minimumTimer = globalThis.setTimeout(() => {
      entry.minimumTimer = null;
      entry.minimumDelayElapsed = true;
      this.maybeRestore(key, entry);
    }, this.minimumDelayMs);
    // A busy OpenCode process can miss both geometries inside the ordinary
    // 500ms fallback. Give its resize report time to arrive, while still
    // restoring quiet/exited or incompatible versions without an endless wait.
    entry.fallbackTimer = globalThis.setTimeout(() => {
      entry.fallbackTimer = null;
      entry.fallbackElapsed = true;
      this.maybeRestore(key, entry);
    }, entry.acknowledgement === "opentui-resize"
      ? Math.max(this.outputFallbackMs, 3000) : this.outputFallbackMs);
    this.maybeRestore(key, entry);
  }

  private finishRestore(
    key: string,
    entry: TerminalRedrawPulse,
    sentSize: TerminalRedrawPulseSize,
  ): void {
    entry.restoreInFlight = false;
    if (this.entries.get(key) !== entry) return;
    if (sameSize(entry.targetSize, sentSize)) {
      this.entries.delete(key);
      return;
    }
    this.maybeRestore(key, entry);
  }

  private failKick(key: string, entry: TerminalRedrawPulse, error: unknown): void {
    this.onError(error);
    if (this.entries.get(key) !== entry) return;
    this.clearTimers(entry);
    this.entries.delete(key);
  }

  private clearTimers(entry: TerminalRedrawPulse): void {
    if (entry.minimumTimer !== null) {
      globalThis.clearTimeout(entry.minimumTimer);
      entry.minimumTimer = null;
    }
    if (entry.fallbackTimer !== null) {
      globalThis.clearTimeout(entry.fallbackTimer);
      entry.fallbackTimer = null;
    }
  }
}
