import { describe, expect, it } from "vitest";
import {
  MAX_UPDATE_ERROR_LENGTH,
  MAX_UPDATE_VERSION_LENGTH,
  UPDATE_PHASES,
  UPDATE_SOURCES,
  failedUpdateState,
  initialUpdateState,
  isUpdateState,
  type UpdateState,
} from "./updater";

describe("initialUpdateState", () => {
  it("creates a bounded, serializable disabled snapshot", () => {
    const state = initialUpdateState("1.2.3");
    expect(state).toEqual({
      phase: "disabled",
      source: "none",
      currentVersion: "1.2.3",
      availableVersion: null,
      progress: null,
      lastCheckedAt: null,
      error: null,
      canCheck: false,
      canRestart: false,
    });
    expect(isUpdateState(JSON.parse(JSON.stringify(state)))).toBe(true);
  });

  it("replaces empty or overlong current versions with a bounded fallback", () => {
    expect(initialUpdateState("").currentVersion).toBe("unknown");
    expect(initialUpdateState("v".repeat(MAX_UPDATE_VERSION_LENGTH + 1)).currentVersion)
      .toBe("unknown");
  });
});

describe("isUpdateState", () => {
  const ready = (): UpdateState => ({
    ...initialUpdateState("1.0.0"),
    phase: "ready",
    source: "remote",
    availableVersion: "1.1.0",
    progress: 100,
    lastCheckedAt: "2026-08-14T00:00:00.000Z",
    canRestart: true,
  });

  it("accepts every declared phase and source", () => {
    for (const phase of UPDATE_PHASES) {
      for (const source of UPDATE_SOURCES) {
        expect(isUpdateState({
          ...ready(),
          phase,
          source,
          canRestart: phase === "ready",
        })).toBe(true);
      }
    }
  });

  it("accepts inclusive progress and text boundaries", () => {
    expect(isUpdateState({
      ...ready(),
      currentVersion: "v".repeat(MAX_UPDATE_VERSION_LENGTH),
      error: "e".repeat(MAX_UPDATE_ERROR_LENGTH),
      progress: 0,
    })).toBe(true);
    expect(isUpdateState({ ...ready(), progress: 100 })).toBe(true);
  });

  it.each([
    null,
    [],
    { ...ready(), phase: "installing" },
    { ...ready(), source: "file" },
    { ...ready(), currentVersion: "" },
    { ...ready(), availableVersion: "v".repeat(MAX_UPDATE_VERSION_LENGTH + 1) },
    { ...ready(), progress: -1 },
    { ...ready(), progress: 101 },
    { ...ready(), progress: Number.NaN },
    { ...ready(), lastCheckedAt: "yesterday" },
    { ...ready(), error: "e".repeat(MAX_UPDATE_ERROR_LENGTH + 1) },
    { ...ready(), canCheck: "yes" },
    { ...ready(), canRestart: false },
    { ...ready(), phase: "idle", canRestart: true },
  ])("rejects malformed or inconsistent snapshots", (value) => {
    expect(isUpdateState(value)).toBe(false);
  });
});

describe("failedUpdateState", () => {
  it("invalidates stale ready actions without copying native error data", () => {
    const state = failedUpdateState({
      ...initialUpdateState("1.0.0"),
      phase: "ready",
      source: "local",
      availableVersion: "1.1.0",
      progress: 100,
      canRestart: true,
    }, " Interface received an invalid update status. ");
    expect(state).toMatchObject({
      phase: "error",
      currentVersion: "1.0.0",
      source: "local",
      availableVersion: null,
      progress: null,
      error: "Interface received an invalid update status.",
      canCheck: false,
      canRestart: false,
    });
    expect(isUpdateState(state)).toBe(true);
  });
});
