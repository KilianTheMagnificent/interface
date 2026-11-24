import { describe, expect, it } from "vitest";
import type { ThreadsView } from "../shared/threads";
import { newerThreadsView } from "./use-threads";

const view = (revision: number, now = 1, autoSettleDays = 3): ThreadsView => ({
  version: 1, revision, now,
  settings: { autoSettleDays, settleOnMerge: true, generatedTitles: false, trackPullRequests: true },
  threads: [],
});

describe("newerThreadsView", () => {
  it("takes the first view and any newer revision", () => {
    const first = view(1);
    expect(newerThreadsView(null, first)).toBe(first);
    const second = view(2);
    expect(newerThreadsView(first, second)).toBe(second);
  });

  it("never lets a late, older answer replace a newer view", () => {
    const newer = view(5);
    expect(newerThreadsView(newer, view(4))).toBe(newer);
  });

  it("keeps the held view's identity when only the clock moved", () => {
    const held = view(3, 100);
    expect(newerThreadsView(held, view(3, 200))).toBe(held);
  });

  it("accepts an equal revision that carries different state", () => {
    const held = view(3, 100, 3);
    const changed = view(3, 100, 7);
    expect(newerThreadsView(held, changed)).toBe(changed);
  });

  it("keeps the hidden sessions' identity while a newer view names the same ones", () => {
    const held = { ...view(3), hiddenKeys: ["codex#worker"] };
    const newer = newerThreadsView(held, { ...view(4), hiddenKeys: ["codex#worker"] });
    expect(newer?.revision).toBe(4);
    expect(newer?.hiddenKeys).toBe(held.hiddenKeys);
    const grown = { ...view(5), hiddenKeys: ["codex#old", "codex#worker"] };
    expect(newerThreadsView(newer, grown)).toBe(grown);
    expect(newerThreadsView(grown, view(6))?.hiddenKeys).toBeUndefined();
  });
});
