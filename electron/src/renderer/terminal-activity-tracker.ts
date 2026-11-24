import type { TerminalLaunchProfileId } from "../shared/ipc";
import {
  createTerminalActivityMonitor, feedTerminalActivity, noteTerminalInput, noteTerminalQuiet,
  setTerminalActivityProfile, terminalActivityBeneathWaiting, terminalActivitySawWork,
  TERMINAL_ACTIVITY_QUIET_MS, type TerminalActivityMonitor, type TerminalLiveActivity,
} from "./terminal-activity";

type Entry = {
  monitor: TerminalActivityMonitor;
  quiet?: ReturnType<typeof setTimeout>;
  /** When the pane counts as quiet. Output only moves this; the pending
   *  timer re-arms itself for the remainder when it fires early, so a
   *  streaming pane does not clear and create a timer for every chunk. */
  quietAt?: number;
  /** Rust's answer (host protocol v44 `TerminalSummary.attention`). */
  hostWaiting: boolean;
  /** The activity last handed to `callbacks.activity`; a new stream reads idle. */
  published: TerminalLiveActivity;
};

/**
 * Owns per-generation parser state and quiet timers independently of pane
 * mounts. Working, idle and finished turns come from the PTY bytes the
 * renderer sees; whether a pane needs the user is Rust's answer alone, set
 * through `hostAttention` from each host summary.
 */
export class TerminalActivityTracker {
  private readonly entries = new Map<string, Entry>();
  constructor(private readonly callbacks: {
    profile: (streamKey: string) => TerminalLaunchProfileId | undefined;
    activity: (streamKey: string, activity: TerminalLiveActivity) => void;
    completed: (streamKey: string, count: number) => void;
  }) {}

  private monitorFor(key: string): Entry {
    let entry = this.entries.get(key);
    if (entry === undefined) {
      entry = { monitor: createTerminalActivityMonitor(), hostWaiting: false, published: "idle" };
      this.entries.set(key, entry);
    }
    if (setTerminalActivityProfile(entry.monitor, this.callbacks.profile(key)) !== null) {
      this.publish(key, entry);
    }
    return entry;
  }

  /** Hands the pane's activity on when it changed: Rust's waiting first. */
  private publish(key: string, entry: Entry): void {
    const activity = entry.hostWaiting ? "waiting" : terminalActivityBeneathWaiting(entry.monitor);
    if (activity === entry.published) return;
    entry.published = activity;
    this.callbacks.activity(key, activity);
  }

  output(key: string, data: Uint8Array): void {
    const entry = this.monitorFor(key);
    const before = entry.monitor.completedTurns;
    const next = feedTerminalActivity(entry.monitor, data);
    // Completion and activity are independent: start+stop can share one chunk.
    if (entry.monitor.completedTurns !== before) {
      this.callbacks.completed(key, entry.monitor.completedTurns);
    }
    if (terminalActivitySawWork(entry.monitor)) {
      entry.quietAt = Date.now() + TERMINAL_ACTIVITY_QUIET_MS;
      if (entry.quiet === undefined) this.armQuiet(key, entry, TERMINAL_ACTIVITY_QUIET_MS);
    }
    if (next !== null) this.publish(key, entry);
  }

  /**
   * Rust's "needs you" for one stream, from its host summary. A stream with
   * no entry yet that is not waiting needs none.
   */
  hostAttention(key: string, waiting: boolean): void {
    if (!waiting && !this.entries.has(key)) return;
    const entry = this.monitorFor(key);
    if (entry.hostWaiting === waiting) return;
    entry.hostWaiting = waiting;
    this.publish(key, entry);
  }

  private armQuiet(key: string, entry: Entry, delay: number): void {
    entry.quiet = setTimeout(() => {
      entry.quiet = undefined;
      const remaining = (entry.quietAt ?? 0) - Date.now();
      if (remaining > 0) {
        this.armQuiet(key, entry, remaining);
        return;
      }
      entry.quietAt = undefined;
      if (noteTerminalQuiet(entry.monitor) !== null) this.publish(key, entry);
    }, delay);
  }

  input(key: string): void {
    const entry = this.monitorFor(key);
    clearTimeout(entry.quiet);
    entry.quiet = undefined;
    entry.quietAt = undefined;
    if (noteTerminalInput(entry.monitor) !== null) this.publish(key, entry);
  }

  prune(matches: (key: string) => boolean): void {
    for (const [key, entry] of this.entries) {
      if (!matches(key)) continue;
      clearTimeout(entry.quiet);
      this.entries.delete(key);
    }
  }

  dispose(): void { this.prune(() => true); }
}
