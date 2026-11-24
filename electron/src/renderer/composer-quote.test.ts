import { describe, expect, it } from "vitest";
import {
  appendReplyQuote, appendTerminalExcerpt, REPLY_QUOTE_MAX_CHARS, replyQuoteBlock,
  TERMINAL_EXCERPT_MAX_CHARS, terminalExcerptBlock,
} from "./composer-quote";

describe("reply quotes", () => {
  it("quotes each line, keeps blank lines as >, and ends with an empty line", () => {
    expect(replyQuoteBlock("Added login().\r\n\r\nIt trims the name.")).toBe(
      "> Added login().\n>\n> It trims the name.\n\n");
    expect(replyQuoteBlock("  \n\t")).toBeNull();
  });

  it("goes after the typed draft, one blank line apart, with the caret after it", () => {
    const first = appendReplyQuote("", "Added login().");
    expect(first).toEqual({ text: "> Added login().\n\n", cursor: 18 });
    const second = appendReplyQuote(`${first!.text}Why this?  \n`, "It trims the name.");
    expect(second!.text).toBe("> Added login().\n\nWhy this?\n\n> It trims the name.\n\n");
    expect(second!.cursor).toBe(second!.text.length);
    // Several quotes in a row stack, each after the last.
    const third = appendReplyQuote(second!.text, "logout()");
    expect(third!.text.endsWith("> It trims the name.\n\n> logout()\n\n")).toBe(true);
  });

  it("is plain text: markup is quoted as typed, never interpreted", () => {
    expect(replyQuoteBlock("<img src=x onerror=alert(1)> **bold**"))
      .toBe("> <img src=x onerror=alert(1)> **bold**\n\n");
  });

  it("cuts a selection past 4,000 characters with …", () => {
    const block = replyQuoteBlock("a".repeat(REPLY_QUOTE_MAX_CHARS + 50))!;
    expect(block).toBe(`> ${"a".repeat(REPLY_QUOTE_MAX_CHARS)}…\n\n`);
    // Counted in characters, so an emoji is never split.
    const emoji = replyQuoteBlock("😀".repeat(REPLY_QUOTE_MAX_CHARS + 1))!;
    expect([...emoji.slice(2, -3)]).toHaveLength(REPLY_QUOTE_MAX_CHARS);
  });
});

describe("terminal excerpts", () => {
  it("fences the selection under the pane's title", () => {
    expect(terminalExcerptBlock("npm test", "FAIL src/a.test.ts\n  expected 1")).toBe(
      "From the terminal \"npm test\":\n```\nFAIL src/a.test.ts\n  expected 1\n```\n\n");
    expect(terminalExcerptBlock("x", "   ")).toBeNull();
    // A title cannot break the heading's quotes or lines.
    expect(terminalExcerptBlock("a \"b\"\nc", "x")!.startsWith("From the terminal \"a b c\":\n"))
      .toBe(true);
    expect(terminalExcerptBlock("", "x")!.startsWith("From the terminal \"Terminal\":")).toBe(true);
  });

  it("uses a fence longer than any backtick run in the output", () => {
    const block = terminalExcerptBlock("sh", "``` not the end\n````")!;
    expect(block).toBe("From the terminal \"sh\":\n`````\n``` not the end\n````\n`````\n\n");
  });

  it("cuts from the top past 8,000 characters so the error at the end survives", () => {
    const lines = Array.from({ length: 2000 }, (_, index) => `line ${index}`);
    lines.push("Error: the last line");
    const block = terminalExcerptBlock("build", lines.join("\n"))!;
    const body = block.split("\n```\n")[1]!.split("\n```")[0]!;
    expect(body.startsWith("…\n")).toBe(true);
    expect(body.endsWith("Error: the last line")).toBe(true);
    expect([...body].length).toBeLessThanOrEqual(TERMINAL_EXCERPT_MAX_CHARS + 2);
    // It restarts on a whole line.
    expect(body.split("\n")[1]).toMatch(/^line \d+$/u);
  });

  it("goes after the typed draft with the caret after it", () => {
    const write = appendTerminalExcerpt("Look at this:", "zsh", "exit 1");
    expect(write).toEqual({
      text: "Look at this:\n\nFrom the terminal \"zsh\":\n```\nexit 1\n```\n\n",
      cursor: "Look at this:\n\nFrom the terminal \"zsh\":\n```\nexit 1\n```\n\n".length,
    });
  });
});
