import type { TerminalControlScreenV1, TerminalDataEvent } from "../shared/ipc";
import {
  BRACKETED_PASTE_ON, pruneTerminalOutputBuffers, replayTerminalOutputChunks,
  terminalOutputDisposition, terminalOutputStreamKey, terminalReplayAssertsBracketedPaste,
  terminalReplayShouldRestoreChunks, type TerminalReplayBoundary,
} from "./terminal-output-state";
import { TerminalReplayBuffer } from "./terminal-replay-buffer";

export const OUTPUT_TRUNCATED_NOTICE = "\r\n\x1b[33m[Earlier background output was truncated.]\x1b[0m\r\n";

/** The part of a mounted emulator the router writes into and reads screens from. */
export interface TerminalStreamView {
  write(data: string | Uint8Array, callback?: () => void): void;
  /** Allow geometry observation only after any buffered PTY bytes are restored. */
  completeSetup(): void;
  controlScreen(): TerminalControlScreenV1 | null;
}

/** A stream's screen as the host is shown it, with the output byte count it reflects. */
export interface TerminalScreenEvidence {
  readonly screen?: TerminalControlScreenV1;
  readonly outputCursor?: string;
}

/** One routed chunk: its stream and the replay parser state before its first byte. */
export interface TerminalStreamOutput {
  readonly key: string;
  readonly boundary: TerminalReplayBoundary;
}

/**
 * Per-stream terminal output state, keyed by `terminalOutputStreamKey`: the
 * mounted emulator and the writes it has not acknowledged, the bounded replay
 * tail a remount restores (PTYs do not replay old bytes), the stream's byte
 * count (the cursor a control screen reflects), the last screen of a detached
 * view, and the terminal ids that have ended for good. Every stream-keyed fact
 * lives here so that `retire` ends all of them at once.
 */
export class TerminalStreamRouter<V extends TerminalStreamView> {
  private readonly views = new Map<string, V>();
  private readonly pendingWrites = new Map<string, Set<() => void>>();
  private readonly replay = new Map<string, TerminalReplayBuffer>();
  private readonly outputBytes = new Map<string, bigint>();
  private readonly sleepingScreens = new Map<string, Required<TerminalScreenEvidence>>();
  private readonly retired = new Set<string>();

  /**
   * Route one PTY data event. Output for a retired terminal or a superseded
   * generation is dropped; output for a generation the control plane has not
   * announced yet waits in its replay tail. `complete` releases the transport
   * credit: at once, unless the stream's view is writing the bytes, then when
   * xterm has (or when the view detaches). Returns null for dropped events.
   */
  deliver(event: TerminalDataEvent, known: { readonly generation: number } | undefined,
      complete: () => void): TerminalStreamOutput | null {
    if (event.kind !== "data" || event.data.byteLength === 0) { complete(); return null; }
    const disposition = terminalOutputDisposition(
      known?.generation,
      event.generation,
      this.retired.has(event.terminalId),
    );
    if (disposition === "discard") { complete(); return null; }
    const key = terminalOutputStreamKey(event.terminalId, event.generation);
    const view = disposition === "deliver" ? this.views.get(key) : undefined;
    this.outputBytes.set(key, (this.outputBytes.get(key) ?? 0n) + BigInt(event.data.byteLength));
    let tail = this.replay.get(key);
    const boundary = tail?.endBoundary ?? "ground";
    if (tail === undefined) {
      tail = new TerminalReplayBuffer();
      this.replay.set(key, tail);
    }
    tail.push(event.data);
    if (view === undefined) complete();
    else {
      const pending = this.pendingWrites.get(key) ?? new Set<() => void>();
      this.pendingWrites.set(key, pending);
      const done = (): void => {
        if (!pending.delete(done)) return;
        if (pending.size === 0) this.pendingWrites.delete(key);
        complete();
      };
      pending.add(done);
      try { view.write(event.data, done); }
      catch (error) { done(); throw error; }
    }
    return { key, boundary };
  }

  /**
   * A view's emulator mounted. The stream's retained tail goes into it first:
   * re-asserted bracketed paste for the CLIs that always enable it, observed
   * input modes, the truncation notice and the tail itself — or, for a torn
   * full-screen frame, only the modes, since the pane repaints on a SIGWINCH
   * instead. Once xterm has drawn all of it the view completes setup and
   * `ready` learns whether that repaint is due; a view replaced meanwhile
   * hears nothing.
   */
  attach(key: string, view: V, profileId: string | undefined,
      ready: (redraw: boolean) => void): void {
    this.views.set(key, view);
    const tail = this.replay.get(key);
    const completeSetup = (): void => {
      // A workspace can be switched again while xterm is still draining its
      // asynchronous write queue. A callback from that disposed view must
      // neither fit/focus its replacement nor publish a false readiness
      // barrier for automation starting the next transition.
      if (this.views.get(key) !== view) return;
      view.completeSetup();
      ready(tail?.truncated === true
        && !terminalReplayShouldRestoreChunks(true, profileId, tail.modes.alternateScreen));
    };
    if (tail === undefined) {
      completeSetup();
      return;
    }
    if (terminalReplayAssertsBracketedPaste(profileId)) view.write(BRACKETED_PASTE_ON);
    const restore = terminalReplayShouldRestoreChunks(
      tail.truncated, profileId, tail.modes.alternateScreen);
    if (!restore) {
      // A SIGWINCH repaint contains screen deltas, not the startup
      // DECSETs. Restore observed input/buffer modes before repainting.
      view.write(tail.modes.restoreSequence);
      view.write(OUTPUT_TRUNCATED_NOTICE, completeSetup);
    } else if (tail.length === 0) {
      if (tail.initialModes !== "") view.write(tail.initialModes);
      if (tail.truncated) view.write(OUTPUT_TRUNCATED_NOTICE, completeSetup);
      else completeSetup();
    } else {
      if (tail.initialModes !== "") view.write(tail.initialModes);
      if (tail.truncated) view.write(OUTPUT_TRUNCATED_NOTICE);
      const chunks = replayTerminalOutputChunks(tail.chunks);
      chunks.forEach((chunk, index) => {
        view.write(chunk, index === chunks.length - 1 ? completeSetup : undefined);
      });
      if (chunks.length === 0) completeSetup();
    }
  }

  /**
   * A view's emulator is being disposed. Its screen stays as the stream's
   * sleeping screen, and writes a disposed xterm may never finish are
   * acknowledged now. The replay tail stays for the next mount.
   */
  detach(key: string): void {
    const screen = this.liveScreen(key);
    if (screen !== null) {
      this.sleepingScreens.set(key, { screen, outputCursor: this.cursor(key).toString() });
    }
    for (const done of this.pendingWrites.get(key) ?? []) done();
    this.views.delete(key);
  }

  /** The mounted emulator for a stream, if any. */
  view(key: string): V | undefined {
    return this.views.get(key);
  }

  /** Every mounted emulator. */
  attachedViews(): IterableIterator<V> {
    return this.views.values();
  }

  /** Clearing a screen drops its replay tail but keeps the program's modes. */
  clear(key: string): void {
    this.replay.get(key)?.clear();
  }

  /** Whether the stream's program asked for the pixel size, as OpenTUI does. */
  queriedPixelSize(key: string): boolean {
    return this.replay.get(key)?.modes.queriedPixelSize === true;
  }

  /**
   * The live screen and the byte count it reflects, or null while xterm is
   * still writing or before the stream has produced any output.
   */
  controlScreen(key: string): { screen: TerminalControlScreenV1; cursor: string } | null {
    const screen = this.liveScreen(key);
    const cursor = this.outputBytes.get(key);
    return screen !== null && cursor !== undefined ? { screen, cursor: String(cursor) } : null;
  }

  /** The live screen when every write has landed, else the one kept at detach. */
  screenEvidence(key: string): TerminalScreenEvidence {
    const screen = this.liveScreen(key);
    if (screen !== null) return { screen, outputCursor: this.cursor(key).toString() };
    return this.sleepingScreens.get(key) ?? {};
  }

  /**
   * End streams for good: every generation of `terminalId` below
   * `minimumGeneration`, or all of them when it is omitted — the terminal is
   * gone, and later output for it is dropped. Naming a generation says the
   * terminal lives at it, so its output flows again. A mounted view keeps its
   * handle and unacknowledged writes until it detaches.
   */
  retire(terminalId: string, minimumGeneration?: number): void {
    if (minimumGeneration === undefined) this.retired.add(terminalId);
    else this.retired.delete(terminalId);
    pruneTerminalOutputBuffers(this.replay, terminalId, minimumGeneration);
    pruneTerminalOutputBuffers(this.outputBytes, terminalId, minimumGeneration);
    pruneTerminalOutputBuffers(this.sleepingScreens, terminalId, minimumGeneration);
  }

  /**
   * Follow the control plane's registry: each listed terminal lives at its
   * generation, so older ones retire, and a detached screen is kept only for
   * a stream still listed.
   */
  sync(terminals: readonly { readonly id: string; readonly generation: number }[]): void {
    const listed = new Set<string>();
    for (const terminal of terminals) {
      this.retire(terminal.id, terminal.generation);
      listed.add(terminalOutputStreamKey(terminal.id, terminal.generation));
    }
    for (const key of this.sleepingScreens.keys()) {
      if (!listed.has(key)) this.sleepingScreens.delete(key);
    }
  }

  private cursor(key: string): bigint {
    return this.outputBytes.get(key) ?? 0n;
  }

  private liveScreen(key: string): TerminalControlScreenV1 | null {
    if ((this.pendingWrites.get(key)?.size ?? 0) !== 0) return null;
    return this.views.get(key)?.controlScreen() ?? null;
  }
}
