/**
 * Live activity for one integrated terminal, derived from the PTY byte
 * stream the renderer already sees.
 *
 * Codex (and similar) prefix an OSC title with a braille or rotating
 * spinner while they work. That is the working signal. Claude Code prefixes
 * every title with ✳, including at rest — that mark is identity, not work,
 * and Claude never animates it. Its turns are reported instead by the
 * host-installed UserPromptSubmit/Stop hooks, which answer with an OSC 2
 * title carrying CLAUDE_ACTIVITY_TITLE_PREFIX; that working state holds
 * until the matching idle frame, never on a quiet timer. Its
 * PermissionRequest hook answers "waiting" while a permission dialog or
 * question is on screen.
 * Grok opens with the self-named title "grok". Once either kind of agent
 * title has been seen, later TUI redraws are not "working", because a
 * prompt that is merely sitting there still repaints.
 *
 * A plain shell never announces titles that way, so ordinary output is
 * working until a quiet tick, and BEL is still "needs input". Codex panes
 * are launched with `tui.notification_method=osc9`, so turn-complete and
 * approval alerts arrive as OSC 9 messages instead of bare BEL — BEL on a
 * Codex profile is ignored so finished turns do not flash "Needs attention".
 * CSI and other non-OSC escapes are never output: they are how a TUI moves
 * the cursor.
 *
 * Host protocol v44 moved "needs you" to Rust: `TerminalSummary.attention`
 * (crates/interface-terminal/src/attention.rs) is the answer the panes, the
 * sidebar, notifications and the phone read. This monitor still tracks its
 * own waiting state only to sequence working, idle and finished turns (a
 * prompt title must not end a dialog's turn); `TerminalActivityTracker`
 * publishes the host's waiting instead of it. The shared fixture
 * `terminal-attention-v44.json` holds both readings to the same answer.
 */

import { terminalOutputStreamKey } from "./terminal-output-state";
import type { TerminalSummary } from "../shared/ipc";

export type TerminalLiveActivity = "idle" | "working" | "waiting";

/** Quiet shells fall back to idle after this much silence. */
export const TERMINAL_ACTIVITY_QUIET_MS = 1_500;

/** Mirrors `CLAUDE_ACTIVITY_TITLE_PREFIX` in crates/interface-terminal/src/launch.rs. */
export const CLAUDE_ACTIVITY_TITLE_PREFIX = "interface-claude-activity:";
/** Rust-projected private activity v1; raw provider snapshots never reach React. */
export const TERMINAL_ACTIVITY_PREFIX = "interface-terminal-activity:v1:";

export interface TerminalActivityMonitor {
  readonly activity: TerminalLiveActivity;
  /** Explicit CLI completion signals, independent of the current activity dot. */
  readonly completedTurns: number;
}

/** Renderer projection only: never written back to the host or saved roster. */
export type TerminalActivitySummary = TerminalSummary & {
  readonly activity: TerminalLiveActivity;
  readonly completedTurns: number;
  /** The agent finished a turn and nobody has focused its pane since. */
  readonly done: boolean;
};

/**
 * The blue "done" dot (owner, 2026-09-25): stream keys whose agent reported a
 * finished turn while its pane was not the focused one. Focusing the pane,
 * new work, a request for input or the process going away clears it. Held in
 * the renderer only, like activity; a reload forgets it.
 */
export function markTerminalDone(
  done: ReadonlySet<string>, key: string, focusedKey: string | null,
): ReadonlySet<string> {
  if (key === focusedKey || done.has(key)) return done;
  return new Set(done).add(key);
}

/** Forget one stream's done mark; the same set when it had none. */
export function clearTerminalDone(
  done: ReadonlySet<string>, key: string | null,
): ReadonlySet<string> {
  if (key === null || !done.has(key)) return done;
  const next = new Set(done);
  next.delete(key);
  return next;
}

const ESC = 0x1b;
const BEL = 0x07;
const CAN = 0x18;
const SUB = 0x1a;
const MAX_PAYLOAD_BYTES = 512;
const utf8 = new TextDecoder("utf8", { fatal: false });

/** Window titles that only name the agent CLI. Shells must not match:
 *  a title of "zsh" is a prompt, not a reason to ignore later output. */
const SELF_NAMED_TITLES = new Set([
  "claude",
  "claude code",
  "codex",
  "grok",
  "qwen",
  "gemini",
  "antigravity",
  "agy",
]);

const enum Scan {
  Ground,
  Escape,
  EscapeIntermediate,
  CsiParameter,
  CsiIntermediate,
  Parameter,
  Payload,
  Discard,
  PayloadEscape,
  DiscardEscape,
  ControlString,
  ControlStringEscape,
}

interface Monitor extends TerminalActivityMonitor {
  activity: TerminalLiveActivity;
  completedTurns: number;
  completionReported: boolean;
  profile: string | undefined;
  knownAgent: boolean;
  seenAgentTitle: boolean;
  /** A hook-reported turn is running; only its idle frame ends it. */
  turn: boolean;
  hostActivity: boolean;
  /** Mirrors Rust: AGY's final native status wins over its capture hooks. */
  agyStatus: boolean;
  sawWork: boolean;
  scan: Scan;
  parameter: number;
  parameterDigits: number;
  payload: number[];
}

export function createTerminalActivityMonitor(): TerminalActivityMonitor {
  return newMonitor();
}

/** Host profile identity also covers title-less startup and renderer reloads. */
export function setTerminalActivityProfile(
  monitor: TerminalActivityMonitor, profile: string | undefined,
): TerminalLiveActivity | null {
  const state = asMonitor(monitor);
  if (state.profile === profile) return null;
  state.profile = profile;
  state.hostActivity = false;
  state.agyStatus = false;
  state.knownAgent = profile !== undefined && profile !== "shell";
  const wasFallbackWork = state.activity === "working" && !state.seenAgentTitle;
  state.seenAgentTitle = false;
  if (wasFallbackWork && state.knownAgent) {
    state.activity = "idle";
    return "idle";
  }
  return null;
}

/** xterm focus reports are protocol traffic, not a reply to a waiting agent. */
export function isTerminalFocusReport(bytes: Uint8Array): boolean {
  return bytes.length === 3 && bytes[0] === ESC && bytes[1] === 0x5b
    && (bytes[2] === 0x49 || bytes[2] === 0x4f);
}

/**
 * Input xterm writes on its own, never a reply to a waiting agent: a focus
 * report, or a capability report answering a query (`ESC [ ?…` DA1, DECRPM,
 * keyboard-mode replies; `ESC [ >…` DA2). The Rust host's input wake and
 * settings takeover use the same rule; `terminal-agent-activity-v1.txt` pins
 * it. Cursor-position reports stay input on both sides: `ESC [ 1 ; 2 R` is
 * also xterm's Shift+F3.
 */
export function isTerminalProtocolReply(bytes: Uint8Array): boolean {
  return isTerminalFocusReport(bytes)
    || bytes.length >= 3 && bytes[0] === ESC && bytes[1] === 0x5b
      && (bytes[2] === 0x3f || bytes[2] === 0x3e);
}

export function feedTerminalActivity(
  monitor: TerminalActivityMonitor, data: Uint8Array,
): TerminalLiveActivity | null {
  const state = asMonitor(monitor);
  const before = state.activity;
  state.sawWork = false;
  let index = 0;
  let escape = -1;
  let bell = -1;
  while (index < data.length) {
    const byte = data[index]!;
    // Plain output dominates a PTY chunk and every byte of it raises the same
    // idempotent "output" signal, so a whole run can be charged once. The
    // per-byte parser below still sees every byte of an escape sequence.
    //
    // A control byte already AT the cursor takes the per-byte path without
    // scanning: otherwise a chunk dense in BELs would re-scan its whole
    // remainder on every one of them, which is quadratic.
    if (state.scan === Scan.Ground && byte !== ESC && byte !== BEL) {
      // Remember each delimiter independently until it has been consumed.
      // Searching for an absent BEL after every ANSI run (or an absent ESC
      // after every bell) rescans the remaining chunk quadratically.
      if (escape < index) {
        escape = data.indexOf(ESC, index);
        if (escape < 0) escape = data.length;
      }
      if (bell < index) {
        bell = data.indexOf(BEL, index);
        if (bell < 0) bell = data.length;
      }
      const next = Math.min(escape, bell);
      applySignal(state, "output");
      if (next === data.length) break;
      index = next;
    }
    applyByte(state, data[index]!);
    index += 1;
  }
  return state.activity === before ? null : state.activity;
}

/** True when the last `feedTerminalActivity` chunk itself was evidence of work. */
export function terminalActivitySawWork(monitor: TerminalActivityMonitor): boolean {
  return asMonitor(monitor).sawWork;
}

export function noteTerminalInput(
  monitor: TerminalActivityMonitor,
): TerminalLiveActivity | null {
  const state = asMonitor(monitor);
  state.completionReported = false;
  if (state.hostActivity) return null;
  if (state.activity !== "waiting") return null;
  // Answering a permission prompt mid-turn resumes the turn, not a rest.
  state.activity = state.turn ? "working" : "idle";
  return state.activity;
}

export function noteTerminalQuiet(
  monitor: TerminalActivityMonitor,
): TerminalLiveActivity | null {
  const state = asMonitor(monitor);
  if (state.activity !== "working" || state.turn) return null;
  state.activity = "idle";
  return "idle";
}

/**
 * What the monitor says with its own waiting set aside: answering a dialog
 * mid-turn resumes the turn, anything else rests. Rust decides waiting.
 */
export function terminalActivityBeneathWaiting(
  monitor: TerminalActivityMonitor,
): TerminalLiveActivity {
  const state = asMonitor(monitor);
  if (state.activity !== "waiting") return state.activity;
  return state.turn ? "working" : "idle";
}

type Stamped<T> = T & { activity: TerminalLiveActivity; completedTurns: number; done: boolean };

/** Stamp renderer-derived activity onto host summaries at the publish boundary. */
export function withTerminalActivity<T extends {
  readonly id: string;
  readonly generation: number;
  readonly activity?: TerminalLiveActivity;
  readonly completedTurns?: number;
  readonly done?: boolean;
}>(
  terminals: readonly T[],
  activityByStream: ReadonlyMap<string, TerminalLiveActivity>,
  completedTurnsByStream?: ReadonlyMap<string, number>,
  doneStreams?: ReadonlySet<string>,
): readonly Stamped<T>[] {
  let changed = false;
  const stamped = terminals.map(terminal => {
    // Activity follows the same generation-fenced identity as PTY output. A
    // process that has produced nothing yet is idle, starting included.
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    const activity = activityByStream.get(key) ?? "idle";
    const completedTurns = completedTurnsByStream?.get(key) ?? 0;
    const done = doneStreams?.has(key) ?? false;
    if (terminal.activity === activity && terminal.completedTurns === completedTurns
        && terminal.done === done) {
      return terminal as Stamped<T>;
    }
    changed = true;
    return { ...terminal, activity, completedTurns, done };
  });
  // Re-stamping an already-stamped row must keep its identity: the result is
  // stored as state and read by memoized tabs and sidebar rows, so one
  // terminal's transition otherwise repaints every other terminal's chrome.
  return changed ? stamped : terminals as readonly Stamped<T>[];
}

function newMonitor(): Monitor {
  return {
    completedTurns: 0,
    completionReported: false,
    activity: "idle",
    profile: undefined,
    knownAgent: false,
    seenAgentTitle: false,
    turn: false,
    hostActivity: false,
    agyStatus: false,
    sawWork: false,
    scan: Scan.Ground,
    parameter: 0,
    parameterDigits: 0,
    payload: [],
  };
}

function asMonitor(monitor: TerminalActivityMonitor): Monitor {
  return monitor as Monitor;
}

function cancelled(byte: number): boolean {
  return byte === CAN || byte === SUB;
}

function intermediate(byte: number): boolean {
  return byte >= 0x20 && byte <= 0x2f;
}

function applyByte(state: Monitor, byte: number): void {
  switch (state.scan) {
    case Scan.Ground:
      if (byte === ESC) state.scan = Scan.Escape;
      else if (byte === BEL) applySignal(state, "bell");
      else applySignal(state, "output");
      return;
    case Scan.Escape:
      if (byte === ESC) return;
      if (cancelled(byte)) {
        state.scan = Scan.Ground;
        return;
      }
      if (byte === 0x5d) {
        state.scan = Scan.Parameter;
        state.parameter = 0;
        state.parameterDigits = 0;
        return;
      }
      if (byte === 0x5b) {
        state.scan = Scan.CsiParameter;
        return;
      }
      if (byte === 0x50 || byte === 0x58 || byte === 0x5e || byte === 0x5f) {
        state.scan = Scan.ControlString;
        return;
      }
      if (intermediate(byte)) {
        state.scan = Scan.EscapeIntermediate;
        return;
      }
      state.scan = Scan.Ground;
      return;
    case Scan.EscapeIntermediate:
      if (byte === ESC) {
        state.scan = Scan.Escape;
        return;
      }
      if (cancelled(byte) || (byte >= 0x30 && byte <= 0x7e)) {
        state.scan = Scan.Ground;
        return;
      }
      return;
    case Scan.CsiParameter:
      if (byte === ESC) {
        state.scan = Scan.Escape;
        return;
      }
      if (cancelled(byte) || (byte >= 0x40 && byte <= 0x7e)) {
        state.scan = Scan.Ground;
        return;
      }
      if (intermediate(byte)) state.scan = Scan.CsiIntermediate;
      return;
    case Scan.CsiIntermediate:
      if (byte === ESC) {
        state.scan = Scan.Escape;
        return;
      }
      if (cancelled(byte) || (byte >= 0x40 && byte <= 0x7e)) {
        state.scan = Scan.Ground;
      }
      return;
    case Scan.Parameter:
      consumeParameter(state, byte);
      return;
    case Scan.Payload:
      consumePayload(state, byte);
      return;
    case Scan.PayloadEscape:
      if (byte === 0x5c) {
        takeTitle(state);
        state.scan = Scan.Ground;
      } else {
        resetScan(state);
      }
      return;
    case Scan.Discard:
      if (byte === BEL) state.scan = Scan.Ground;
      else if (byte === ESC) state.scan = Scan.DiscardEscape;
      return;
    case Scan.DiscardEscape:
      if (byte === 0x5c) state.scan = Scan.Ground;
      else resetScan(state);
      return;
    case Scan.ControlString:
      if (byte === ESC) state.scan = Scan.ControlStringEscape;
      return;
    case Scan.ControlStringEscape:
      if (byte === 0x5c) state.scan = Scan.Ground;
      else if (byte !== ESC) state.scan = Scan.ControlString;
  }
}

function consumeParameter(state: Monitor, byte: number): void {
  if (byte >= 0x30 && byte <= 0x39) {
    if (state.parameterDigits >= 4) {
      state.scan = Scan.Discard;
      return;
    }
    state.parameter = state.parameter * 10 + (byte - 0x30);
    state.parameterDigits += 1;
    return;
  }
  if (byte === 0x3b) {
    if (
      state.parameterDigits > 0
      && (
        state.parameter === 0
        || state.parameter === 2
        || state.parameter === 9
        || state.parameter === 7501
      )
    ) {
      state.payload = [];
      state.scan = Scan.Payload;
    } else {
      state.scan = Scan.Discard;
    }
    return;
  }
  if (byte === BEL) state.scan = Scan.Ground;
  else if (byte === ESC) state.scan = Scan.DiscardEscape;
  else state.scan = Scan.Discard;
}

function consumePayload(state: Monitor, byte: number): void {
  if (byte === BEL) {
    takeTitle(state);
    state.scan = Scan.Ground;
    return;
  }
  if (byte === ESC) {
    state.scan = Scan.PayloadEscape;
    return;
  }
  if (state.payload.length >= MAX_PAYLOAD_BYTES) {
    state.payload = [];
    state.scan = Scan.Discard;
    return;
  }
  state.payload.push(byte);
}

function takeTitle(state: Monitor): void {
  const raw = utf8.decode(Uint8Array.from(state.payload));
  const parameter = state.parameter;
  state.payload = [];
  if (state.agyStatus && parameter !== 7501) return;
  if ((parameter === 0 || parameter === 2) && raw.startsWith(TERMINAL_ACTIVITY_PREFIX)) {
    const signal = raw.slice(TERMINAL_ACTIVITY_PREFIX.length);
    if (signal !== "working" && signal !== "waiting" && signal !== "idle" && signal !== "complete") return;
    state.hostActivity = true;
    state.seenAgentTitle = true;
    state.turn = signal === "working" || signal === "waiting";
    if (signal === "complete") completeTurn(state);
    else if (signal === "working" || signal === "waiting") state.completionReported = false;
    state.activity = signal === "complete" ? "idle" : signal;
    return;
  }
  if (state.hostActivity && !(state.profile === "gemini" && parameter === 7501)) return;
  if (parameter === 9) {
    // Only Codex panes are launched with OSC 9 notifications; another CLI's
    // OSC 9 (an iTerm2-style alert) is not a Codex turn ending. Rust's
    // workspace sleep reads them the same way.
    if (state.profile !== "codex") return;
    const kind = codexOsc9NotificationKind(raw);
    if (kind === "waiting") applySignal(state, "osc9-waiting");
    else if (kind === "idle") applySignal(state, "osc9-idle");
    return;
  }
  if (parameter === 7501) {
    const status = osc7501State(raw);
    if (status !== null && state.profile === "gemini") {
      state.agyStatus = true;
      state.hostActivity = false;
    }
    if (status === "working") applySignal(state, "turn-start");
    else if (status === "blocked") applySignal(state, "turn-waiting");
    else if (status === "done") applySignal(state, "turn-end");
    else if (status === "idle" || status === "error") {
      state.seenAgentTitle = true;
      state.turn = false;
      state.activity = "idle";
    } else if (status === "clear") {
      state.seenAgentTitle = state.knownAgent;
      state.turn = false;
      state.activity = "idle";
    }
    return;
  }
  if (raw.startsWith(CLAUDE_ACTIVITY_TITLE_PREFIX)) {
    const turn = raw.slice(CLAUDE_ACTIVITY_TITLE_PREFIX.length);
    if (turn === "working") applySignal(state, "turn-start");
    else if (turn === "waiting") applySignal(state, "turn-waiting");
    else if (turn === "idle") applySignal(state, "turn-end");
    return;
  }
  const kind = titleKind(raw);
  if (kind === "working") applySignal(state, "working");
  else if (kind === "agent") applySignal(state, "agent");
  else if (kind === "prompt") applySignal(state, "prompt");
}

function resetScan(state: Monitor): void {
  state.scan = Scan.Ground;
  state.payload = [];
  state.parameter = 0;
  state.parameterDigits = 0;
}

type Signal =
  | "working"
  | "agent"
  | "prompt"
  | "bell"
  | "output"
  | "osc9-waiting"
  | "osc9-idle"
  | "turn-start"
  | "turn-waiting"
  | "turn-end";

function applySignal(state: Monitor, signal: Signal): void {
  switch (signal) {
    case "working":
      state.completionReported = false;
      state.seenAgentTitle = true;
      state.activity = "working";
      state.sawWork = true;
      return;
    case "agent":
      // Claude renames the session mid-turn; that identity title does not
      // end a hook-reported turn.
      state.seenAgentTitle = true;
      if (state.activity !== "waiting") state.activity = state.turn ? "working" : "idle";
      return;
    case "prompt":
      if ((!state.seenAgentTitle && !state.knownAgent) || state.activity === "waiting") return;
      state.activity = state.turn ? "working" : "idle";
      return;
    case "bell":
      // Codex is forced onto OSC 9; bare BEL there is turn-complete noise.
      // Claude may ring beside its Stop frame too. The explicit end wins
      // until real input or a new turn starts, in either arrival order.
      if (state.hostActivity || state.agyStatus || state.profile === "codex" || (state.profile === "claude" && state.completionReported)) return;
      state.activity = "waiting";
      return;
    case "osc9-waiting":
      state.completionReported = false;
      state.activity = "waiting";
      return;
    case "osc9-idle":
      completeTurn(state);
      state.activity = "idle";
      return;
    case "turn-start":
      state.completionReported = false;
      // Hook frames only ever come from Claude; a working turn is not the
      // fallback work a profile change may demote. sawWork stays false so
      // no quiet timer is armed against a turn that reports its own end.
      state.seenAgentTitle = true;
      state.turn = true;
      state.activity = "working";
      return;
    case "turn-waiting":
      // Claude is showing a permission dialog or a question mid-turn.
      // Answering it resumes the turn (noteTerminalInput); the next tool
      // call or the turn's end replace it too.
      state.completionReported = false;
      state.seenAgentTitle = true;
      state.turn = true;
      state.activity = "waiting";
      return;
    case "turn-end":
      completeTurn(state);
      state.turn = false;
      state.activity = "idle";
      return;
    case "output":
      if (state.seenAgentTitle || state.knownAgent || state.activity === "waiting") return;
      state.activity = "working";
      state.sawWork = true;
  }
}

function completeTurn(state: Monitor): void {
  if (state.completionReported) return;
  state.completionReported = true;
  state.completedTurns += 1;
}

function isTransientSpinner(character: string): boolean {
  const code = character.codePointAt(0);
  if (code === undefined) return false;
  if (code >= 0x2800 && code <= 0x28ff) return true;
  return "●○◐◓◑◒◆▪★*⏳⚡".includes(character);
}

function isBrandMark(character: string): boolean {
  return "✳✻✽·•".includes(character);
}

/**
 * Antigravity (`agy`) OSC 7501 `ProgramStatus` frames: colon-separated
 * `key=value` fields with `state=working|blocked|done|error|idle|clear`.
 * Mirrors `osc7501_state` in `crates/interface-terminal/src/attention.rs`.
 */
export function osc7501State(
  raw: string,
): "working" | "blocked" | "done" | "error" | "idle" | "clear" | null {
  for (const field of raw.split(":")) {
    if (!field.startsWith("state=")) continue;
    const state = field.slice("state=".length);
    if (
      state === "working"
      || state === "blocked"
      || state === "done"
      || state === "error"
      || state === "idle"
      || state === "clear"
    ) {
      return state;
    }
    return null;
  }
  return null;
}

/**
 * Codex OSC 9 payloads (from `tui.notification_method=osc9`).
 * Approval-family messages need attention; turn-complete is idle.
 * Exact shapes from codex-rs/tui chatwidget notifications:
 *   "Approval requested: …" | "Approval requested by …"
 *   "Codex wants to edit …"
 *   "Plan mode prompt: …"
 *   "Agent turn complete" | truncated assistant reply preview
 */
export function codexOsc9NotificationKind(raw: string): "waiting" | "idle" | null {
  const message = trimTitle(raw);
  if (message === "") return null;
  const lower = message.toLowerCase();
  if (
    lower.startsWith("approval requested")
    || lower.startsWith("codex wants to edit")
    || lower.startsWith("plan mode prompt:")
  ) {
    return "waiting";
  }
  return "idle";
}

/** How a window title reads: a transient spinner is work, a brand mark or the
 * CLI's own name is the agent at rest, anything else is a prompt/task name. */
export function titleKind(raw: string): "working" | "agent" | "prompt" | null {
  const title = trimTitle(raw);
  if (title === "") return null;
  if (isTransientSpinner(title[0]!)) return "working";
  let index = 0;
  let hadBrand = false;
  while (index < title.length && isBrandMark(title[index]!)) {
    hadBrand = true;
    index += 1;
    while (index < title.length && isTrimmable(title[index]!)) index += 1;
  }
  if (index < title.length && isTransientSpinner(title[index]!)) return "working";
  const rest = title.slice(index).trim().toLowerCase();
  if (hadBrand || SELF_NAMED_TITLES.has(rest)) return "agent";
  return "prompt";
}

function isTrimmable(character: string): boolean {
  return character.trim() === "" || character.charCodeAt(0) < 32;
}

/** An OSC payload without the whitespace and C0 controls around it. */
function trimTitle(raw: string): string {
  let start = 0;
  let end = raw.length;
  while (start < end && isTrimmable(raw[start]!)) start += 1;
  while (end > start && isTrimmable(raw[end - 1]!)) end -= 1;
  return raw.slice(start, end);
}
