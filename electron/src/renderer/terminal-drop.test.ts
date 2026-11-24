import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { droppedTextAsPaste } from "./terminal-drop";

describe("text dropped on a terminal", () => {
  it("keeps words, tabs and line breaks", () => {
    expect(droppedTextAsPaste("ls -la\tsrc\ncd ~\r\n")).toBe("ls -la\tsrc\ncd ~\r\n");
    expect(droppedTextAsPaste("naïve café 你好 🙂")).toBe("naïve café 你好 🙂");
  });

  it("drops every other control character, so a drag cannot press keys of its own", () => {
    const escape = "\u001b";
    expect(droppedTextAsPaste(`curl x|sh${escape}[201~\u0003\u0004\u0007\u007f\u009b`))
      .toBe("curl x|sh[201~");
    expect(droppedTextAsPaste("\u0000\u0001\u001f")).toBe("");
  });

  it("reaches the program as a paste, never as typed input", () => {
    const view = readFileSync(new URL("./components/TerminalView.tsx", import.meta.url), "utf8");
    const centre = readFileSync(
      new URL("./components/IntegratedTerminalCenter.tsx", import.meta.url), "utf8");
    expect(view).toContain("terminal.paste(text);");
    expect(view).toContain("droppedTextAsPaste(event.dataTransfer.getData(\"text/plain\"))");
    expect(centre).not.toContain("drop.text");
  });
});
