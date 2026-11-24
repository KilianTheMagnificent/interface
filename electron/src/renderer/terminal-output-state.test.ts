import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  applyTerminalLifecycleEvent, applyTerminalRegistrySnapshot,
  advanceTerminalReplayBoundary, INITIAL_TERMINAL_REPLAY_BOUNDARY,
  mergeTerminalSummary, pruneTerminalOutputBuffers,
  replayTerminalOutputChunks,
  terminalOutputDisposition, terminalOutputStreamKey,
  terminalRedrawKickSize, terminalReplayShouldRestoreChunks,
  BRACKETED_PASTE_ON, terminalReplayAssertsBracketedPaste,
  type TerminalReplayBoundary, type TerminalReplayChunk,
} from "./terminal-output-state";
import type { TerminalSummary } from "../shared/ipc";

const TERMINAL = "00000000-0000-4000-8000-000000000001";
const OTHER = "00000000-0000-4000-8000-000000000002";

describe("terminal output generation reconciliation", () => {
  it("delivers the current generation, buffers a future one, and fences stale output", () => {
    expect(terminalOutputDisposition(4, 4, false)).toBe("deliver");
    expect(terminalOutputDisposition(4, 5, false)).toBe("buffer");
    expect(terminalOutputDisposition(4, 3, false)).toBe("discard");
    expect(terminalOutputDisposition(undefined, 1, false)).toBe("buffer");
    expect(terminalOutputDisposition(4, 5, true)).toBe("discard");
  });

  it("does not let a late control response revive a finished generation", () => {
    const running: TerminalSummary = {
      id: TERMINAL,
      generation: 1,
      title: "Shell",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:00.000Z",
    };
    const exited: TerminalSummary = {
      ...running,
      state: "exited",
      exit: { code: 0 },
      // Millisecond timestamps can collide for a shell that exits immediately.
      updatedAt: running.updatedAt,
    };
    const current = mergeTerminalSummary([running], exited);
    expect(current[0]?.state).toBe("exited");
    expect(mergeTerminalSummary(current, running)).toBe(current);
  });

  it("rejects older same-generation summaries while accepting a restart", () => {
    const current: TerminalSummary = {
      id: TERMINAL,
      generation: 2,
      title: "New title",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 100, rows: 30 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:02.000Z",
    };
    const stale = { ...current, title: "Old title", updatedAt: "2026-08-27T00:00:01.000Z" };
    expect(mergeTerminalSummary([current], stale)[0]?.title).toBe("New title");
    const restarted = { ...current, generation: 3, updatedAt: "2026-08-27T00:00:03.000Z" };
    expect(mergeTerminalSummary([current], restarted)[0]?.generation).toBe(3);
  });

  it("keeps the list and its rows when a reply and its push carry the same row", () => {
    const resized: TerminalSummary = {
      id: TERMINAL,
      generation: 1,
      title: "Shell",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 120, rows: 40 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:05.000Z",
      launch: { version: 1, profileId: "shell" },
    };
    const other: TerminalSummary = { ...resized, id: OTHER, createdAt: "2026-08-27T00:00:01.000Z" };
    const terminals = [resized, other];
    // The push is a separate object with the same content (structured clone).
    const echo = structuredClone(resized);
    expect(mergeTerminalSummary(terminals, echo)).toBe(terminals);
    // A row that differs in anything at the same stamp still replaces it.
    const renamed = { ...echo, title: "Renamed" };
    const merged = mergeTerminalSummary(terminals, renamed);
    expect(merged).not.toBe(terminals);
    expect(merged.find(terminal => terminal.id === TERMINAL)).toBe(renamed);
    expect(merged.find(terminal => terminal.id === OTHER)).toBe(other);

    // The lifecycle echo still wins: its filing and hand-off run as before,
    // while the revision moves and the drawn list stays.
    const current = { revision: 3, terminals };
    const pushed = applyTerminalLifecycleEvent(current, { kind: "upsert", revision: 4, terminal: echo });
    expect(pushed.payloadApplied).toBe(true);
    expect(pushed.registry.revision).toBe(4);
    expect(pushed.registry.terminals).toBe(terminals);
    // An older echo is still fenced, as before.
    const stale = applyTerminalLifecycleEvent(current, { kind: "upsert", revision: 5,
      terminal: { ...echo, updatedAt: "2026-08-27T00:00:04.000Z" } });
    expect(stale.payloadApplied).toBe(false);
    expect(stale.registry.terminals).toBe(terminals);
  });

  it("prunes obsolete generations without deleting future bytes awaiting a summary", () => {
    const buffers = new Map([
      [terminalOutputStreamKey(TERMINAL, 1), "old"],
      [terminalOutputStreamKey(TERMINAL, 2), "current"],
      [terminalOutputStreamKey(TERMINAL, 3), "future"],
      [terminalOutputStreamKey(OTHER, 1), "other"],
    ]);
    expect(pruneTerminalOutputBuffers(buffers, TERMINAL, 2)).toBe(1);
    expect([...buffers.values()]).toEqual(["current", "future", "other"]);
    expect(pruneTerminalOutputBuffers(buffers, TERMINAL)).toBe(2);
    expect([...buffers.values()]).toEqual(["other"]);
  });

  it("makes a late lifecycle R-1 inert after a newer registry snapshot", () => {
    const current = {
      revision: 7,
      terminals: [] as TerminalSummary[],
    };
    const terminal: TerminalSummary = {
      id: TERMINAL,
      generation: 1,
      title: "Claude",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:00.000Z",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: "provider-session", resume: true },
    };
    const newer = applyTerminalRegistrySnapshot(current, {
      revision: 9,
      terminals: [terminal],
    });
    const late = applyTerminalLifecycleEvent(newer, {
      kind: "upsert",
      revision: 8,
      terminal: {
        ...terminal,
        launchSession: { version: 1, id: "stale-provider-session", resume: true },
      },
    });
    const authoritativeWorkspaceWrites: TerminalSummary[] = [];
    if (late.payloadApplied) authoritativeWorkspaceWrites.push(terminal);

    expect(late.registry).toBe(newer);
    expect(late.registry.terminals[0]?.launchSession?.id).toBe("provider-session");
    expect(authoritativeWorkspaceWrites).toEqual([]);
  });

  it("advances a newer revision without applying its stale terminal payload", () => {
    const currentTerminal: TerminalSummary = {
      id: TERMINAL,
      generation: 2,
      title: "Claude",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:02.000Z",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: "current-provider-session", resume: true },
    };
    const current = { revision: 7, terminals: [currentTerminal] };
    const late = applyTerminalLifecycleEvent(current, {
      kind: "upsert",
      revision: 8,
      terminal: {
        ...currentTerminal,
        generation: 1,
        updatedAt: "2026-08-27T00:00:01.000Z",
        launchSession: { version: 1, id: "stale-provider-session", resume: true },
      },
    });

    expect(late.registry).toEqual({ revision: 8, terminals: [currentTerminal] });
    expect(late.registry.terminals).toBe(current.terminals);
    expect(late.payloadApplied).toBe(false);
  });

  it("does not let an equal-revision snapshot replace a newer control summary", () => {
    const currentTerminal: TerminalSummary = {
      id: TERMINAL,
      generation: 2,
      title: "Claude",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:02.000Z",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: "current-provider-session", resume: true },
    };
    const current = { revision: 7, terminals: [currentTerminal] };
    const accepted = applyTerminalRegistrySnapshot(current, {
      revision: 7,
      terminals: [{
        ...currentTerminal,
        generation: 1,
        updatedAt: "2026-08-27T00:00:01.000Z",
        launchSession: { version: 1, id: "stale-provider-session", resume: true },
      }],
    });

    expect(accepted).toBe(current);
    expect(accepted.terminals[0]?.launchSession?.id).toBe("current-provider-session");
  });

  it("applies a snapshot removal even though its lifecycle shares the revision", () => {
    const currentTerminal: TerminalSummary = {
      id: TERMINAL,
      generation: 1,
      title: "Claude",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:00.000Z",
    };
    const current = { revision: 7, terminals: [currentTerminal] };

    const snapshot = applyTerminalRegistrySnapshot(current, {
      revision: 7,
      terminals: [],
    });
    expect(snapshot).toEqual({ revision: 7, terminals: [] });

    const removed = applyTerminalLifecycleEvent(snapshot, {
      kind: "removed",
      revision: 7,
      id: TERMINAL,
      generation: 1,
    });
    expect(removed.registry).toBe(snapshot);
    expect(removed.payloadApplied).toBe(false);
    expect(removed.registry.terminals).toEqual([]);
  });

  it("advances a higher snapshot revision without replacing a newer control row", () => {
    const currentTerminal: TerminalSummary = {
      id: TERMINAL,
      generation: 3,
      title: "Claude",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:03.000Z",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: "current-provider-session", resume: true },
    };
    const current = { revision: 5, terminals: [currentTerminal] };
    const accepted = applyTerminalRegistrySnapshot(current, {
      revision: 6,
      terminals: [{
        ...currentTerminal,
        generation: 2,
        updatedAt: "2026-08-27T00:00:02.000Z",
        launchSession: { version: 1, id: "stale-provider-session", resume: true },
      }],
    });

    expect(accepted.revision).toBe(6);
    expect(accepted.terminals).toBe(current.terminals);
    expect(accepted.terminals[0]?.launchSession?.id).toBe("current-provider-session");
  });

  it("lets a higher-revision snapshot binding win a per-terminal timestamp tie", () => {
    const currentTerminal: TerminalSummary = {
      id: TERMINAL,
      generation: 2,
      title: "Claude",
      root: "/tmp/project",
      cwd: "/tmp/project",
      state: "running",
      size: { columns: 80, rows: 24 },
      createdAt: "2026-08-27T00:00:00.000Z",
      updatedAt: "2026-08-27T00:00:02.000Z",
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, resume: true },
    };
    const snapshotTerminal: TerminalSummary = {
      ...currentTerminal,
      launchSession: { version: 1, id: "confirmed-provider-session", resume: true },
    };
    const accepted = applyTerminalRegistrySnapshot(
      { revision: 5, terminals: [currentTerminal] },
      { revision: 6, terminals: [snapshotTerminal] },
    );

    expect(accepted.revision).toBe(6);
    expect(accepted.terminals[0]).toBe(snapshotTerminal);
    expect(accepted.terminals[0]?.launchSession?.id).toBe("confirmed-provider-session");
  });
});

describe("advanceTerminalReplayBoundary", () => {
  it("walks PTY bytes with an indexed loop", () => {
    const source = readFileSync(new URL("./terminal-output-state.ts", import.meta.url), "utf8");
    const fn = source.slice(
      source.indexOf("export function advanceTerminalReplayBoundary"),
      source.indexOf("export function replayTerminalOutputChunks"),
    );
    expect(fn).toContain("for (let index = 0; index < data.length; index += 1)");
    expect(fn).not.toContain("for (const ");
  });

  it("skips ground-state bytes without changing the boundary a byte walk finds", () => {
    const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
    // The reference: the same parser fed one byte at a time, so a chunk-wide
    // skip that lands on the wrong byte disagrees with it.
    const byteByByte = (
      boundary: TerminalReplayBoundary, bytes: Uint8Array,
    ): TerminalReplayBoundary => {
      let at = boundary;
      for (const byte of bytes) at = advanceTerminalReplayBoundary(at, Uint8Array.of(byte));
      return at;
    };
    const cases: readonly (readonly [TerminalReplayBoundary, string])[] = [
      // No ESC at all: ground absorbs the whole buffer.
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "plain output with no controls\r\n"],
      // ESC in the middle, and one that finishes inside the buffer.
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "before \x1b[31mred\x1b[0m after"],
      // ESC as the very first byte, and as the very last one.
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "\x1b[2J"],
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "text at the boundary\x1b"],
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "one\x1b[38;2;"],
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "osc \x1b]0;title\x07back to ground"],
      // Multi-byte ground text: UTF-8 continuation bytes have the high bit set
      // and must not be mistaken for a control byte by the chunk-wide skip.
      [INITIAL_TERMINAL_REPLAY_BOUNDARY, "h\u00e9llo \u00fcnicode\x1b[0m"],
      // Entering mid-sequence: the skip must not fire outside ground.
      ["csi-parameter", "38;2;20m then prose"],
      ["osc", "still the title"],
      ["escape", "[1mbold"],
      ["control-string", "payload\x1b\\done"],
      // Empty input keeps whatever boundary it was handed.
      ["escape-intermediate", ""],
    ];
    for (const [boundary, text] of cases) {
      const bytes = encode(text);
      expect(advanceTerminalReplayBoundary(boundary, bytes), text)
        .toBe(byteByByte(boundary, bytes));
    }
    expect(advanceTerminalReplayBoundary(
      INITIAL_TERMINAL_REPLAY_BOUNDARY, encode("text at the boundary\x1b"),
    )).toBe("escape");
    expect(advanceTerminalReplayBoundary(
      INITIAL_TERMINAL_REPLAY_BOUNDARY, encode("plain output with no controls\r\n"),
    )).toBe("ground");
  });
});

describe("boundary-aware terminal replay", () => {
  const decode = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);
  const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
  const decodedReplay = (chunks: readonly TerminalReplayChunk[]): string =>
    decode(Uint8Array.from(replayTerminalOutputChunks(chunks).flatMap(chunk => [...chunk])));

  function chunk(
    text: string, boundary: TerminalReplayBoundary,
  ): { replay: TerminalReplayChunk; end: TerminalReplayBoundary } {
    const bytes = encode(text);
    return {
      replay: { bytes, boundary },
      end: advanceTerminalReplayBoundary(boundary, bytes),
    };
  }

  it("drops a numeric CSI tail only when dropped bytes prove its parser context", () => {
    const boundary = advanceTerminalReplayBoundary(
      INITIAL_TERMINAL_REPLAY_BOUNDARY, encode("\x1b[38;2;"),
    );
    const first = chunk("200;10", boundary);
    const second = chunk(";5mHello", first.end);
    expect(boundary).toBe("csi-parameter");
    expect(decodedReplay([first.replay, second.replay])).toBe("Hello");
  });

  it("recovers when truncation lands immediately after ESC", () => {
    const boundary = advanceTerminalReplayBoundary(
      INITIAL_TERMINAL_REPLAY_BOUNDARY, encode("\x1b"),
    );
    expect(decodedReplay([chunk("[31mRed", boundary).replay])).toBe("Red");
  });

  it("does not paint an OSC payload whose introducer was dropped", () => {
    const boundary = advanceTerminalReplayBoundary(
      INITIAL_TERMINAL_REPLAY_BOUNDARY, encode("\x1b]0;agent"),
    );
    expect(decodedReplay([chunk(" title\x07prompt", boundary).replay])).toBe("prompt");
  });

  it("preserves bracketed and digit-leading ground-state text byte-for-byte", () => {
    const first = chunk("[200] ordinary status\r\n", INITIAL_TERMINAL_REPLAY_BOUNDARY);
    const second = chunk("2026Q3 migration complete", first.end);
    const third = chunk(";38;2;20m is also prose without prior ESC", second.end);
    expect(decodedReplay([first.replay, second.replay, third.replay]))
      .toBe("[200] ordinary status\r\n2026Q3 migration complete;38;2;20m is also prose without prior ESC");
  });

  it("leaves a complete sequence intact when replay starts in ground state", () => {
    const intact = chunk("\x1b[38;2;87;87;87mkept", INITIAL_TERMINAL_REPLAY_BOUNDARY);
    expect(replayTerminalOutputChunks([intact.replay])).toEqual([intact.replay.bytes]);
  });
});

describe("truncated agent replay", () => {
  it("keeps a shell tail and drops a torn agent TUI frame", () => {
    expect(terminalReplayShouldRestoreChunks(false, "grok")).toBe(true);
    expect(terminalReplayShouldRestoreChunks(true, "shell")).toBe(true);
    expect(terminalReplayShouldRestoreChunks(true, undefined)).toBe(true);
    expect(terminalReplayShouldRestoreChunks(true, "grok")).toBe(false);
    expect(terminalReplayShouldRestoreChunks(true, "claude")).toBe(false);
    expect(terminalReplayShouldRestoreChunks(true, "codex")).toBe(false);
  });

  it("drops a torn frame from a full-screen TUI started inside a shell", () => {
    expect(terminalReplayShouldRestoreChunks(true, "shell", true)).toBe(false);
    expect(terminalReplayShouldRestoreChunks(true, undefined, true)).toBe(false);
    expect(terminalReplayShouldRestoreChunks(false, "shell", true)).toBe(true);
    expect(terminalReplayShouldRestoreChunks(true, "shell", false)).toBe(true);
  });

  it("re-asserts bracketed paste for the CLIs that always enable it", () => {
    // A remounted xterm never saw the program's DECSET 2004; without it, a
    // dropped image path pastes unbracketed and Claude Code / Codex keep it
    // as typed text instead of drawing "[Image #1]".
    expect(BRACKETED_PASTE_ON).toBe("\x1b[?2004h");
    expect(terminalReplayAssertsBracketedPaste("claude")).toBe(true);
    expect(terminalReplayAssertsBracketedPaste("codex")).toBe(true);
    expect(terminalReplayAssertsBracketedPaste("shell")).toBe(false);
    expect(terminalReplayAssertsBracketedPaste("grok")).toBe(false);
    expect(terminalReplayAssertsBracketedPaste(undefined)).toBe(false);
  });

  it("nudges a same-size PTY by one row so the host still SIGWINCHs", () => {
    expect(terminalRedrawKickSize({ columns: 120, rows: 40 }))
      .toEqual({ columns: 120, rows: 39 });
    expect(terminalRedrawKickSize({ columns: 80, rows: 1 }))
      .toEqual({ columns: 79, rows: 1 });
    expect(terminalRedrawKickSize({ columns: 2, rows: 1 })).toBeNull();
  });
});
