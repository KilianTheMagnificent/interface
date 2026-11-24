import { describe, expect, it } from "vitest";
import {
  parseMarkdown, parseMarkdownIncremental, splitMarkdownTableRow, type MarkdownParse,
} from "./markdown";

// The incremental parse must be parseMarkdown with a cache in front of it, for
// any text and any previous parse. These are differential tests: every prefix
// of every document, grown one character at a time, in random chunks, and
// through edits that do not append at all, against a fresh full parse.

const DOCUMENTS: readonly string[] = [
  [
    "# Result",
    "",
    "The pane spacing is consistent. Code remains **distinct** from prose:",
    "",
    "```ts",
    "const pane = conversations.get(sessionId);",
    "",
    "if (pane === undefined) return;",
    "```",
    "",
    "- first",
    "- second with `code | pipe`",
    "  continued under the item",
    "- third",
    "",
    "3. ordered",
    "4. next",
    "",
    "> A quoted note",
    ">",
    "> with a second paragraph",
    "",
    "---",
    "",
    "Done.",
  ].join("\n"),
  [
    "Here is the comparison:",
    "| Area | State | Score |",
    "| :--- | :---: | ---: |",
    "| Chat | **Ready** | 9 |",
    "| Review | one \\| two | 7 |",
    "",
    "| Lonely | header |",
    "| --- | --- |",
    "",
    "After the table, a paragraph",
    "that continues | with a pipe",
    "| x | y |",
    "| - | - |",
    "| 1 | 2 |",
  ].join("\n"),
  [
    "## Heading | with a pipe",
    "| --- | --- |",
    "| a | b |",
    "",
    "~~~~",
    "```",
    "not a close",
    "",
    "~~~~",
    "",
    "::code-comment{title=\"[P1] Guard this\" body=\"This can **throw**.\\n\\n- a\\n- b\" file=\"/repo/a.ts\" start=3 end=9 priority=1}",
    "",
    "Text :codex-annotation{index=\"4\"} [link](https://example.com) <https://example.com/x>",
    "   ",
    "\t",
    "> > nested",
    "> quote",
    "lazy line",
    "",
    "***",
    "___",
    "- a",
    "* b",
    "+ c",
    "1) paren",
  ].join("\n"),
  [
    "Unclosed fence follows.",
    "",
    "```python",
    "def f():",
    "",
    "    return 1",
  ].join("\n"),
  // Windows and old-Mac line endings, which the parser normalizes first.
  "# Title\r\n\r\nFirst paragraph\r\nsecond line\r\n\r\n- item\r\n- item two\r\n\r\n| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n\r\ndone\rtail",
  "  \n\n\n# after blank lines\n\n\n\nparagraph\n\n",
];

// Line fragments weighted toward the constructs whose boundaries matter.
const FRAGMENTS: readonly string[] = [
  "", "", "", "   ", "# Heading", "### deep **bold**", "- item", "* star item", "10. ordered",
  "  continued", "    indented", "> quote", ">", "> > nested", "```", "```ts", "~~~", "````",
  "| a | b |", "|---|---|", "| :-- | --: |", "| 1 | 2 |", "a | b", "| `x | y` | z |",
  "plain prose with *emphasis*", "trailing \\|", "---", "***", "_ _ _",
  "::code-comment{title=\"t\" body=\"b\"}", "text :codex-annotation{index=\"1\"}", "[a](b)",
  "<https://example.com>", "~~strike~~", "`code`", "**unfinished",
  "set max_tokens and top_p", "__init__.py", "my__init__py", "2 * 3 * 4", "*a* _a_ __b__",
  "| snake_case | 1_024 |",
];

function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6D2B79F5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function randomDocument(next: () => number): string {
  const lines: string[] = [];
  const count = 4 + Math.floor(next() * 14);
  for (let line = 0; line < count; line += 1) {
    lines.push(FRAGMENTS[Math.floor(next() * FRAGMENTS.length)]!);
  }
  const ending = next();
  const joined = lines.join(ending < 0.8 ? "\n" : ending < 0.95 ? "\r\n" : "\r");
  return next() < 0.3 ? `${joined}\n` : joined;
}

const RANDOM_DOCUMENTS: readonly string[] = (() => {
  const next = random(20260927);
  return Array.from({ length: 160 }, () => randomDocument(next));
})();

function grow(text: string, steps: readonly number[]): void {
  let previous: MarkdownParse | undefined;
  for (const end of steps) {
    const prefix = text.slice(0, end);
    previous = parseMarkdownIncremental(prefix, previous);
    expect(previous.blocks, JSON.stringify(prefix)).toEqual(parseMarkdown(prefix));
  }
}

describe("parseMarkdownIncremental", () => {
  it("equals a full parse at every prefix of a message streamed one character at a time", () => {
    for (const text of [...DOCUMENTS, ...RANDOM_DOCUMENTS]) {
      grow(text, Array.from({ length: text.length + 1 }, (_, end) => end));
    }
  });

  it("equals a full parse when the message arrives in uneven chunks", () => {
    const next = random(7);
    for (const text of [...DOCUMENTS, ...RANDOM_DOCUMENTS]) {
      const steps: number[] = [];
      for (let end = 0; end < text.length;) {
        end = Math.min(text.length, end + 1 + Math.floor(next() * 40));
        steps.push(end);
      }
      grow(text, steps);
    }
  });

  it("equals a full parse after any edit, not just an append", () => {
    const next = random(11);
    const all = [...DOCUMENTS, ...RANDOM_DOCUMENTS];
    for (const text of all) {
      let previous = parseMarkdownIncremental(text);
      for (let edit = 0; edit < 6; edit += 1) {
        const at = Math.floor(next() * (text.length + 1));
        const other = all[Math.floor(next() * all.length)]!;
        const choice = next();
        // Truncate, rewrite the tail, cut from the middle, or switch messages.
        const edited = choice < 0.25 ? text.slice(0, at)
          : choice < 0.5 ? text.slice(0, at) + other.slice(Math.floor(next() * other.length))
          : choice < 0.75 ? text.slice(0, at) + text.slice(Math.min(text.length, at + 5))
          : other;
        previous = parseMarkdownIncremental(edited, previous);
        expect(previous.blocks, JSON.stringify(edited)).toEqual(parseMarkdown(edited));
      }
    }
  });

  it("keeps every settled block's object while the tail streams", () => {
    const head = "# Title\n\nFirst paragraph.\n\n- one\n- two\n\n";
    const first = parseMarkdownIncremental(`${head}The tail`);
    const second = parseMarkdownIncremental(`${head}The tail grows`, first);
    expect(second.blocks).toHaveLength(4);
    for (let index = 0; index < 3; index += 1) expect(second.blocks[index]).toBe(first.blocks[index]);
    expect(second.blocks[3]).not.toBe(first.blocks[3]);
    // An unchanged text is the same parse, and a new blank line becomes the next resume point.
    expect(parseMarkdownIncremental(`${head}The tail grows`, second)).toBe(second);
    const third = parseMarkdownIncremental(`${head}The tail grows\n\nNew`, second);
    expect(third.blocks[3]).toEqual(second.blocks[3]);
    expect(third.blocks[3]).toBe(second.blocks[3]);
  });

  it("never resumes inside a fence that has not closed", () => {
    // No checkpoint inside an open fence: the blank line there is code, and a
    // later close would have to reclaim it.
    const open = parseMarkdownIncremental("Intro\n\n```\na\n\nb");
    expect(open.blocks).toEqual(parseMarkdown("Intro\n\n```\na\n\nb"));
    const closed = parseMarkdownIncremental("Intro\n\n```\na\n\nb\n```\n\nafter", open);
    expect(closed.blocks).toEqual(parseMarkdown("Intro\n\n```\na\n\nb\n```\n\nafter"));
    expect(closed.blocks[0]).toBe(open.blocks[0]);
  });
});

describe("splitMarkdownTableRow", () => {
  it("answers no for a line without a pipe before walking it", () => {
    expect(splitMarkdownTableRow("ordinary prose with `code` and \\* escapes")).toBeUndefined();
    expect(splitMarkdownTableRow("a | b")).toEqual(["a", "b"]);
  });
});
