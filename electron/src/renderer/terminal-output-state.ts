import type { TerminalLifecycleEvent, TerminalSummary } from "../shared/ipc";
import { equalValue } from "./stable-values";

export interface TerminalRegistryState {
  readonly revision: number;
  readonly terminals: readonly TerminalSummary[];
}

export interface TerminalLifecycleApplication {
  readonly registry: TerminalRegistryState;
  /** Whether this event's terminal payload, not only its revision, won. */
  readonly payloadApplied: boolean;
}

export type TerminalOutputDisposition = "deliver" | "buffer" | "discard";

/**
 * Reconcile independently-delivered PTY bytes with the latest control-plane
 * summary. A later generation may reach Electron before its lifecycle push,
 * so it must wait rather than being mistaken for stale output.
 */
export function terminalOutputDisposition(
  knownGeneration: number | undefined,
  incomingGeneration: number,
  retired: boolean,
): TerminalOutputDisposition {
  if (retired || !Number.isSafeInteger(incomingGeneration) || incomingGeneration < 1) {
    return "discard";
  }
  if (knownGeneration === undefined || incomingGeneration > knownGeneration) return "buffer";
  if (incomingGeneration < knownGeneration) return "discard";
  return "deliver";
}

export function terminalOutputStreamKey(terminalId: string, generation: number): string {
  return `${terminalId}:${generation.toString()}`;
}

/** Parser state at a PTY chunk boundary. Only ESC-prefixed controls matter. */
export type TerminalReplayBoundary =
  | "ground"
  | "escape"
  | "escape-intermediate"
  | "csi-parameter"
  | "csi-intermediate"
  | "osc"
  | "osc-escape"
  | "control-string"
  | "control-string-escape";

export const INITIAL_TERMINAL_REPLAY_BOUNDARY: TerminalReplayBoundary = "ground";

export interface TerminalReplayChunk {
  readonly bytes: Uint8Array;
  /** State immediately before `bytes[0]` was received. */
  readonly boundary: TerminalReplayBoundary;
}

const ESC = 0x1b;
const BEL = 0x07;
const CAN = 0x18;
const SUB = 0x1a;

function cancelled(byte: number): boolean {
  return byte === CAN || byte === SUB;
}

function intermediate(byte: number): boolean {
  return byte >= 0x20 && byte <= 0x2f;
}

function final(byte: number): boolean {
  return byte >= 0x30 && byte <= 0x7e;
}

function nextTerminalReplayBoundary(
  boundary: TerminalReplayBoundary, byte: number,
): TerminalReplayBoundary {
  switch (boundary) {
    case "ground":
      return byte === ESC ? "escape" : "ground";
    case "escape":
      if (byte === ESC) return "escape";
      if (cancelled(byte)) return "ground";
      if (byte === 0x5b) return "csi-parameter";
      if (byte === 0x5d) return "osc";
      if (byte === 0x50 || byte === 0x58 || byte === 0x5e || byte === 0x5f) {
        return "control-string";
      }
      if (intermediate(byte)) return "escape-intermediate";
      return final(byte) ? "ground" : "escape";
    case "escape-intermediate":
      if (byte === ESC) return "escape";
      if (cancelled(byte) || final(byte)) return "ground";
      return "escape-intermediate";
    case "csi-parameter":
      if (byte === ESC) return "escape";
      if (cancelled(byte)) return "ground";
      if (byte >= 0x30 && byte <= 0x3f) return "csi-parameter";
      if (intermediate(byte)) return "csi-intermediate";
      return byte >= 0x40 && byte <= 0x7e ? "ground" : "csi-parameter";
    case "csi-intermediate":
      if (byte === ESC) return "escape";
      if (cancelled(byte)) return "ground";
      return byte >= 0x40 && byte <= 0x7e ? "ground" : "csi-intermediate";
    case "osc":
      if (byte === BEL) return "ground";
      return byte === ESC ? "osc-escape" : "osc";
    case "osc-escape":
      if (byte === 0x5c || byte === BEL) return "ground";
      return byte === ESC ? "osc-escape" : "osc";
    case "control-string":
      return byte === ESC ? "control-string-escape" : "control-string";
    case "control-string-escape":
      if (byte === 0x5c) return "ground";
      return byte === ESC ? "control-string-escape" : "control-string";
  }
}

/** Advance the replay parser while the full, ordered PTY stream is available. */
export function advanceTerminalReplayBoundary(
  boundary: TerminalReplayBoundary, data: Uint8Array,
  observe?: (boundary: TerminalReplayBoundary, byte: number) => void,
): TerminalReplayBoundary {
  let next = boundary;
  for (let index = 0; index < data.length; index += 1) {
    // Ground absorbs every byte but ESC, so a chunk of ordinary output only
    // needs the search for the next ESC, not a state transition per byte.
    if (next === "ground") {
      const escape = data.indexOf(ESC, index);
      if (escape < 0) return "ground";
      index = escape;
    }
    observe?.(next, data[index]!);
    next = nextTerminalReplayBoundary(next, data[index]!);
  }
  return next;
}

/**
 * Reconcile a retained chunk tail with the parser boundary recorded before it.
 *
 * If truncation removed the bytes that opened an ESC/CSI/string sequence,
 * replay begins only after that proven control sequence terminates. Once a
 * ground-state byte boundary is recovered, all later chunks remain byte-exact.
 * Ground-state `[200]` and digit-leading prose are therefore never guessed to
 * be escape tails.
 */
export function replayTerminalOutputChunks(
  chunks: readonly TerminalReplayChunk[],
): readonly Uint8Array[] {
  const replay: Uint8Array[] = [];
  let synchronized = false;
  for (const chunk of chunks) {
    if (synchronized || chunk.boundary === "ground") {
      synchronized = true;
      if (chunk.bytes.byteLength > 0) replay.push(chunk.bytes);
      continue;
    }
    let boundary: TerminalReplayBoundary = chunk.boundary;
    for (let index = 0; index < chunk.bytes.byteLength; index += 1) {
      boundary = nextTerminalReplayBoundary(boundary, chunk.bytes[index]!);
      if (boundary !== "ground") continue;
      synchronized = true;
      if (index + 1 < chunk.bytes.byteLength) replay.push(chunk.bytes.subarray(index + 1));
      break;
    }
  }
  return replay;
}

/**
 * Agent TUIs paint an alternate-screen frame. Replaying a truncated tail of
 * that frame leaves CUP/EL leftovers that never get a full redraw, so a pane
 * can show a shrunk TUI in a sea of blank cells. Shells have no such frame;
 * keep their tail — unless the shell is running a full-screen TUI (OpenCode
 * typed at the prompt), which is the same torn frame under a shell profile.
 */
export function terminalReplayShouldRestoreChunks(
  truncated: boolean,
  profileId: string | undefined,
  alternateScreen = false,
): boolean {
  if (!truncated) return true;
  return !alternateScreen && (profileId === undefined || profileId === "shell");
}

/** DECSET 2004, written to xterm only: it never reaches the PTY. */
export const BRACKETED_PASTE_ON = "\x1b[?2004h";

/**
 * Private modes live in the xterm instance, not in the retained output. A
 * remounted pane replays a tail (or, after a torn frame, nothing but a
 * SIGWINCH repaint), and neither carries the `CSI ? 2004 h` the program sent
 * once at startup, so xterm would paste unbracketed from then on. Claude Code
 * and Codex then take a dropped image path, or a multi-line paste, as typed
 * text. Both enable bracketed paste at startup and never turn it off, so a
 * remount of their panes re-asserts it. Shells and the other CLIs are left to
 * whatever the replay says.
 */
export function terminalReplayAssertsBracketedPaste(profileId: string | undefined): boolean {
  return profileId === "claude" || profileId === "codex";
}

/** A one-cell smaller geometry so an equal-size host resize still SIGWINCHs. */
export function terminalRedrawKickSize(
  size: { readonly columns: number; readonly rows: number },
): { columns: number; rows: number } | null {
  if (size.rows > 1) return { columns: size.columns, rows: size.rows - 1 };
  if (size.columns > 2) return { columns: size.columns - 1, rows: size.rows };
  return null;
}

function finalState(state: TerminalSummary["state"]): boolean {
  return state === "exited" || state === "failed";
}

/**
 * Whether `incoming` wins the per-terminal merge, and the list that results.
 * Winning and changing the list are separate facts: a resize reply and its
 * lifecycle echo carry the same row, and whichever arrives second wins without
 * changing anything, so the list and every row in it keep their identity.
 */
function mergeSummary(
  terminals: readonly TerminalSummary[], incoming: TerminalSummary,
): { readonly terminals: readonly TerminalSummary[]; readonly applied: boolean } {
  const existing = terminals.find(terminal => terminal.id === incoming.id);
  if (existing !== undefined) {
    if (existing.generation > incoming.generation) return { terminals, applied: false };
    if (existing.generation === incoming.generation) {
      if (finalState(existing.state) && !finalState(incoming.state)) {
        return { terminals, applied: false };
      }
      // Host stamps are all rendered through
      // `interface_contracts::CANONICAL_TIMESTAMP_FORMAT`
      // (crates/interface-contracts/src/validation.rs:6), so they are
      // fixed-width UTC with a trailing Z: byte order IS time order. `id`
      // still compares by locale — it is an opaque identifier, not a stamp.
      if (existing.updatedAt > incoming.updatedAt) return { terminals, applied: false };
    }
    if (equalValue(existing, incoming)) return { terminals, applied: true };
  }
  return {
    terminals: [...terminals.filter(terminal => terminal.id !== incoming.id), incoming]
      .sort((left, right) => (left.createdAt < right.createdAt ? -1
        : left.createdAt > right.createdAt ? 1 : 0)
        || left.id.localeCompare(right.id)),
    applied: true,
  };
}

/**
 * Merge summaries arriving on the independently scheduled control response and
 * lifecycle-push paths. A late create/restart response must never move an
 * already-finished generation back to Running. A row equal to the one already
 * held changes nothing and returns `terminals` itself.
 */
export function mergeTerminalSummary(
  terminals: readonly TerminalSummary[], incoming: TerminalSummary,
): readonly TerminalSummary[] {
  return mergeSummary(terminals, incoming).terminals;
}

/**
 * Apply one host lifecycle event only when it advances the accepted registry.
 * Returning the exact current object is the caller's side-effect fence: no
 * workspace filing, persistence, restart, or hand-off may run for that event.
 */
export function applyTerminalLifecycleEvent(
  current: TerminalRegistryState, event: TerminalLifecycleEvent,
): TerminalLifecycleApplication {
  if (event.revision <= current.revision) {
    return { registry: current, payloadApplied: false };
  }
  if (event.kind === "upsert") {
    // An echo equal to the held row still wins: its filing, seating and
    // hand-off run exactly as for a changed row. Only the list is kept.
    const { terminals, applied } = mergeSummary(current.terminals, event.terminal);
    return {
      registry: { revision: event.revision, terminals },
      payloadApplied: applied,
    };
  }
  const blockedByNewerGeneration = current.terminals.some(terminal =>
    terminal.id === event.id && terminal.generation > event.generation);
  const terminals = blockedByNewerGeneration
    ? current.terminals
    : current.terminals.filter(terminal => terminal.id !== event.id
      || terminal.generation > event.generation);
  return {
    registry: { revision: event.revision, terminals },
    payloadApplied: !blockedByNewerGeneration,
  };
}

/**
 * Accept a full host snapshot unless a newer lifecycle revision already won.
 * For every accepted revision, preserve a current control-response row only
 * when the snapshot has the same terminal and that row wins the ordinary
 * generation/state/timestamp merge. Snapshot omissions remain authoritative:
 * the matching lifecycle removal has the same revision and will be fenced.
 */
export function applyTerminalRegistrySnapshot(
  current: TerminalRegistryState, snapshot: TerminalRegistryState,
): TerminalRegistryState {
  if (snapshot.revision < current.revision) return current;
  let terminals = snapshot.terminals;
  for (const currentTerminal of current.terminals) {
    const snapshotTerminal = snapshot.terminals.find(
      terminal => terminal.id === currentTerminal.id,
    );
    if (snapshotTerminal === undefined) continue;
    // Snapshot is the incoming authority, so it wins generation/timestamp
    // ties. Preserve the unversioned control row only when it is strictly
    // newer under the same merge rules.
    if (!mergeSummary([currentTerminal], snapshotTerminal).applied) {
      terminals = mergeTerminalSummary(terminals, currentTerminal);
    }
  }
  const sameAsCurrent = terminals.length === current.terminals.length
    && terminals.every((terminal, index) => terminal === current.terminals[index]);
  if (sameAsCurrent) terminals = current.terminals;
  return snapshot.revision === current.revision && sameAsCurrent
    ? current
    : { revision: snapshot.revision, terminals };
}

/**
 * Drop detached output that can no longer be mounted. Supplying a generation
 * retains that generation and any future generation already waiting for its
 * lifecycle summary; omitting it retires every stream for the terminal ID.
 */
export function pruneTerminalOutputBuffers<T>(
  buffers: Map<string, T>,
  terminalId: string,
  minimumGeneration?: number,
): number {
  const prefix = `${terminalId}:`;
  let removed = 0;
  for (const key of buffers.keys()) {
    if (!key.startsWith(prefix)) continue;
    if (minimumGeneration !== undefined) {
      const generation = Number(key.slice(prefix.length));
      if (!Number.isSafeInteger(generation) || generation >= minimumGeneration) continue;
    }
    if (buffers.delete(key)) removed += 1;
  }
  return removed;
}
