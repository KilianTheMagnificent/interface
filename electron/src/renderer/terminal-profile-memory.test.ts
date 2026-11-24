import { describe, expect, it } from "vitest";
import { TERMINAL_LAUNCH_PROFILE_IDS } from "../shared/ipc";
import {
  parseTerminalProfileMemory, pruneTerminalProfileMemory, readTerminalProfileMemory,
  TERMINAL_PROFILE_MEMORY_KEY, writeTerminalProfileMemory,
  type TerminalProfileMemoryStorage,
} from "./terminal-profile-memory";

function memoryStorage(): TerminalProfileMemoryStorage & { values: Map<string, string> } {
  const values = new Map<string, string>();
  return {
    values,
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => { values.set(key, value); },
    removeItem: key => { values.delete(key); },
  };
}

describe("terminal provider icon memory", () => {
  it("round-trips presentation identity without changing launch metadata", () => {
    const storage = memoryStorage();
    writeTerminalProfileMemory(storage, new Map([
      ["terminal-a", "codex"],
      ["terminal-b", "grok"],
      ["terminal-omp", "omp"],
      ["terminal-opencode", "opencode"],
    ]));
    expect([...readTerminalProfileMemory(storage)]).toEqual([
      ["terminal-a", "codex"],
      ["terminal-b", "grok"],
      ["terminal-omp", "omp"],
      ["terminal-opencode", "opencode"],
    ]);
    writeTerminalProfileMemory(storage, new Map());
    expect(storage.values.has(TERMINAL_PROFILE_MEMORY_KEY)).toBe(false);
  });

  it("remembers a mark for every launch profile the host defines", () => {
    const storage = memoryStorage();
    const marks = new Map(TERMINAL_LAUNCH_PROFILE_IDS.map(id => [`terminal-${id}`, id] as const));
    writeTerminalProfileMemory(storage, marks);
    expect(readTerminalProfileMemory(storage)).toEqual(marks);
  });

  it("fails closed on malformed, foreign, duplicate, or unbounded data", () => {
    expect(parseTerminalProfileMemory("not-json").size).toBe(0);
    expect(parseTerminalProfileMemory(JSON.stringify({ version: 2, entries: [] })).size).toBe(0);
    expect(parseTerminalProfileMemory(JSON.stringify({
      version: 1, entries: [["terminal-a", "unknown"]],
    })).size).toBe(0);
    expect(parseTerminalProfileMemory(JSON.stringify({
      version: 1, entries: [["terminal-a", "codex"], ["terminal-a", "grok"]],
    })).size).toBe(0);
    expect(parseTerminalProfileMemory(" ".repeat(64 * 1024 + 1)).size).toBe(0);
  });

  it("keeps live memory usable when browser storage is unavailable", () => {
    const blocked: TerminalProfileMemoryStorage = {
      getItem: () => { throw new Error("blocked"); },
      setItem: () => { throw new Error("blocked"); },
      removeItem: () => { throw new Error("blocked"); },
    };
    expect(readTerminalProfileMemory(blocked).size).toBe(0);
    expect(() => {
      writeTerminalProfileMemory(blocked, new Map([["terminal-a", "claude"]]));
    }).not.toThrow();
  });

  it("retains saved offline terminals through cold-start ordering and prunes deletion", () => {
    const restored = parseTerminalProfileMemory(JSON.stringify({
      version: 1, entries: [["offline-terminal", "codex"]],
    }));
    const whileRegistryIsEmpty = pruneTerminalProfileMemory(
      restored, new Set(["offline-terminal"]),
    );
    expect(whileRegistryIsEmpty).toBe(restored);
    expect([...whileRegistryIsEmpty]).toEqual([["offline-terminal", "codex"]]);
    expect(pruneTerminalProfileMemory(whileRegistryIsEmpty, new Set()).size).toBe(0);
  });
});
