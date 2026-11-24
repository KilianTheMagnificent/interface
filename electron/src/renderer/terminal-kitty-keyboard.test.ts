import { describe, expect, it } from "vitest";
import { KittyKeyboardStacks } from "./terminal-kitty-keyboard";

describe("KittyKeyboardStacks", () => {
  it("follows Codex's pushes and pops on each screen", () => {
    const stacks = new KittyKeyboardStacks();
    expect(stacks.readsKeyReports("normal")).toBe(false);
    stacks.apply("normal", ">", [7]);
    stacks.enterAlternate();
    stacks.apply("alternate", ">", [7]);
    expect(stacks.readsKeyReports("alternate")).toBe(true);
    expect(stacks.restoreSequence("alternate")).toBe("\x1b[>7u");
    // xterm reports an omitted count as 0; the protocol's default is one entry.
    stacks.apply("alternate", "<", [0]);
    expect(stacks.readsKeyReports("alternate")).toBe(false);
    expect(stacks.readsKeyReports("normal")).toBe(true);
    stacks.apply("normal", "<", []);
    expect(stacks.readsKeyReports("normal")).toBe(false);
    expect(stacks.restoreSequence("normal")).toBe("");
  });

  it("reads only the disambiguate flag of the top entry", () => {
    const stacks = new KittyKeyboardStacks();
    stacks.apply("normal", ">", [1]);
    stacks.apply("normal", ">", [8]);
    expect(stacks.readsKeyReports("normal")).toBe(false);
    stacks.apply("normal", "<", [1]);
    expect(stacks.readsKeyReports("normal")).toBe(true);
  });

  it("replaces, adds and removes flags on the top entry", () => {
    const stacks = new KittyKeyboardStacks();
    stacks.apply("normal", "=", [1]);
    expect(stacks.restoreSequence("normal")).toBe("\x1b[>1u");
    stacks.apply("normal", "=", [4, 2]);
    expect(stacks.restoreSequence("normal")).toBe("\x1b[>5u");
    stacks.apply("normal", "=", [1, 3]);
    expect(stacks.restoreSequence("normal")).toBe("\x1b[>4u");
    expect(stacks.readsKeyReports("normal")).toBe(false);
    stacks.apply("normal", "=", [0x7fffffff, 0]);
    expect(stacks.restoreSequence("normal")).toBe("\x1b[>31u");
  });

  it("forgets a stale alternate stack, resets on RIS and stays bounded", () => {
    const stacks = new KittyKeyboardStacks();
    stacks.apply("alternate", ">", [1]);
    stacks.enterAlternate();
    expect(stacks.readsKeyReports("alternate")).toBe(false);
    for (let index = 0; index < 100; index++) stacks.apply("normal", ">", [1]);
    expect(stacks.restoreSequence("normal").length).toBeLessThanOrEqual(16 * "\x1b[>1u".length);
    stacks.apply("normal", "<", [1000]);
    expect(stacks.readsKeyReports("normal")).toBe(false);
    stacks.apply("normal", ">", [1]);
    stacks.reset();
    expect(stacks.readsKeyReports("normal")).toBe(false);
    stacks.apply("normal", "?", []);
    expect(stacks.restoreSequence("normal")).toBe("");
  });
});
