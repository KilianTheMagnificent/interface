import { describe, expect, it } from "vitest";
import type { ConvItem } from "../conversation-reducer";
import { THREAD_GLYPH_NAMES } from "./ThreadGlyph";
import { threadPathParts, threadStepGlyph, threadToolGlyph } from "./thread-look";

/* ==========================================================================
   THE THREADS LOOK (owner, 2026-10-23): each step on the live turn's rail
   wears its tool's glyph, read the way the step's verb is read, and a changed
   file's directory is split from its name so the row can dim it.
   ========================================================================== */

describe("a step's glyph on the rail", () => {
  it("reads each kind of work by its tool's name, every provider's spelling", () => {
    const cases: [string, string][] = [
      ["Read", "read"], ["read_file", "read"],
      ["Edit", "edit"], ["MultiEdit", "edit"], ["NotebookEdit", "edit"], ["str_replace", "edit"],
      ["fileChange", "edit"], ["apply_patch", "edit"],
      ["Write", "write"], ["write_file", "write"],
      ["Bash", "run"], ["shell", "run"], ["commandExecution", "run"], ["exec_command", "run"],
      ["Grep", "search"], ["Glob", "search"], ["list_dir", "search"],
      ["WebFetch", "web"], ["WebSearch", "web"], ["webSearch", "web"],
      ["Task", "agent"], ["Agent", "agent"],
      ["TodoWrite", "plan"], ["update_plan", "plan"], ["ExitPlanMode", "plan"],
    ];
    for (const [tool, glyph] of cases) expect(threadToolGlyph(tool), tool).toBe(glyph);
  });

  it("falls back to the generic tool for a name it was never taught", () => {
    for (const tool of ["mcp__github__create_issue", "github/create_issue", "Skill", "", "  "]) {
      expect(threadToolGlyph(tool), tool).toBe("tool");
    }
  });

  it("draws an approval as the call it asks for, a subagent as an agent, a lone change as an edit", () => {
    const items: [ConvItem, string][] = [
      [{ kind: "tool", id: "t", tool: "Bash", summary: "npm test", input: { command: "npm test" } }, "run"],
      [{ kind: "approval", id: "a", tool: "Write", summary: "/w/a.ts", input: {} }, "write"],
      [{ kind: "subagent", id: "s", subagentType: "Explore", description: "Find it", status: "running",
        children: [], toolCount: 0, subagentCount: 0 }, "agent"],
      [{ kind: "diff", id: "d", path: "/w/a.ts", callId: "gone", added: 1, removed: 0 }, "edit"],
      [{ kind: "text", id: "x", text: "Hello", streaming: false }, "tool"],
    ];
    for (const [item, glyph] of items) expect(threadStepGlyph(item), item.kind).toBe(glyph);
  });

  it("only ever names a glyph the set draws", () => {
    const names = new Set<string>(THREAD_GLYPH_NAMES);
    for (const tool of ["Read", "Edit", "Write", "Bash", "Grep", "WebFetch", "Task", "TodoWrite", "x"]) {
      expect(names.has(threadToolGlyph(tool)), tool).toBe(true);
    }
  });
});

describe("a changed file's path, split for the row", () => {
  it("keeps the separator with the directory and the name whole", () => {
    expect(threadPathParts("src/auth/retry.ts")).toEqual({ dir: "src/auth/", name: "retry.ts" });
    expect(threadPathParts("crates\\host\\mod.rs")).toEqual({ dir: "crates\\host\\", name: "mod.rs" });
    expect(threadPathParts("/abs/a.ts")).toEqual({ dir: "/abs/", name: "a.ts" });
  });

  it("gives a bare name no directory, and always joins back to the path", () => {
    expect(threadPathParts("README.md")).toEqual({ dir: "", name: "README.md" });
    expect(threadPathParts("")).toEqual({ dir: "", name: "" });
    for (const shown of ["a/b/c.ts", "c.ts", "dir/", "a/b.ts/", "~/x/y", "/"]) {
      const { dir, name } = threadPathParts(shown);
      expect(dir + name, shown).toBe(shown);
    }
  });
});
