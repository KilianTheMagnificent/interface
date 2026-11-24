import { describe, expect, it } from "vitest";
import { createKeepAwake, type PowerSaveBlockerPort } from "./keep-awake";
import { NO_WORK_IN_FLIGHT, type WorkInFlightV1 } from "../shared/quit-guard";

/* Review D3 (2026-10-14): nothing kept the Mac awake while agents worked. */
describe("keeping the Mac awake while agents work", () => {
  const fake = () => {
    const calls: string[] = [];
    let next = 1;
    const live = new Set<number>();
    const port: PowerSaveBlockerPort = {
      start(type) { const id = next++; live.add(id); calls.push(`start:${type}:${id}`); return id; },
      stop(id) { live.delete(id); calls.push(`stop:${id}`); },
    };
    return { calls, live, port };
  };
  const work = (over: Partial<WorkInFlightV1>): WorkInFlightV1 => ({ ...NO_WORK_IN_FLIGHT, ...over });

  it("holds one app-suspension blocker from the first working report and releases it when idle", () => {
    const { calls, live, port } = fake();
    const keep = createKeepAwake(port);
    keep.update(work({ terminals: 1 }));
    keep.update(work({ terminals: 2, away: 1 }));
    keep.update(work({ threads: 1, runs: 1 }));
    expect(calls).toEqual(["start:prevent-app-suspension:1"]);
    expect(keep.held()).toBe(true);
    keep.update(NO_WORK_IN_FLIGHT);
    expect(calls).toEqual(["start:prevent-app-suspension:1", "stop:1"]);
    expect(live.size).toBe(0);
    keep.update(NO_WORK_IN_FLIGHT);
    expect(calls).toHaveLength(2);
    keep.update(work({ away: 1 }));
    expect(calls.at(-1)).toBe("start:prevent-app-suspension:2");
  });

  it("lets the Mac sleep when everything is only waiting for an answer", () => {
    const { calls, port } = fake();
    const keep = createKeepAwake(port);
    keep.update(work({ terminals: 1, threads: 1, waiting: 2 }));
    expect(calls).toEqual([]);
    keep.update(work({ terminals: 2, waiting: 1 }));
    expect(keep.held()).toBe(true);
    keep.update(work({ terminals: 2, waiting: 2 }));
    expect(keep.held()).toBe(false);
  });

  it("releases when the report goes away and for good on quit", () => {
    const { calls, live, port } = fake();
    const keep = createKeepAwake(port);
    keep.update(work({ terminals: 1 }));
    keep.update(null);
    expect(live.size).toBe(0);
    keep.update(work({ terminals: 1 }));
    keep.release();
    expect(live.size).toBe(0);
    keep.update(work({ terminals: 1 }));
    expect(keep.held()).toBe(false);
    expect(calls).toEqual(["start:prevent-app-suspension:1", "stop:1",
      "start:prevent-app-suspension:2", "stop:2"]);
  });
});
