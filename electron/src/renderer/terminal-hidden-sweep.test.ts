import { describe, expect, it } from "vitest";
import type { TerminalWorkspaceRecord } from "../shared/ipc";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, serializeTerminalWorkspaceState,
} from "./terminal-workspace";
import {
  HIDDEN_TERMINAL_GRACE_MS, attachedTerminalIds, sweepHiddenTerminals,
} from "./terminal-hidden-sweep";

const NOW = "2026-10-01T00:00:00.000Z";

function record(id: string, terminalIds: readonly string[], shown?: readonly string[]):
    TerminalWorkspaceRecord {
  let layout = EMPTY_TERMINAL_WORKSPACE_STATE;
  for (const terminalId of shown ?? []) layout = openTerminalTab(layout, "pane-a", terminalId);
  return {
    id, name: id, createdAt: NOW, updatedAt: NOW,
    terminals: terminalIds.map(terminalId => ({ id: terminalId, title: "Claude", cwd: "/tmp/p" })),
    ...(shown === undefined ? {} : { layout: serializeTerminalWorkspaceState(layout) }),
  };
}

describe("terminals a pane shows", () => {
  it("joins the open layout with every saved one, but not closed tabs", () => {
    const attached = attachedTerminalIds([
      record("w-open", ["t-open", "t-closed"], ["t-open"]),
      record("w-other", ["t-other", "t-parked"], ["t-other"]),
    ], new Set(["t-open", "t-just-placed"]));
    expect([...attached].sort()).toEqual(["t-just-placed", "t-open", "t-other"]);
  });

  it("counts every terminal of a record saved before layouts existed", () => {
    expect([...attachedTerminalIds([record("w-legacy", ["t-a", "t-b"])], new Set())].sort())
      .toEqual(["t-a", "t-b"]);
  });
});

describe("sweeping hidden terminals", () => {
  it("ends a quiet hidden terminal only once the whole grace has passed", () => {
    const first = sweepHiddenTerminals([{ key: "t:1", busy: false }], new Map(), 1_000);
    expect(first.due).toEqual([]);
    expect(first.nextCheckMs).toBe(HIDDEN_TERMINAL_GRACE_MS);
    const early = sweepHiddenTerminals([{ key: "t:1", busy: false }], first.since,
      1_000 + HIDDEN_TERMINAL_GRACE_MS - 1);
    expect(early.due).toEqual([]);
    expect(early.nextCheckMs).toBe(1);
    const late = sweepHiddenTerminals([{ key: "t:1", busy: false }], early.since,
      1_000 + HIDDEN_TERMINAL_GRACE_MS);
    expect(late.due).toEqual(["t:1"]);
    expect(late.since.size).toBe(0);
    expect(late.nextCheckMs).toBeNull();
  });

  it("starts the grace when a busy terminal goes quiet, not when it was hidden", () => {
    const hidden = sweepHiddenTerminals([{ key: "t:1", busy: false }], new Map(), 0);
    const working = sweepHiddenTerminals([{ key: "t:1", busy: true }], hidden.since,
      HIDDEN_TERMINAL_GRACE_MS * 3);
    expect(working.due).toEqual([]);
    expect(working.nextCheckMs).toBeNull();
    const quiet = sweepHiddenTerminals([{ key: "t:1", busy: false }], working.since,
      HIDDEN_TERMINAL_GRACE_MS * 4);
    expect(quiet.due).toEqual([]);
    expect(quiet.since.get("t:1")).toBe(HIDDEN_TERMINAL_GRACE_MS * 4);
  });

  it("forgets a terminal that a pane shows again, and a restart starts a new clock", () => {
    const hidden = sweepHiddenTerminals([{ key: "t:1", busy: false }], new Map(), 0);
    const placed = sweepHiddenTerminals([], hidden.since, 10);
    expect(placed.since.size).toBe(0);
    const restarted = sweepHiddenTerminals([{ key: "t:2", busy: false }], hidden.since,
      HIDDEN_TERMINAL_GRACE_MS);
    expect(restarted.due).toEqual([]);
  });
});
