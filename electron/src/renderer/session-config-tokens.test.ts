import { describe, expect, it } from "vitest";
import { CONFIG_FIELDS, ConfigAttempts, configToken } from "./session-config-tokens";

const A = "claude:a";
const B = "codex:b";

function tokens() {
  let changes = 0;
  const attempts = new ConfigAttempts(() => { changes += 1; });
  return { attempts, changes: () => changes };
}

describe("ConfigAttempts", () => {
  it("locks sends from the first write until that write settles", () => {
    const { attempts, changes } = tokens();
    expect(attempts.pending(A)).toBe(false);
    const write = attempts.begin(A, "effort");
    expect(write.attempt).toBe(1);
    expect(changes()).toBe(1);
    expect(attempts.pending(A)).toBe(true);
    expect(attempts.inFlightFor(A, "effort")).toBe(true);
    expect(attempts.inFlightFor(A, "model")).toBe(false);
    expect(attempts.pending(B)).toBe(false);
    expect(write.isCurrent()).toBe(true);
    expect(write.settle()).toBe(true);
    expect(changes()).toBe(2);
    expect(attempts.pending(A)).toBe(false);
    // A second settle of the same write is a no-op.
    expect(write.settle()).toBe(false);
    expect(changes()).toBe(2);
  });

  it("lets a newer write of the same field supersede an older one, never the reverse", () => {
    const { attempts } = tokens();
    const older = attempts.begin(A, "model");
    const newer = attempts.begin(A, "model");
    expect(newer.attempt).toBe(2);
    expect(attempts.current(A, "model")).toBe(2);
    // The older answer arriving late is stale and cannot release the newer lock.
    expect(older.isCurrent()).toBe(false);
    expect(older.settle()).toBe(false);
    expect(attempts.pending(A)).toBe(true);
    expect(newer.isCurrent()).toBe(true);
    expect(newer.settle()).toBe(true);
    expect(attempts.pending(A)).toBe(false);
  });

  it("keeps fields independent", () => {
    const { attempts } = tokens();
    const model = attempts.begin(A, "model");
    const effort = attempts.begin(A, "effort");
    expect(model.settle()).toBe(true);
    expect(attempts.pending(A)).toBe(true);
    expect(effort.isCurrent()).toBe(true);
    expect(effort.settle()).toBe(true);
    expect(attempts.pending(A)).toBe(false);
  });

  it("invalidates every field when the session closes", () => {
    const { attempts, changes } = tokens();
    const writes = CONFIG_FIELDS.map(field => attempts.begin(A, field));
    const other = attempts.begin(B, "permission");
    const before = changes();
    attempts.invalidate(A);
    expect(changes()).toBe(before + 1);
    expect(attempts.pending(A)).toBe(false);
    for (const write of writes) {
      expect(write.isCurrent()).toBe(false);
      expect(write.settle()).toBe(false);
    }
    // The next run's first write starts past the invalidated attempts.
    expect(attempts.begin(A, "model").attempt).toBe(3);
    expect(attempts.current(A, "fastmode")).toBe(2);
    expect(other.isCurrent()).toBe(true);
    expect(attempts.pending(B)).toBe(true);
  });

  it("names tokens the way App's command-refresh map keys them", () => {
    expect(configToken(A, "model")).toBe(`${A}\0model`);
  });
});
