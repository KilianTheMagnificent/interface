import { describe, expect, it } from "vitest";
import { equalValue, retainEqualItems } from "./stable-values";

describe("retainEqualItems", () => {
  const key = (item: { id: string }): string => item.id;

  it("returns the previous list when every rebuilt item equals the one before it", () => {
    const previous = [{ id: "a", dot: "idle" }, { id: "b", dot: "idle" }];
    const rebuilt = previous.map(item => ({ ...item }));
    expect(retainEqualItems(previous, rebuilt, key)).toBe(previous);
  });

  it("hands on unchanged items and only the changed one anew", () => {
    const previous = [{ id: "a", dot: "idle" }, { id: "b", dot: "idle" }];
    const changed = { id: "b", dot: "working" };
    const next = retainEqualItems(previous, [{ id: "a", dot: "idle" }, changed], key);
    expect(next).not.toBe(previous);
    expect(next[0]).toBe(previous[0]);
    expect(next[1]).toBe(changed);
  });

  it("keeps items across a reorder, an addition and a removal without keeping the list", () => {
    const previous = [{ id: "a" }, { id: "b" }];
    const reordered = retainEqualItems(previous, [{ id: "b" }, { id: "a" }], key);
    expect(reordered).not.toBe(previous);
    expect(reordered[0]).toBe(previous[1]);
    expect(reordered[1]).toBe(previous[0]);
    const added = retainEqualItems(previous, [{ id: "a" }, { id: "b" }, { id: "c" }], key);
    expect(added.slice(0, 2)).toEqual(previous);
    expect(added[0]).toBe(previous[0]);
    expect(retainEqualItems(previous, [{ id: "a" }], key)).toEqual([previous[0]]);
  });

  it("compares by value, so a function member must itself be stable", () => {
    const load = (): void => undefined;
    const previous = [{ id: "a", load }];
    expect(retainEqualItems(previous, [{ id: "a", load }], key)).toBe(previous);
    expect(retainEqualItems(previous, [{ id: "a", load: () => undefined }], key)).not.toBe(previous);
    expect(equalValue({ preview: ["x"] }, { preview: ["x"] })).toBe(true);
  });
});
