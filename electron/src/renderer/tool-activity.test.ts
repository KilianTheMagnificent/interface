import { describe, expect, it } from "vitest";
import { toolGlyph } from "./tool-activity";

describe("toolGlyph", () => {
  it("names the tools it can name honestly and no others", () => {
    expect(toolGlyph("Bash")).toBe("terminal");
    expect(toolGlyph("commandExecution")).toBe("terminal");
    expect(toolGlyph("Read")).toBe("file");
    expect(toolGlyph("list_dir")).toBe("file");
    expect(toolGlyph("apply_patch")).toBe("git-compare");
    expect(toolGlyph("fileChange")).toBe("git-compare");
    // An MCP call or an unknown tool gets no glyph rather than a guess.
    expect(toolGlyph("node_repl/js")).toBeNull();
    expect(toolGlyph("WebFetch")).toBeNull();
  });
});
