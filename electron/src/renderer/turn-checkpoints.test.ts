import { afterEach, describe, expect, it, vi } from "vitest";
import {
  LEGACY_TURN_CHECKPOINTS_PREFIX, TURN_CHECKPOINTS_PER_SESSION, checkpointIdFor,
  forgetTurnCheckpoints, nextTurnCheckpoints, recordTurnCheckpoint, rekeyTurnCheckpoint,
  sweepLegacyTurnCheckpoints, turnRestorePointsFor,
  type TurnCheckpoint,
} from "./turn-checkpoints";

/* ==========================================================================
   THE MAP BETWEEN A USER TURN AND THE CHECKPOINT TAKEN JUST BEFORE IT.
   The transcript draws a control exactly where this map has an entry, so
   every rule here is a rule about what may be OFFERED: a wrong entry is a
   rollback to the wrong moment, and a missing entry is only a missing button.
   ========================================================================== */

const SESSION = "claude#s1";

const entry = (over: Partial<TurnCheckpoint> = {}): TurnCheckpoint => ({
  turnKey: "e:0", checkpointId: "cp-0", fileCount: 3, at: 1_000, ...over,
});

afterEach(() => {
  forgetTurnCheckpoints(SESSION);
  forgetTurnCheckpoints("codex#other");
});

describe("nextTurnCheckpoints", () => {
  it("appends the new turn", () => {
    const next = nextTurnCheckpoints([entry()], entry({ turnKey: "e:1", checkpointId: "cp-1" }));
    expect(next.map(item => item.checkpointId)).toEqual(["cp-0", "cp-1"]);
  });

  it("replaces the entry for a turn key it already holds", () => {
    // One row can only stand before one checkpoint. A second record for the
    // same key is a re-mint of that row, never a second offer on it.
    const next = nextTurnCheckpoints([entry()], entry({ checkpointId: "cp-again", fileCount: 9 }));
    expect(next).toHaveLength(1);
    expect(next[0]).toMatchObject({ checkpointId: "cp-again", fileCount: 9 });
  });

  it("drops the oldest once the session is at its ceiling", () => {
    const full = Array.from({ length: TURN_CHECKPOINTS_PER_SESSION }, (_, i) =>
      entry({ turnKey: `e:${i}`, checkpointId: `cp-${i}` }));
    const next = nextTurnCheckpoints(full, entry({ turnKey: "e:new", checkpointId: "cp-new" }));
    expect(next).toHaveLength(TURN_CHECKPOINTS_PER_SESSION);
    expect(next[0].turnKey).toBe("e:1");
    expect(next[next.length - 1].turnKey).toBe("e:new");
  });
});

describe("recordTurnCheckpoint / turnRestorePointsFor / checkpointIdFor", () => {
  it("records one turn and offers it back", () => {
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 14);
    const points = turnRestorePointsFor(SESSION);
    expect([...points.keys()]).toEqual(["e:0"]);
    expect(points.get("e:0")?.label).toBe("Undo · 14 files");
    // Shown verbatim in the confirm dialog, so it may only say what is true:
    // the count is the checkpoint's own, and the loss is stated.
    expect(points.get("e:0")?.description).toContain("14 files");
    expect(points.get("e:0")?.description).toContain("lost");
    expect(checkpointIdFor(SESSION, "e:0")).toBe("cp-0");
  });

  it("says one file rather than 1 files", () => {
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 1);
    expect(turnRestorePointsFor(SESSION).get("e:0")?.label).toBe("Undo · 1 file");
  });

  it("records nothing for a checkpoint that captured no files", () => {
    // The pane's own rule: no entry, no control. A checkpoint with nothing in
    // it would offer a rollback that changes nothing and says it did.
    recordTurnCheckpoint(SESSION, "e:0", "cp-empty", 0);
    expect(turnRestorePointsFor(SESSION).size).toBe(0);
    expect(checkpointIdFor(SESSION, "e:0")).toBeNull();
  });

  it("records nothing it could not restore", () => {
    recordTurnCheckpoint(SESSION, "", "cp-0", 2);
    recordTurnCheckpoint(SESSION, "e:1", "", 2);
    recordTurnCheckpoint(SESSION, "e:2", "c".repeat(129), 2);
    recordTurnCheckpoint(SESSION, "e:3", "cp-3", 1.5);
    expect(turnRestorePointsFor(SESSION).size).toBe(0);
  });

  it("keeps only the newest turns of one session", () => {
    for (let i = 0; i <= TURN_CHECKPOINTS_PER_SESSION; i += 1) {
      recordTurnCheckpoint(SESSION, `e:${i}`, `cp-${i}`, 2);
    }
    const points = turnRestorePointsFor(SESSION);
    expect(points.size).toBe(TURN_CHECKPOINTS_PER_SESSION);
    expect(checkpointIdFor(SESSION, "e:0")).toBeNull();
    expect(checkpointIdFor(SESSION, `e:${TURN_CHECKPOINTS_PER_SESSION}`))
      .toBe(`cp-${TURN_CHECKPOINTS_PER_SESSION}`);
  });

  it("is scoped per session", () => {
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 2);
    expect(turnRestorePointsFor("codex#other").size).toBe(0);
    expect(checkpointIdFor("codex#other", "e:0")).toBeNull();
  });

  it("starts empty in every new run of the renderer", async () => {
    // THE SAFETY PROPERTY. Row keys are mergeTranscript's, and the key for a
    // just-sent message is its local echo (`e:0`) — an id minted per run and
    // restarted at 0 by the next one. An entry kept from an earlier run names
    // a row that no longer exists, and would attach to the NEXT run's first
    // send: a rollback to the wrong moment, which is worse than no button.
    recordTurnCheckpoint(SESSION, "e:0", "cp-yesterday", 3);
    vi.resetModules();
    const nextRun = await import("./turn-checkpoints");
    expect(nextRun.turnRestorePointsFor(SESSION).size).toBe(0);
    expect(nextRun.checkpointIdFor(SESSION, "e:0")).toBeNull();
  });

  it("follows a turn across the row key it settles into", () => {
    // The echo row an entry was recorded against becomes a live row the moment
    // the engine streams the user's own turn back (every ACP engine does).
    // Without this the entry names a row that no longer exists and the control
    // vanishes a second after the send.
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 3);
    rekeyTurnCheckpoint(SESSION, "e:0", "l:user-3");
    expect([...turnRestorePointsFor(SESSION).keys()]).toEqual(["l:user-3"]);
    expect(checkpointIdFor(SESSION, "l:user-3")).toBe("cp-0");
    expect(checkpointIdFor(SESSION, "e:0")).toBeNull();
  });

  it("keeps the rest of the session's turns where they were", () => {
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 3);
    recordTurnCheckpoint(SESSION, "e:1", "cp-1", 4);
    rekeyTurnCheckpoint(SESSION, "e:0", "l:user-3");
    expect([...turnRestorePointsFor(SESSION).keys()]).toEqual(["l:user-3", "e:1"]);
    expect(checkpointIdFor(SESSION, "e:1")).toBe("cp-1");
  });

  it("refuses a re-key it cannot make honestly", () => {
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 3);
    rekeyTurnCheckpoint(SESSION, "e:missing", "l:user-3");   // nothing to move
    rekeyTurnCheckpoint(SESSION, "e:0", "");                 // no key to move to
    rekeyTurnCheckpoint("codex#other", "e:0", "l:user-3");   // another session's turn
    expect([...turnRestorePointsFor(SESSION).keys()]).toEqual(["e:0"]);
    expect(turnRestorePointsFor("codex#other").size).toBe(0);
  });

  it("does not leave two turns pointed at one row", () => {
    // The loser has to be the entry being moved ONTO, not a silent second control.
    recordTurnCheckpoint(SESSION, "l:user-3", "cp-stale", 9);
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 3);
    rekeyTurnCheckpoint(SESSION, "e:0", "l:user-3");
    expect([...turnRestorePointsFor(SESSION).keys()]).toEqual(["l:user-3"]);
    expect(checkpointIdFor(SESSION, "l:user-3")).toBe("cp-0");
  });

  it("forgets a session's turns", () => {
    // The session was deleted. Every row key these entries name went with it.
    recordTurnCheckpoint(SESSION, "e:0", "cp-0", 3);
    forgetTurnCheckpoints(SESSION);
    expect(turnRestorePointsFor(SESSION).size).toBe(0);
    expect(checkpointIdFor(SESSION, "e:0")).toBeNull();
  });
});

describe("sweepLegacyTurnCheckpoints", () => {
  it("removes every key an earlier build wrote, and nothing else", () => {
    // Nothing reads them any more, and the origin quota they sit in is shared
    // with the steering queue and the workspace layout.
    const cell: Record<string, string> = {
      [`${LEGACY_TURN_CHECKPOINTS_PREFIX}claude#s1`]: "[]",
      [`${LEGACY_TURN_CHECKPOINTS_PREFIX}codex#s2`]: "[]",
      "iface.steeringQueue.claude#s1": "[]",
    };
    const storage = {
      get length() { return Object.keys(cell).length; },
      key: (index: number) => Object.keys(cell)[index] ?? null,
      removeItem: (key: string) => { delete cell[key]; },
    };
    sweepLegacyTurnCheckpoints(storage);
    expect(Object.keys(cell)).toEqual(["iface.steeringQueue.claude#s1"]);
  });
});
