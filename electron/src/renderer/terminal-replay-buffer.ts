import {
  advanceTerminalReplayBoundary, INITIAL_TERMINAL_REPLAY_BOUNDARY,
  type TerminalReplayBoundary, type TerminalReplayChunk,
} from "./terminal-output-state";
import { TerminalReplayModes } from "./terminal-replay-modes";

export const MAX_REPLAY_OUTPUT_BYTES = 1024 * 1024;
export const MAX_REPLAY_OUTPUT_CHUNKS = 4096;

/** A bounded replay tail. Eviction releases bytes without shifting the array. */
export class TerminalReplayBuffer {
  private retained: (TerminalReplayChunk | undefined)[] = [];
  private head = 0;
  bytes = 0;
  truncated = false;
  readonly modes = new TerminalReplayModes();
  initialModes = "";

  constructor(public endBoundary: TerminalReplayBoundary = INITIAL_TERMINAL_REPLAY_BOUNDARY) {}

  get length(): number { return this.retained.length - this.head; }

  /** Materialize only when a view remounts, never on the output hot path. */
  get chunks(): readonly TerminalReplayChunk[] {
    return this.retained.slice(this.head) as TerminalReplayChunk[];
  }

  /** Clearing screen history does not reset the running program's modes/parser. */
  clear(): void {
    this.retained = [];
    this.head = 0;
    this.bytes = 0;
    this.truncated = false;
    this.initialModes = this.modes.restoreSequence;
  }

  push(data: Uint8Array): void {
    if (data.byteLength === 0) return;
    // Transport frames are bounded, but keep this class bounded independently.
    const skipped = Math.max(0, data.byteLength - MAX_REPLAY_OUTPUT_BYTES);
    const boundary = advanceTerminalReplayBoundary(this.endBoundary, data.subarray(0, skipped), this.modes.observe);
    const tail = skipped === 0 ? data : data.slice(skipped);
    this.retained.push({ bytes: tail, boundary });
    this.endBoundary = advanceTerminalReplayBoundary(boundary, tail, this.modes.observe);
    this.bytes += tail.byteLength;
    this.truncated ||= skipped > 0;
    while (this.bytes > MAX_REPLAY_OUTPUT_BYTES || this.length > MAX_REPLAY_OUTPUT_CHUNKS) {
      this.bytes -= this.retained[this.head]!.bytes.byteLength;
      this.retained[this.head++] = undefined;
      this.truncated = true;
    }
    if (this.head >= MAX_REPLAY_OUTPUT_CHUNKS && this.head * 2 >= this.retained.length) {
      this.retained = this.retained.slice(this.head);
      this.head = 0;
    }
  }
}
